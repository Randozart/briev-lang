// ── Parser Shared Utilities ────────────────────────────────────────────
// 2026-07-12: Phase 1.6 — expect, advance, peek, error reporting, span tracking.
// Flat code: each function is max 2 levels of nesting.

use crate::ast::{Annotation, Expr, TopLevel};
use crate::errors::{Span, SyntaxError};
use crate::lexer::Token;
use std::collections::HashSet;

/// 2026-09-22 (order-free-modifiers plan): `<keywords>* <identifier> <name>`.
/// Modifier/strategy keywords compose in any order, then exactly one
/// structural identifier, then the name. This struct carries what
/// `consume_modifier_prefix` collected; the dispatch validates the
/// identifier against the modifier set.
#[derive(Debug, Clone, Default)]
pub struct ModifierPrefix {
    pub annotations: Vec<Annotation>,
    pub is_async: bool,
    pub sync_groups: Option<Vec<String>>,
}

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
    /// 2026-09-22 (unified-metaprogramming plan): the most recently parsed
    /// `...name` rest parameter (TypeScript-style), recorded by
    /// `parse_parameter_list`. Compile-time `$defn`/`$txn` paths consume it
    /// into `Definition.variadic_param`; runtime `defn`/`txn`/op paths
    /// reject it.
    pub pending_variadic: Option<String>,
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
            pending_variadic: None,
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

    /// Consume a run of modifier/strategy keywords in ANY order and return
    /// what was seen. Stops at the first non-modifier token (the structural
    /// identifier). Duplicate modifiers are a hard error. `bootstrap` is a
    /// FIXED compound (`bootstrap node`) and is NOT consumed here.
    pub fn consume_modifier_prefix(&mut self) -> Result<ModifierPrefix, SyntaxError> {
        let mut prefix = ModifierPrefix::default();
        loop {
            // Data-driven: the annotation-name flavor of each keyword token.
            // `seq`/`pack`/`coll` before a struct/obj are LAYOUT flags the
            // struct/obj parser owns (`seq struct`, `pack seq struct`), so the
            // scanner skips them when the run heads for a struct.
            match self.peek() {
                Some(Token::Seq) if !self.peek_targets_struct_like() => {
                    self.record_annotation(&mut prefix, "seq")?;
                }
                Some(Token::Pack) if !self.peek_targets_struct_like() => {
                    self.record_annotation(&mut prefix, "pack")?;
                }
                Some(Token::Coll) if !self.peek_targets_struct_like() => {
                    self.record_annotation(&mut prefix, "coll")?;
                }
                Some(Token::Accel) => self.record_annotation(&mut prefix, "accel")?,
                Some(Token::Out) => self.record_annotation(&mut prefix, "out")?,
                Some(Token::Mem) => self.record_annotation(&mut prefix, "mem")?,
                Some(Token::Reg) => self.record_annotation(&mut prefix, "reg")?,
                Some(Token::Vol) => self.record_annotation(&mut prefix, "vol")?,
                Some(Token::Async) => {
                    if prefix.is_async {
                        return Err(self.dup_modifier("async"));
                    }
                    self.pos += 1;
                    prefix.is_async = true;
                }
                Some(Token::Sync) => {
                    // `sync<group>` — the group barrier classifier.
                    // Parameterized (SPEC §12.1); validated by the dispatcher.
                    if prefix.sync_groups.is_some() {
                        return Err(self.dup_modifier("sync"));
                    }
                    self.pos += 1;
                    prefix.sync_groups = Some(self.parse_sync_groups()?);
                }
                Some(Token::Identifier(s))
                    if (s == "net" || s == "stdnet")
                        && matches!(self.tokens.get(self.pos + 1).map(|(t, _)| t), Some(Token::Lt)) =>
                {
                    // 2026-09-25 (E14b-7, rail-membership plan): `net<name>`
                    // / `stdnet<name>` — the supply-net strategy keywords.
                    // Contextual identifiers, not reserved tokens: the arm
                    // only fires when directly followed by `<` in a
                    // declaration prefix, where no comparison syntax can
                    // occur. Payload is a canonical identifier string
                    // ("NAME" or "pin:NAME,pin:NAME") carried as the
                    // annotation value; the analysis splits it — the
                    // compiler never reads the name as physics (Rule 15).
                    let keyword = s.clone();
                    if prefix.annotations.iter().any(|a| a.name == keyword) {
                        return Err(self.dup_modifier(&keyword));
                    }
                    self.pos += 1;
                    let payload = self.parse_net_modifier_payload(&keyword)?;
                    prefix.annotations.push(Annotation {
                        name: keyword,
                        value: Some(Expr::Quoted(payload.into_bytes())),
                    });
                }
                _ => break,
            }
        }
        Ok(prefix)
    }

    /// Consume the current modifier token and record its annotation, or error
    /// on a duplicate.
    fn record_annotation(
        &mut self,
        prefix: &mut ModifierPrefix,
        name: &str,
    ) -> Result<(), SyntaxError> {
        self.pos += 1;
        if prefix.annotations.iter().any(|a| a.name == name) {
            return Err(self.dup_modifier(name));
        }
        prefix.annotations.push(Annotation {
            name: name.to_string(),
            value: None,
        });
        Ok(())
    }

    /// `sync<group>` — parse the comma-separated domain list (SPEC §12.1).
    fn parse_sync_groups(&mut self) -> Result<Vec<String>, SyntaxError> {
        if self.eat(&Token::Lt) {
            let mut names = Vec::new();
            loop {
                names.push(self.expect_identifier()?);
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect(Token::Gt)?;
            Ok(names)
        } else {
            Ok(vec![])
        }
    }

    /// 2026-09-25 (E14b-7): the payload of `net<...>` / `stdnet<...>` —
    /// either one bare net name (`<v3_3>`, `<VBUS>`) or a comma list of
    /// pin-qualified names (`<in: VBUS, vout: V3V3>`). Canonicalized to
    /// "NAME" or "pin:NAME,..." (identifiers cannot contain the
    /// separators, so the analysis split is unambiguous).
    fn parse_net_modifier_payload(&mut self, keyword: &str) -> Result<String, SyntaxError> {
        self.expect(Token::Lt)?;
        let mut parts: Vec<String> = Vec::new();
        loop {
            let first = self.expect_identifier()?;
            if self.eat(&Token::Colon) {
                let name = self.expect_identifier()?;
                parts.push(format!("{first}:{name}"));
            } else {
                parts.push(first);
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(Token::Gt)?;
        if parts.is_empty() {
            return self.error_at_current(&format!(
                "`{keyword}<>` names nothing — give a net name (`{keyword}<VBUS>`) or \
                 pin-qualified names (`{keyword}<in: VBUS, vout: V3V3>`)"
            ));
        }
        Ok(parts.join(","))
    }

    /// True when the current `seq`/`pack`/`coll` modifier is heading for a
    /// struct/obj (a layout flag the struct parser owns) rather than a
    /// node/txn/let/defn. Lookahead over the modifier run to the first
    /// structural identifier.
    fn peek_targets_struct_like(&self) -> bool {
        let mut i = self.pos;
        while let Some((t, _)) = self.tokens.get(i) {
            if Self::is_modifier_token_after(t) {
                i += 1;
            } else if matches!(t, Token::Sync) {
                i = Self::skip_sync_groups(i, &self.tokens);
            } else {
                return matches!(
                    t,
                    Token::Struct | Token::Obj | Token::Union | Token::Type
                );
            }
        }
        false
    }

    /// True when the token is a bare modifier keyword (no parameter). `sync`
    /// is parameterized (`sync<g>`) and handled separately.
    fn is_modifier_token_after(t: &Token) -> bool {
        matches!(
            t,
            Token::Seq
                | Token::Pack
                | Token::Coll
                | Token::Accel
                | Token::Async
                | Token::Out
                | Token::Mem
                | Token::Reg
                | Token::Vol
        )
    }

    /// Skip a `sync<g>` parameter list, returning the index after the `>`.
    fn skip_sync_groups(
        mut i: usize,
        tokens: &[(Token, std::ops::Range<usize>)],
    ) -> usize {
        i += 1; // sync
        if tokens.get(i).is_some_and(|(t, _)| matches!(t, Token::Lt)) {
            i += 1;
            while let Some((t, _)) = tokens.get(i) {
                if matches!(t, Token::Identifier(_) | Token::Comma) {
                    i += 1;
                } else {
                    break;
                }
            }
            if tokens.get(i).is_some_and(|(t, _)| matches!(t, Token::Gt)) {
                i += 1;
            }
        }
        i
    }

    /// 2026-09-22 (order-free-modifiers plan): true when the modifier run at
    /// the current position is heading for a `let` (the statement-level let
    /// modifiers). The statement parser only routes vol/out/mem/reg here when
    /// the run terminates at `let`.
    pub(crate) fn peek_targets_let(&self) -> bool {
        let mut i = self.pos;
        while let Some((t, _)) = self.tokens.get(i) {
            match t {
                Token::Mem | Token::Reg | Token::Vol | Token::Out => i += 1,
                _ => return matches!(t, Token::Let),
            }
        }
        false
    }

    fn dup_modifier(&self, name: &str) -> SyntaxError {
        SyntaxError::UnexpectedToken {
            expected: format!("a structural identifier after modifiers (duplicate '{name}')"),
            found: format!("{}", self.peek().map(|t| format!("{t}")).unwrap_or_else(|| "EOF".into())),
            span: self
                .tokens
                .get(self.pos)
                .map(|(_, s)| self.make_span(s.clone()))
                .unwrap_or_else(crate::errors::Span::dummy),
        }
    }

    /// Peek at the token after the current one without consuming anything.
    /// Used for lookahead disambiguation (e.g. `Int[8]` array vs `Int [pre]`
    /// contract after a return type).
    pub fn peek_next(&self) -> Option<&Token> {
        self.tokens.get(self.pos + 1).map(|(t, _)| t)
    }

    /// 2026-09-16: true when the token after the current one is an identifier.
    /// Used by the chain-capture/back-reference postfix disambiguation.
    pub fn peek_next_is_identifier(&self) -> bool {
        self.tokens
            .get(self.pos + 1)
            .is_some_and(|(t, _)| matches!(t, Token::Identifier(_)))
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

    /// 2026-09-21: Return the source text of the current token and advance.
    /// Used by raw-body parsers (bad fn) to accumulate verbatim text.
    pub fn token_text(&mut self) -> String {
        if let Some((tok, range)) = self.tokens.get(self.pos) {
            let text = self.source[range.clone()].to_string();
            self.pos += 1;
            text
        } else {
            String::new()
        }
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

    /// Check if the token AFTER the current one is an identifier with a
    /// specific name (the `bootstrap bad` disambiguation).
    pub fn lookahead_is_identifier(&self, name: &str) -> bool {
        self.tokens
            .get(self.pos + 1)
            .map_or(false, |(t, _)| matches!(t, Token::Identifier(s) if s == name))
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
            Token::Import => "import".into(),
            Token::From => "from".into(), Token::As => "as".into(),
            Token::Frgn => "frgn".into(),
            Token::Op => "op".into(), Token::Type => "type".into(),
            Token::Trait => "trait".into(), Token::Impl => "impl".into(),
            Token::Cell => "cell".into(),             Token::Struct => "struct".into(),
            Token::Render => "render".into(),
            Token::Fab => "fab".into(),
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
