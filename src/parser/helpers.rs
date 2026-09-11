// ── Parser Shared Utilities ────────────────────────────────────────────
// 2026-07-12: Phase 1.6 — expect, advance, peek, error reporting, span tracking.
// Flat code: each function is max 2 levels of nesting.

use crate::ast::TopLevel;
use crate::errors::{Span, SyntaxError};
use crate::lexer::Token;
use std::collections::HashSet;

pub struct Parser<'a> {
    pub tokens: Vec<(Token, std::ops::Range<usize>)>,
    pub pos: usize,
    pub source: &'a str,
    pub strict_mode: bool,
    /// 2026-07-24: Pending doc comment to attach to the next definition.
    pub pending_doc: Option<String>,
    /// 2026-07-25: Pending `>` split from `>>` in nested generics.
    /// When the type parser consumes `>>` as a single `>`, it sets this flag
    /// so the next `expect(Gt)` or `eat(Gt)` uses the pending token.
    pub pending_gt: bool,
    /// 2026-08-04 (Phase 1): names that denote TYPES, pre-scanned from the
    /// token stream. Used to disambiguate the C-style cast `(Type) expr` from
    /// grouping `(expr)` — `(x) - 1` is grouping-minus, `(Int) -1` is a cast.
    /// Mirrors the C typedef-table approach; includes primitives, hashwords,
    /// and in-file `type`/`struct`/`obj`/`enum`/`meld` declaration names.
    pub known_types: HashSet<String>,
}

impl<'a> Parser<'a> {
    pub fn new(tokens: Vec<(Token, std::ops::Range<usize>)>, source: &'a str) -> Self {
        let mut p = Parser {
            known_types: HashSet::new(),
            tokens,
            pos: 0,
            source,
            strict_mode: false,
            pending_doc: None,
            pending_gt: false,
        };
        p.prescan_known_types();
        p
    }

    /// 2026-08-04 (Phase 1): collect type names from the token stream for the
    /// C-style cast disambiguation. Primitives + hashwords (`Int`, `String`)
    /// are always types; `type`/`struct`/`obj`/`enum`/`meld` declaration names
    /// are collected from their declaration sites. Cheap, single pass.
    fn prescan_known_types(&mut self) {
        for name in [
            "Int", "UInt", "Float", "Float32", "F32", "Float64", "F64", "Double",
            "String", "Bool", "Void", "Char", "Blob", "Bit", "bits", "Ptr",
        ] {
            self.known_types.insert(name.to_string());
        }
        let toks = &self.tokens;
        let mut i = 0;
        while i < toks.len() {
            let is_decl = matches!(toks[i].0, Token::Type | Token::Struct | Token::Obj | Token::Enum | Token::Meld);
            if is_decl {
                if let Some((Token::Identifier(name), _)) = toks.get(i + 1) {
                    self.known_types.insert(name.clone());
                }
                i += 2;
                continue;
            }
            // Hashword categories are types: `Int`, `String`, `String<UTF8>`.
            if let Token::Identifier(name) = &toks[i].0 {
                if name.starts_with('#') {
                    self.known_types.insert(name.clone());
                }
            }
            i += 1;
        }
    }

    pub fn with_strict_mode(mut self, mode: bool) -> Self {
        self.strict_mode = mode;
        self
    }

    /// Peek at the current token without consuming it.
    pub fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    /// Peek at the token after the current one without consuming anything.
    /// Used for lookahead disambiguation (e.g. `Int[8]` array vs `Int [pre]`
    /// contract after a return type).
    pub fn peek_next(&self) -> Option<&Token> {
        self.tokens.get(self.pos + 1).map(|(t, _)| t)
    }

    /// 2026-08-05 (Phase 3): `optional frgn` — true when the current token is
    /// the identifier `optional` and the following token is the `frgn` keyword.
    pub fn peek_next_is_frgn(&self) -> bool {
        matches!(self.peek_next(), Some(Token::Frgn))
    }

    /// 2026-08-05 (Phase 3): true when the current token is a canonical
    /// duration unit — the `cyc`/`ms` tokens or the contextual identifiers
    /// `cyc`, `ns`, `ms`, `s`, `min` (SPEC §16.1).
    pub fn lookahead_is_duration_unit(&self) -> bool {
        match self.peek() {
            Some(Token::Cyc) | Some(Token::Ms) => true,
            Some(Token::Identifier(u)) => {
                matches!(u.as_str(), "cyc" | "ns" | "ms" | "s" | "min")
            }
            _ => false,
        }
    }

    /// Peek at the current token and its span.
    pub fn peek_with_span(&self) -> Option<(&Token, &std::ops::Range<usize>)> {
        self.tokens.get(self.pos).map(|(t, s)| (t, s))
    }

    /// Check if the current token matches a specific kind.
    pub fn check(&self, kind: &Token) -> bool {
        self.peek().map_or(false, |t| {
            std::mem::discriminant(t) == std::mem::discriminant(kind)
        })
    }

    /// Expect a specific token, consume it, or return an error.
    pub fn expect(&mut self, kind: Token) -> Result<(), SyntaxError> {
        // 2026-07-25: Check for pending `>` from `>>` splitting in nested generics.
        if matches!(kind, Token::Gt) && self.pending_gt {
            self.pending_gt = false;
            return Ok(());
        }
        let (cur, span) = self
            .peek_with_span()
            .ok_or_else(|| SyntaxError::UnexpectedEOF {
                expected: format!("{:?}", kind),
                span: Span::dummy(),
            })?;
        if std::mem::discriminant(cur) == std::mem::discriminant(&kind) {
            self.pos += 1;
            Ok(())
        } else {
            // 2026-08-06 (diagnostics): `x => body` is a common lambda spelling
            // mistake — match arms use `=>`, lambda parameters use `->`.
            let mut found = format!("{}", cur);
            if matches!(kind, Token::Semicolon) && matches!(cur, Token::FatArrow) {
                found = format!(
                    "{} (hint: match arms use '=>'; lambda parameters use '->', e.g. `x -> body`)",
                    found
                );
            }
            Err(SyntaxError::UnexpectedToken {
                expected: format!("{:?}", kind),
                found,
                span: self.make_span(span.clone()),
            })
        }
    }

    /// 2026-07-24: Take the pending doc comment and return it, clearing the buffer.
    pub fn take_doc(&mut self) -> Option<String> {
        self.pending_doc.take()
    }

    /// 2026-07-24: Set the pending doc comment from a DocComment token.
    pub fn set_doc(&mut self, text: String) {
        self.pending_doc = Some(text);
    }

    /// Advance past the current token and return it.
    pub fn advance(&mut self) -> Option<(Token, std::ops::Range<usize>)> {
        let tok = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        tok
    }

    /// Get the current token as an identifier string, or error.
    /// 2026-07-14: Also accepts keyword tokens that are commonly used as
    /// identifiers (reg, op, bank, asm, stage, cell, etc.).
    pub fn expect_identifier(&mut self) -> Result<String, SyntaxError> {
        match self.advance() {
            Some((Token::Identifier(name), _)) => Ok(name),
            Some((tok, span)) => {
                // 2026-08-22 (spec-conformance Phase 2): SPEC §4.1 reserved
                // words (`sed`, `pvt`, `reg`) and removed forms (`meld`) are
                // rejected as identifier spellings with their own messages
                // instead of being silently accepted via the keyword-as-
                // identifier fallback. Undo: re-add their arms to
                // keyword_as_identifier below.
                if let Some(msg) = Self::removed_word_message(&tok) {
                    return Err(SyntaxError::InvalidStatement {
                        reason: msg.to_string(),
                        span: self.make_span(span),
                    });
                }
                if let Some(name) = self.keyword_as_identifier(&tok) {
                    Ok(name)
                } else if let Some(kw) = Self::reserved_keyword_name(&tok) {
                    // 2026-08-17: a RESERVED keyword (modifier/strategy word
                    // like `out`, `vol`, `seq`) cannot be an identifier here —
                    // say so specifically instead of the generic "expected
                    // identifier, found 'out'".
                    Err(SyntaxError::ReservedKeyword {
                        keyword: kw.to_string(),
                        span: self.make_span(span),
                    })
                } else {
                    Err(SyntaxError::UnexpectedToken {
                        expected: "identifier".into(),
                        found: format!("{}", tok),
                        span: self.make_span(span),
                    })
                }
            }
            None => Err(SyntaxError::UnexpectedEOF {
                expected: "identifier".into(),
                span: Span::dummy(),
            }),
        }
    }

    /// Shortcut: check for Digit
    pub fn expect_integer(&mut self) -> Result<i64, SyntaxError> {
        match self.advance() {
            Some((Token::Integer(n), _)) => Ok(n),
            Some((tok, span)) => Err(SyntaxError::UnexpectedToken {
                expected: "integer".into(),
                found: format!("{}", tok),
                span: self.make_span(span),
            }),
            None => Err(SyntaxError::UnexpectedEOF {
                expected: "integer".into(),
                span: Span::dummy(),
            }),
        }
    }

    /// Get the current token as a string literal, or error.
    pub fn expect_string(&mut self) -> Result<String, SyntaxError> {
        match self.advance() {
            Some((Token::String(s), _)) => Ok(s),
            Some((tok, span)) => Err(SyntaxError::UnexpectedToken {
                expected: "string literal".into(),
                found: format!("{}", tok),
                span: self.make_span(span),
            }),
            None => Err(SyntaxError::UnexpectedEOF {
                expected: "string literal".into(),
                span: Span::dummy(),
            }),
        }
    }

    /// Check if the current token is any identifier.
    /// 2026-08-01 (C2): for `-> handler(val)` — an identifier arg name.
    pub fn peek_is_identifier(&self) -> bool {
        self.peek().map_or(false, |t| matches!(t, Token::Identifier(_)))
    }

    /// Check if the current token is an identifier with a specific name.
    pub fn check_identifier(&self, name: &str) -> bool {
        self.peek()
            .map_or(false, |t| matches!(t, Token::Identifier(s) if s == name))
    }

    /// Consume a specific identifier if present.
    pub fn eat_identifier(&mut self, name: &str) -> bool {
        if self.check_identifier(name) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Consume a token if it matches, without error.
    pub fn eat(&mut self, kind: &Token) -> bool {
        // 2026-07-25: Check for pending `>` from `>>` splitting in nested generics.
        if matches!(kind, Token::Gt) && self.pending_gt {
            self.pending_gt = false;
            return true;
        }
        if self.check(kind) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// 2026-07-25: Consume `>` or `>>` as a type close bracket.
    /// `>>` in nested generics like `Foo<Bar<Int>>` is lexed as a single Shr token.
    /// This method consumes it as one `>` and sets pending_gt for the second.
    pub fn eat_type_close(&mut self) -> bool {
        if self.eat(&Token::Gt) {
            return true;
        }
        if self.eat(&Token::Shr) {
            self.pending_gt = true;
            return true;
        }
        false
    }

    /// Create a Span from a logos byte range.
    pub fn make_span(&self, range: std::ops::Range<usize>) -> Span {
        let start = range.start;
        let end = range.end;
        let line = self.source[..start].lines().count();
        let column = start - self.source[..start].rfind('\n').map_or(0, |i| i + 1);
        Span::new(start, end, line, column + 1)
    }

    /// Report an error at the current position.
    pub fn error_at_current<T>(&self, msg: &str) -> Result<T, SyntaxError> {
        let span = self
            .peek_with_span()
            .map(|(_, s)| self.make_span(s.clone()))
            .unwrap_or(Span::dummy());
        Err(SyntaxError::InvalidExpression {
            reason: msg.to_string(),
            span,
        })
    }

    /// 2026-08-22 (spec-conformance plan Phase 2): an unrecognized
    /// declaration word at top level or statement head (SPEC §4.1: a wrong
    /// keyword spelling gives a suggested correction). A canonical keyword
    /// in the WRONG POSITION is not a spelling problem — say so instead of
    /// suggesting the word to itself (`endprogram` at top level).
    pub fn error_unknown_item<T>(&self, name: &str, position: &str) -> Result<T, SyntaxError> {
        static VOCAB: std::sync::OnceLock<crate::vocab::LanguageVocab> = std::sync::OnceLock::new();
        let vocab = VOCAB.get_or_init(crate::vocab::LanguageVocab::canonical);
        let reason = if vocab.is_canonical_keyword(name) {
            format!(
                "'{name}' is not valid at this position ({position}) — check the \
                 surrounding syntax"
            )
        } else {
            match crate::vocab::keyword_hint(vocab, name) {
                Some(hint) => format!("unknown {position} '{name}' — {hint}"),
                None => format!("unknown {position} '{name}'"),
            }
        };
        self.error_at_current(&reason)
    }

    /// Report an error at a specific span.
    pub fn error_at<T>(&self, msg: &str, span: Span) -> Result<T, SyntaxError> {
        Err(SyntaxError::InvalidExpression {
            reason: msg.to_string(),
            span,
        })
    }

    /// 2026-08-05 (normative spec Phase 0): report a construct that is
    /// normative in spec/SPEC.md but not yet implemented. The compiler must
    /// reject it explicitly rather than accept placeholder/subset semantics.
    pub fn error_staged<T>(&self, feature: &str) -> Result<T, SyntaxError> {
        let span = self
            .peek_with_span()
            .map(|(_, s)| self.make_span(s.clone()))
            .unwrap_or(Span::dummy());
        Err(SyntaxError::StagedFeature {
            feature: feature.to_string(),
            span,
        })
    }

    /// 2026-07-26: Read the body of a `render struct/obj { ... }` block.
    /// The open brace `{` must already be consumed (via expect(LBrace)).
    /// Tracks brace depth through the token stream. Returns the raw HTML
    /// text sliced from the source between `{` and the matching `}`.
    /// Advances the parser position past the closing `}`.
    pub fn read_html_body(&mut self) -> Result<String, SyntaxError> {
        // Position of the first token after '{' — this is where HTML starts
        let start = self.peek_with_span()
            .map(|(_, s)| s.start)
            .unwrap_or(self.pos);
        let mut depth: u64 = 1;
        loop {
            let Some((tok, span)) = self.tokens.get(self.pos).cloned() else {
                return Err(SyntaxError::InvalidExpression {
                    reason: "unexpected end of file in render block body (missing })".into(),
                    span: crate::errors::Span::dummy(),
                });
            };
            match tok {
                Token::LBrace => { depth += 1; self.pos += 1; }
                Token::RBrace => {
                    depth -= 1;
                    self.pos += 1;
                    if depth == 0 {
                        let raw = &self.source[start..span.start];
                        return Ok(raw.trim().to_string());
                    }
                }
                _ => { self.pos += 1; }
            }
        }
    }

    /// 2026-07-14: Bridge between lexer keyword tokens and parser identifier matching.
    /// Maps keyword tokens (Frgn, Struct, Enum, Ok, etc.) to their string representations.
    pub fn keyword_as_identifier(&self, tok: &Token) -> Option<String> {
        Some(match tok {
            Token::Export => "export".into(),
            Token::Defn => "defn".into(), Token::Let => "let".into(),
            Token::Const => "const".into(), Token::Txn => "txn".into(),
            Token::Node => "node".into(), Token::Async => "async".into(),
            Token::Await => "await".into(),             Token::Term => "term".into(), Token::EndProgram => "endprogram".into(),
            Token::Rollback => "rollback".into(),
            Token::Defer => "defer".into(),
            Token::Mutex => "mutex".into(),
            Token::Barrier => "barrier".into(),
            Token::Import => "import".into(),
            Token::From => "from".into(), Token::As => "as".into(),
            Token::Frgn => "frgn".into(),
            Token::Op => "op".into(), Token::Type => "type".into(),
            Token::Trait => "trait".into(), Token::Impl => "impl".into(),
            Token::Cell => "cell".into(),             Token::Struct => "struct".into(),
            Token::Render => "render".into(),
            Token::Enum => "enum".into(), Token::Trg => "trg".into(),
            Token::Within => "within".into(),
            Token::Match => "match".into(),
            // 2026-07-15: Template/Macro tokens removed
            Token::Quote => "quote".into(),
            Token::Dollar => "$".into(), Token::DollarBang => "$!".into(),
            Token::Foreach => "foreach".into(), Token::Break => "break".into(),
            Token::Sync => "sync".into(),
            Token::Underscore => "_".into(),
            Token::When => "when".into(),
            Token::Cyc => "cyc".into(),
            Token::Ms => "ms".into(),
            Token::Input => "input".into(), Token::Output => "output".into(),
            Token::BoolTrue => "true".into(), Token::BoolFalse => "false".into(),
            // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): atomic
            // ordering constants in expression position — the trailing
            // argument of an atomic intrinsic (`AtomicLoad#(p, relaxed)`).
            // Consumed as ordering markers by the emitter/interpreter;
            // anywhere else they surface as an unknown-identifier error.
            Token::Relaxed => "relaxed".into(),
            Token::Acquire => "acquire".into(),
            Token::Release => "release".into(),
            Token::Bartered => "bartered".into(),
            _ => return None,
        })
    }

    /// 2026-08-17: report the keyword name for tokens that are RESERVED (a
    /// modifier/strategy/construct keyword that CANNOT be used as an
    /// identifier) — the complement of `keyword_as_identifier`. These are the
    /// words a user most often mistakes for a variable name (`out`, `vol`,
    /// `seq`, `pack`, ...). When `expect_identifier` hits one it emits a
    /// specific "reserved keyword" error instead of the generic
    /// "expected identifier, found 'out'".
    /// 2026-08-22 (spec-conformance Phase 2): message for tokens that are
    /// removed surface (§4.4) or reserved words (§4.1) when they appear
    /// where a name is expected. `None` for everything else.
    pub(crate) fn removed_word_message(tok: &Token) -> Option<&'static str> {
        Some(match tok {
            Token::Meld => "`meld` was removed — structural sums (`Int | String`) and \
                            `coll` declarations replace it",
            Token::Reg => "`reg` is reserved and cannot be used as a name",
            Token::Pvt => "`pvt` is reserved and cannot be used as a name",
            Token::Sed => "`sed` is reserved and cannot be used as a name",
            _ => return None,
        })
    }

    fn reserved_keyword_name(tok: &Token) -> Option<&'static str> {
        Some(match tok {
            Token::Out => "out",
            Token::Vol => "vol",
            Token::Seq => "seq",
            Token::Pack => "pack",
            Token::Coll => "coll",
            Token::Accel => "accel",
            Token::Atomic => "atomic",
            Token::Relaxed => "relaxed",
            Token::Acquire => "acquire",
            Token::Release => "release",
            Token::Bartered => "bartered",
            Token::Union => "union",
            Token::Trap => "trap",
            Token::Halt => "halt",
            Token::Spawn => "spawn",
            Token::BeginProgram => "beginprogram",
            _ => return None,
        })
    }

    /// Check if we're at end of file.
    pub fn is_at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }
}
