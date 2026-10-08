// ── .bld (BILLD — Briev Intermediate Low-Level Dialect) parser ─────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-10-08 (BILLD plan, docs/plans/2026-10-08-billd-intermediate-dialect.md):
// braced, Briev-shaped parser for .bld — the execution-recipe tier.
// Shares the Briev TOKEN stream and the shared expression/type parsers;
// owns its own top-level and statement dispatch because .bld permits
// what .bv forbids (unbounded `loop`/`while`, `when`/`else`, `return`) and
// forbids what .bv requires (reactor statements, `term`, `foreach` …).
//
// Contextual keywords (documented dialect boundary, no global lexer
// change): `else`, `loop`, `while`, `continue`, `return`, `bad`
// are ordinary identifiers in the shared lexer and only recognized at
// .bld statement heads — a .bv identifier named `loop` keeps working.
// `when` is a SHARED keyword token (.bv's conditional) that .bld ACCEPTS
// — the conditional spelling is `when` in every dialect (Briev has no
// `if`; a statement-head `if` gets a fix pointing at `when`). Conversely
// `halt;`, `term;`, `foreach …` are SHARED keyword tokens that .bld
// REJECTS with a fix (one honest spelling per meaning, Rule 3).
//
// Grammar sketch:
//   item     := import | const | defn | bootstrap
//   stmt     := let | assign/compound | call | when | loop | while
//            |  break | continue | return | block | bad-block
//   expr     := the shared Briev expression parser
//
// To undo: delete this file + src/ast/bld.rs, revert the `bld` module
// lines, and drop the compound-assign tokens.

use crate::ast::bld::*;
use crate::ast::{BinaryOpKind, Expr};
use crate::errors::SyntaxError;
use crate::lexer::Token;
use crate::parser::Parser;

/// Parse a whole .bld source file.
pub fn parse_bld(source: &str) -> Result<BldProgram, SyntaxError> {
    let tokens = crate::lexer::tokenize(source).map_err(|e| SyntaxError::InvalidExpression {
        reason: format!(".bld source cannot be tokenized: {e}"),
        span: crate::errors::Span::dummy(),
    })?;
    let mut p = Parser::new(tokens, source);
    p.parse_bld_program()
}

impl<'a> Parser<'a> {
    // ── Top level ─────────────────────────────────────────────────────

    pub(crate) fn parse_bld_program(&mut self) -> Result<BldProgram, SyntaxError> {
        let mut items: Vec<BldTopLevel> = Vec::new();
        let mut saw_bootstrap = false;
        while let Some(tok) = self.peek().cloned() {
            match tok {
                Token::DocComment(_) | Token::DocCommentBang(_) | Token::Semicolon => {
                    self.pos += 1;
                }
                Token::Import => {
                    let imp = self.parse_bld_import()?;
                    items.push(BldTopLevel::Import(imp));
                }
                Token::Const => {
                    let cst = self.parse_bld_const()?;
                    items.push(BldTopLevel::Const(cst));
                }
                Token::Defn => {
                    let defn = self.parse_bld_defn()?;
                    items.push(BldTopLevel::Defn(defn));
                }
                Token::Bootstrap => {
                    let span = self.current_span();
                    if saw_bootstrap {
                        return Err(SyntaxError::InvalidStatement {
                            reason: "a .bld file has at most ONE `bootstrap` entry — \
                                     merge the recipes, or split the file and pick one \
                                     entry per image"
                                .to_string(),
                            span,
                        });
                    }
                    if self.lookahead_is_identifier("bad") {
                        return Err(SyntaxError::InvalidStatement {
                            reason: "`bootstrap bad` is the .bv spelling — a .bld body \
                                     is already BILLD; write `bootstrap <Name>() { … }` \
                                     (use `bad { … }` blocks inside for .bad text)"
                                .to_string(),
                            span,
                        });
                    }
                    saw_bootstrap = true;
                    let bs = self.parse_bld_bootstrap()?;
                    items.push(BldTopLevel::Bootstrap(bs));
                }
                Token::Identifier(name) => {
                    return Err(SyntaxError::InvalidStatement {
                        reason: self.bld_unknown_top_level(&name),
                        span: self.current_span(),
                    });
                }
                other => {
                    let word = format!("{other}");
                    let fix = "top level takes `import \"…\";`, `const NAME = …;`, \
                               `defn name() {{ … }}`, or `bootstrap Name() {{ … }}`; \
                               instructions live inside a body";
                    let dialect = if crate::vocab::LanguageVocab::canonical()
                        .is_canonical_keyword(&word)
                    {
                        format!("`{word}` is a .bv keyword, not a .bld item — {fix}")
                    } else {
                        format!("`{word}` cannot start a .bld item — {fix}")
                    };
                    return Err(SyntaxError::InvalidStatement {
                        reason: dialect,
                        span: self.current_span(),
                    });
                }
            }
        }
        let end = self.source.len();
        Ok(BldProgram {
            items,
            span: crate::errors::Span::new(0, end, 0, 0),
        })
    }

    /// House-style message for an unknown .bld top-level word: names the
    /// .bld item set, and says when the word is a .bv-only keyword.
    fn bld_unknown_top_level(&self, name: &str) -> String {
        if name == "if" {
            return "`if` is not a Briev keyword — the conditional is `when` in every \
                     dialect: `when cond { … } else { … }`"
                .to_string();
        }
        let fix = "a .bld file holds `import \"…\";`, `const NAME = …;`, \
                   `defn name(…) { … }`, and `bootstrap Name() { … }`";
        if crate::vocab::LanguageVocab::canonical().is_canonical_keyword(name) {
            format!("`{name}` is a .bv keyword, not a .bld item — {fix}")
        } else {
            format!("unknown .bld item `{name}` — {fix}")
        }
    }

    fn parse_bld_import(&mut self) -> Result<BldImport, SyntaxError> {
        let span = self.current_span();
        self.pos += 1; // `import`
        let path = match self.peek().cloned() {
            Some(Token::String(s)) => {
                self.pos += 1;
                s
            }
            _ => {
                return Err(SyntaxError::UnexpectedToken {
                    expected: "a quoted module path, e.g. `import \"std/bad/arch.bad\";`"
                        .to_string(),
                    found: describe_token(self.peek()),
                    span: self.current_span(),
                })
            }
        };
        self.expect(Token::Semicolon)?;
        Ok(BldImport { path, span })
    }

    fn parse_bld_const(&mut self) -> Result<BldConst, SyntaxError> {
        let span = self.current_span();
        self.pos += 1; // `const`
        let name = self.expect_identifier()?;
        self.expect(Token::Eq)?;
        let value = self.parse_expression()?;
        self.expect(Token::Semicolon)?;
        Ok(BldConst { name, value, span })
    }

    fn parse_bld_defn(&mut self) -> Result<BldDefn, SyntaxError> {
        let span = self.current_span();
        self.pos += 1; // `defn`
        let name = self.expect_identifier()?;
        let params = self.parse_bld_params()?;
        let ret = self.parse_return_type()?;
        self.reject_contracts_here()?;
        let body = self.parse_bld_body()?;
        Ok(BldDefn { name, params, ret, body, span })
    }

    fn parse_bld_bootstrap(&mut self) -> Result<BldBootstrap, SyntaxError> {
        let span = self.current_span();
        self.pos += 1; // `bootstrap`
        let name = self.expect_identifier()?;
        self.expect(Token::LParen)?;
        if !self.check(&Token::RParen) {
            return Err(SyntaxError::InvalidStatement {
                reason: "a `bootstrap` entry takes no parameters — the reset vector \
                         is called by the hardware, not by an ABI caller"
                    .to_string(),
                span: self.current_span(),
            });
        }
        self.expect(Token::RParen)?;
        let body = self.parse_bld_body()?;
        Ok(BldBootstrap { name, body, span })
    }

    fn parse_bld_params(&mut self) -> Result<Vec<BldParam>, SyntaxError> {
        self.expect(Token::LParen)?;
        let mut params = Vec::new();
        if !self.check(&Token::RParen) {
            loop {
                let span = self.current_span();
                let name = self.expect_identifier()?;
                let ty = self.parse_optional_type()?;
                params.push(BldParam { name, ty, span });
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
        }
        self.expect(Token::RParen)?;
        Ok(params)
    }

    /// v1: contracts live in `.bad { … }` blocks — reject the bracket here
    /// instead of silently ignoring it (never accept-then-drop).
    fn reject_contracts_here(&self) -> Result<(), SyntaxError> {
        if self.check(&Token::LBracket) {
            return Err(SyntaxError::InvalidStatement {
                reason: "contracts are not part of the .bld grammar (v1) — write \
                         them in a `bad { … }` block, where .bad positional contract \
                         groups (`name: [pre] [post]`) apply"
                    .to_string(),
                span: self.current_span(),
            });
        }
        Ok(())
    }

    // ── Bodies and statements ─────────────────────────────────────────

    /// `{ stmt; stmt; … }` — the `{` must be next.
    fn parse_bld_body(&mut self) -> Result<Vec<BldStmt>, SyntaxError> {
        if !self.check(&Token::LBrace) {
            return Err(SyntaxError::UnexpectedToken {
                expected: "`{` to open the body".to_string(),
                found: describe_token(self.peek()),
                span: self.current_span(),
            });
        }
        self.pos += 1;
        let mut stmts = Vec::new();
        loop {
            if self.check(&Token::RBrace) {
                self.pos += 1;
                return Ok(stmts);
            }
            if self.peek().is_none() {
                return Err(SyntaxError::UnexpectedEOF {
                    expected: "`}` to close this body".to_string(),
                    span: self.current_span(),
                });
            }
            if self.eat(&Token::Semicolon) {
                continue; // empty statement / stray separator — tolerated
            }
            stmts.push(self.parse_bld_statement()?);
        }
    }

    pub(crate) fn parse_bld_statement(&mut self) -> Result<BldStmt, SyntaxError> {
        // Doc comments are inert.
        if matches!(
            self.peek(),
            Some(Token::DocComment(_)) | Some(Token::DocCommentBang(_))
        ) {
            self.pos += 1;
            return self.parse_bld_statement();
        }
        let span = self.current_span();
        let Some(tok) = self.peek().cloned() else {
            return Err(SyntaxError::UnexpectedEOF {
                expected: "a .bld statement".to_string(),
                span: crate::errors::Span::dummy(),
            });
        };
        // Shared .bv keywords: rejected loudly with a .bld fix — never
        // silently reparsed into something else (dialect boundary).
        if let Some((word, fix)) = bv_keyword_rejection(&tok) {
            return Err(SyntaxError::InvalidStatement {
                reason: format!("`{word}` is a .bv statement — .bld recipes use {fix}"),
                span,
            });
        }
        match tok {
            Token::Let => self.parse_bld_let(),
            Token::When => self.parse_bld_when(),
            Token::Break => {
                self.pos += 1;
                self.expect(Token::Semicolon)?;
                Ok(BldStmt::Break { span })
            }
            Token::LBrace => {
                let body = self.parse_bld_body()?;
                Ok(BldStmt::Block { body, span })
            }
            Token::Import | Token::Defn | Token::Bootstrap | Token::Const => {
                Err(SyntaxError::InvalidStatement {
                    reason: format!(
                        "`{tok}` is a top-level .bld item — it cannot appear inside                          a body"
                    ),
                    span,
                })
            }
            Token::Identifier(name) => self.dispatch_bld_ident_stmt(&name, span),
            _ => self.parse_bld_expr_stmt(),
        }
    }

    /// Statement-head contextual keywords: ordinary identifiers in the
    /// shared lexer, reserved only where a statement starts (the .bld
    /// dialect boundary — no global lexer change).
    fn dispatch_bld_ident_stmt(
        &mut self, name: &str, span: crate::errors::Span,
    ) -> Result<BldStmt, SyntaxError> {
        match name {
            "loop" => self.parse_bld_loop(),
            "while" => self.parse_bld_while(),
            "if" => Err(SyntaxError::InvalidStatement {
                reason: "`if` is not a Briev keyword — the conditional is `when` in \
                         every dialect: `when cond { … } else { … }`"
                    .to_string(),
                span,
            }),
            "else" => Err(SyntaxError::InvalidStatement {
                reason: "stray `else` — it belongs to the `if` directly above it"
                    .to_string(),
                span,
            }),
            "continue" => {
                self.pos += 1;
                self.expect(Token::Semicolon)?;
                Ok(BldStmt::Continue { span })
            }
            "return" => {
                self.pos += 1;
                let value = if self.check(&Token::Semicolon) {
                    None
                } else {
                    Some(self.parse_expression()?)
                };
                self.expect(Token::Semicolon)?;
                Ok(BldStmt::Return { value, span })
            }
            "bad" if matches!(self.peek_next(), Some(&Token::LBrace)) => {
                self.parse_bld_bad_block()
            }
            "yield" => Err(SyntaxError::InvalidStatement {
                reason: "`yield` is a reactor-scheduler statement — .bld code runs                          naked; delete it or move the block to .bv"
                    .to_string(),
                span,
            }),
            _ => self.parse_bld_expr_stmt(),
        }
    }

    /// `let name: T? = expr;` — the initializer is required.
    fn parse_bld_let(&mut self) -> Result<BldStmt, SyntaxError> {
        let span = self.current_span();
        self.pos += 1; // `let`
        let name = self.expect_identifier()?;
        let ty = self.parse_optional_type()?;
        if !self.eat(&Token::Eq) {
            return Err(SyntaxError::InvalidStatement {
                reason: format!(
                    "`let {name}` needs an initializer — a .bld binding always has \
                     a value: `let {name} = …;`"
                ),
                span: self.current_span(),
            });
        }
        let value = self.parse_expression()?;
        self.expect(Token::Semicolon)?;
        Ok(BldStmt::Let { name, ty, value, span })
    }

    /// `when cond { … } else when cond { … } else { … }`
    fn parse_bld_when(&mut self) -> Result<BldStmt, SyntaxError> {
        let span = self.current_span();
        self.pos += 1; // `when`
        let cond = self.parse_expression()?;
        let then = self.parse_bld_body()?;
        let mut otherwise = None;
        if self.eat_identifier("else") {
            if self.check(&Token::When) {
                let nested = self.parse_bld_when()?;
                otherwise = Some(vec![nested]);
            } else {
                otherwise = Some(self.parse_bld_body()?);
            }
        }
        Ok(BldStmt::When { cond, then, otherwise, span })
    }

    /// `loop { … }` — unbounded spin. The body block is mandatory so the
    /// spin's extent is never ambiguous.
    fn parse_bld_loop(&mut self) -> Result<BldStmt, SyntaxError> {
        let span = self.current_span();
        self.pos += 1; // `loop`
        if !self.check(&Token::LBrace) {
            return Err(SyntaxError::InvalidStatement {
                reason: "`loop` needs a block: `loop { … }` — an unbounded spin's \
                         extent must be explicit"
                    .to_string(),
                span: self.current_span(),
            });
        }
        let body = self.parse_bld_body()?;
        Ok(BldStmt::Loop { body, span })
    }

    /// `while cond { … }`
    fn parse_bld_while(&mut self) -> Result<BldStmt, SyntaxError> {
        let span = self.current_span();
        self.pos += 1; // `while`
        let cond = self.parse_expression()?;
        if !self.check(&Token::LBrace) {
            return Err(SyntaxError::InvalidStatement {
                reason: "`while` needs a block: `while cond { … }`".to_string(),
                span: self.current_span(),
            });
        }
        let body = self.parse_bld_body()?;
        Ok(BldStmt::While { cond, body, span })
    }

    /// `bad { … }` — verbatim .bad passthrough. The text between the
    /// braces is sliced from the source (comments, strings, newlines
    /// preserved) and handed to the .bad parser at lowering.
    fn parse_bld_bad_block(&mut self) -> Result<BldStmt, SyntaxError> {
        let span = self.current_span();
        self.pos += 1; // `bad`
        self.expect(Token::LBrace)?;
        // Text starts at the first token inside the block.
        let start = self
            .peek_with_span()
            .map(|(_, r)| r.start)
            .unwrap_or(self.source.len());
        let mut depth: u64 = 1;
        let mut close: Option<std::ops::Range<usize>> = None;
        while let Some((tok, range)) = self.tokens.get(self.pos).cloned() {
            match tok {
                Token::LBrace => depth += 1,
                Token::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(range);
                        break;
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
        let Some(close) = close else {
            return Err(SyntaxError::UnexpectedEOF {
                expected: "`}` to close the `bad { … }` block".to_string(),
                span: self.current_span(),
            });
        };
        let text = self.source[start..close.start].to_string();
        self.pos += 1; // consume the closing `}`
        self.eat(&Token::Semicolon);
        Ok(BldStmt::Bad { text, span })
    }

    /// Expression statements: assignment (`=`, `+=`, `|=` …) or a call.
    /// Everything else is rejected HERE with a targeted message — a .bld
    /// statement must DO something observable.
    fn parse_bld_expr_stmt(&mut self) -> Result<BldStmt, SyntaxError> {
        let span = self.current_span();
        let lhs = self.parse_expression()?;

        // Compound assignment: `x <op>= rhs;` — the compound tokens stop
        // the expression parser (they are not expression operators).
        let compound = match self.peek() {
            Some(Token::PipeEq) => Some((BinaryOpKind::BitOr, "|=")),
            Some(Token::AmpEq) => Some((BinaryOpKind::BitAnd, "&=")),
            Some(Token::CaretEq) => Some((BinaryOpKind::BitXor, "^=")),
            Some(Token::ShlEq) => Some((BinaryOpKind::Shl, "<<=")),
            Some(Token::ShrEq) => Some((BinaryOpKind::Shr, ">>=")),
            Some(Token::PlusEq) => Some((BinaryOpKind::Add, "+=")),
            Some(Token::MinusEq) => Some((BinaryOpKind::Sub, "-=")),
            Some(Token::StarEq) => Some((BinaryOpKind::Mul, "*=")),
            Some(Token::SlashEq) => Some((BinaryOpKind::Div, "/=")),
            _ => None,
        };
        if let Some((op, label)) = compound {
            self.pos += 1;
            let rhs = self.parse_expression()?;
            self.expect(Token::Semicolon)?;
            ensure_assignable(&lhs, label, span)?;
            return Ok(BldStmt::Assign {
                target: lhs.clone(),
                value: Expr::BinaryOp(op, Box::new(lhs), Box::new(rhs)),
                span,
            });
        }

        self.expect(Token::Semicolon)?;
        // `x = rhs` arrives folded as BinaryOp(Eq, …) by the shared
        // expression parser (same convention as .bv's statement level).
        if let Expr::BinaryOp(BinaryOpKind::Eq, target, value) = lhs {
            ensure_assignable(&target, "=", span)?;
            return Ok(BldStmt::Assign { target: *target, value: *value, span });
        }
        match lhs {
            Expr::Call(callee, args, _) => Ok(BldStmt::Call { callee, args, span }),
            Expr::MethodCall(recv, name, args, _, _) => {
                // Method chains lower as a call on the receiver's result;
                // v1 keeps the surface: receiver call + method call.
                Ok(BldStmt::Call {
                    callee: format!("{}.{}", expr_head(&recv), name),
                    args,
                    span,
                })
            }
            Expr::Identifier(name) => Err(SyntaxError::InvalidStatement {
                reason: format!(
                    "`{name}` alone does nothing — call it (`{name}();`), assign it \
                     (`{name} = …;`), or use it inside an expression"
                ),
                span,
            }),
            _ => Err(SyntaxError::InvalidStatement {
                reason: "a .bld statement must be a call, an assignment, or a \
                         control-flow form (`if`/`loop`/`while`/`return`) — this \
                         expression has no effect on the machine"
                    .to_string(),
                span,
            }),
        }
    }

    fn current_span(&self) -> crate::errors::Span {
        self.peek_with_span()
            .map(|(_, r)| self.make_span(r.clone()))
            .unwrap_or_else(crate::errors::Span::dummy)
    }
}

/// Shared .bv keyword tokens that are NOT .bld statements — the
/// dialect-boundary rejection table: (token, word, fix). Data, not
/// scattered matches: adding a rejected keyword is one row (Rules 15/17).
/// Returns None when `tok` is an ordinary token.
fn bv_keyword_rejection(tok: &Token) -> Option<(&'static str, &'static str)> {
    static REJECTED: &[(Token, &str, &str)] = &[
        (Token::Halt, "halt", "`Halt();` (the PascalCase engine verb)"),
        (Token::Term, "term", "`return …;`"),
        (
            Token::Foreach,
            "foreach",
            "`loop { … }` / `while cond { … }` (unbounded — this tier does not \
             prove termination)",
        ),
        (Token::Match, "match", "`when`/`else` chains (no reactor dispatch in .bld)"),
        (Token::Trap, "trap", "`Halt();`"),
        (Token::Txn, "txn", "a `defn` (reactor constructs do not exist in .bld)"),
        (Token::Node, "node", "a `defn` (reactor constructs do not exist in .bld)"),
        (Token::BeginProgram, "beginprogram", "plain top-to-bottom recipe flow"),
        (Token::EndProgram, "endprogram", "plain top-to-bottom recipe flow"),
        (Token::Rollback, "rollback", "plain top-to-bottom recipe flow"),
        (Token::Spawn, "spawn", "a `defn` call or a `bad { … }` block"),
        (Token::Await, "await", "a `defn` call or a `bad { … }` block"),
        (Token::Mutex, "mutex", "a `defn` call or a `bad { … }` block"),
        (Token::Defer, "defer", "a `defn` call or a `bad { … }` block"),
        (Token::Trg, "trg", "a `defn` call or a `bad { … }` block"),
        (Token::Sync, "sync", "a `defn` call or a `bad { … }` block"),
    ];
    REJECTED.iter().find(|(t, ..)| t == tok).map(|(_, w, f)| (*w, *f))
}

/// Assignability check for `=` / compound targets.
fn ensure_assignable(target: &Expr, op: &str, span: crate::errors::Span) -> Result<(), SyntaxError> {
    let ok = matches!(
        target,
        Expr::Identifier(_) | Expr::Field(_, _) | Expr::Index(_, _)
    );
    if ok {
        return Ok(());
    }
    let what = match target {
        Expr::Call(..) => "a call result",
        Expr::BinaryOp(..) | Expr::UnaryOp(..) => "a computed expression",
        _ => "this expression",
    };
    Err(SyntaxError::InvalidStatement {
        reason: format!(
            "the left side of `{op}` ({what}) is not assignable — assign to a \
             variable, field, or element"
        ),
        span,
    })
}

/// A one-word description of an expression's head for diagnostics.
fn expr_head(e: &Expr) -> String {
    match e {
        Expr::Identifier(n) => n.clone(),
        Expr::Call(n, _, _) => n.clone(),
        other => format!("{other:?}"),
    }
}

fn describe_token(tok: Option<&Token>) -> String {
    match tok {
        Some(t) => format!("{t}"),
        None => "end of file".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> BldProgram {
        parse_bld(src).unwrap_or_else(|e| panic!("parse failed: {e}"))
    }

    fn parse_err(src: &str) -> String {
        parse_bld(src).expect_err("expected a parse error").to_string()
    }

    /// The flagship shape: a protected-mode boot recipe with engine
    /// intrinsics, raw `|=`, a polling loop, and a return.
    #[test]
    fn boot_recipe_parses() {
        let p = parse(
            r#"
            import "std/bad/arch.bad";
            const CR0_PE = 0x1;

            bootstrap Reset_Handler() {
                DisableInterrupts();
                let val = ReadControlReg(0);
                val |= CR0_PE;
                val = SetBit(val, 0);
                WriteControlReg(0, val);
                loop {
                    let status = ReadRegister(0x3fd);
                    when status & 0x20 != 0 {
                        break;
                    }
                }
                while val != 0 {
                    val -= 1;
                }
                Halt();
            }
            "#,
        );
        assert_eq!(p.items.len(), 3);
        let BldTopLevel::Bootstrap(bs) = &p.items[2] else {
            panic!("expected bootstrap")
        };
        assert_eq!(bs.name, "Reset_Handler");
        assert_eq!(bs.body.len(), 8);
        match &bs.body[2] {
            BldStmt::Assign { value, .. } => {
                assert!(matches!(value, Expr::BinaryOp(BinaryOpKind::BitOr, _, _)));
            }
            other => panic!("expected compound assign, got {other:?}"),
        }
        assert!(matches!(bs.body[5], BldStmt::Loop { .. }));
        assert!(matches!(bs.body[6], BldStmt::While { .. }));
        assert!(matches!(bs.body[7], BldStmt::Call { .. }));
    }

    #[test]
    fn import_const_defn_parse() {
        let p = parse(
            "import \"std/bad/arch.bad\";\n\
             const STACK_TOP = 0x90000;\n\
             defn uart_putc(c: Int) -> Int {\n\
                 PutChar#(c);\n\
                 return 1;\n\
             }\n",
        );
        assert!(matches!(p.items[0], BldTopLevel::Import(_)));
        assert!(matches!(p.items[1], BldTopLevel::Const(_)));
        let BldTopLevel::Defn(d) = &p.items[2] else {
            panic!("expected defn")
        };
        assert_eq!(d.name, "uart_putc");
        assert_eq!(d.params.len(), 1);
        assert!(d.ret.is_some());
        assert!(matches!(d.body[1], BldStmt::Return { .. }));
    }

    #[test]
    fn when_else_when_chain_parses() {
        let p = parse(
            "bootstrap B() {\n\
                 when a == 1 { x(); } else when a == 2 { y(); } else { z(); }\n\
             }\n",
        );
        let BldTopLevel::Bootstrap(bs) = &p.items[0] else {
            panic!("expected bootstrap")
        };
        let BldStmt::When { otherwise, .. } = &bs.body[0] else {
            panic!("expected when")
        };
        let inner = otherwise.as_ref().expect("else branch");
        assert!(matches!(inner[0], BldStmt::When { .. }), "else-when nests as When");
    }

    /// Briev has no `if` — a statement-head `if` gets the one honest
    /// spelling (`when`), never a silent reparse.
    #[test]
    fn if_statement_rejected_with_when_fix() {
        let err = parse_err("bootstrap B() { if x == 1 { y(); } }");
        assert!(err.contains("`when`"), "{err}");
    }

    #[test]
    fn break_continue_return_forms_parse() {
        let p = parse(
            "defn f() -> Int {\n\
                 loop { continue; }\n\
                 while true { break; }\n\
                 return;\n\
             }\n",
        );
        let BldTopLevel::Defn(d) = &p.items[0] else {
            panic!("expected defn")
        };
        assert!(matches!(d.body[0], BldStmt::Loop { .. }));
        assert!(matches!(d.body[1], BldStmt::While { .. }));
        assert!(matches!(d.body[2], BldStmt::Return { value: None, .. }));
    }

    /// The `bad { … }` block text is sliced VERBATIM — comments, newlines,
    /// and `.bad` grammar untouched for the .bad parser at lowering.
    #[test]
    fn bad_block_text_is_verbatim() {
        let p = parse(
            "bootstrap B() {\n\
                 bad {\n\
                     section .text\n\
                     cli                 // x86 disable interrupts\n\
                     mov r0, 1\n\
                 }\n\
                 Halt();\n\
             }\n",
        );
        let BldTopLevel::Bootstrap(bs) = &p.items[0] else {
            panic!("expected bootstrap")
        };
        let BldStmt::Bad { text, .. } = &bs.body[0] else {
            panic!("expected bad block, got {:?}", bs.body[0])
        };
        assert!(text.contains("section .text"), "{text:?}");
        assert!(text.contains("// x86 disable interrupts"), "{text:?}");
        assert!(text.contains("mov r0, 1"), "{text:?}");
        assert!(!text.contains("Halt"), "block ends at its closing brace");
    }

    /// A `bad { }` block inside a nested block still closes on the RIGHT
    /// brace (depth counting through tokens, strings are single tokens).
    #[test]
    fn bad_block_in_nested_block() {
        let p = parse(
            "defn f() {\n\
                 { bad { nop } }\n\
                 return;\n\
             }\n",
        );
        let BldTopLevel::Defn(d) = &p.items[0] else {
            panic!("expected defn")
        };
        let BldStmt::Block { body, .. } = &d.body[0] else {
            panic!("expected block")
        };
        assert!(matches!(body[0], BldStmt::Bad { .. }));
    }

    #[test]
    fn let_with_type_annotation_parses() {
        let p = parse("defn f() { let x: Int = 5; return; }");
        let BldTopLevel::Defn(d) = &p.items[0] else {
            panic!("expected defn")
        };
        let BldStmt::Let { name, ty, .. } = &d.body[0] else {
            panic!("expected let")
        };
        assert_eq!(name, "x");
        assert!(ty.is_some());
    }

    /// Every compound-assign token desugars to `target = target <op> rhs`
    /// with the target DUPLICATED (not moved).
    #[test]
    fn compound_bit_assign_desugars() {
        for (src, op) in [
            ("x |= 1;", BinaryOpKind::BitOr),
            ("x &= 1;", BinaryOpKind::BitAnd),
            ("x ^= 1;", BinaryOpKind::BitXor),
            ("x <<= 1;", BinaryOpKind::Shl),
            ("x >>= 1;", BinaryOpKind::Shr),
            ("x += 1;", BinaryOpKind::Add),
        ] {
            let p = parse(&format!("defn f() {{ {src} return; }}"));
            let BldTopLevel::Defn(d) = &p.items[0] else {
                panic!("expected defn")
            };
            let BldStmt::Assign { target, value, .. } = &d.body[0] else {
                panic!("expected assign for {src}");
            };
            assert!(matches!(target, Expr::Identifier(_)));
            assert!(
                matches!(value, Expr::BinaryOp(kind, lhs, _) if *kind == op
                    && matches!(**lhs, Expr::Identifier(_))),
                "{src} → {value:?}"
            );
        }
    }

    /// `halt;` is a shared .bv keyword token — .bld rejects it pointing at
    /// the one honest spelling (`Halt();`), never accepts both.
    #[test]
    fn halt_keyword_rejected_with_fix() {
        let err = parse_err("bootstrap B() { halt; }");
        assert!(err.contains("Halt()"), "{err}");
    }

    #[test]
    fn foreach_rejected_with_loop_fix() {
        let err = parse_err("bootstrap B() { foreach x in xs { } }");
        assert!(err.contains("loop"), "{err}");
    }

    #[test]
    fn term_rejected_with_return_fix() {
        let err = parse_err("defn f() { term 1; }");
        assert!(err.contains("return"), "{err}");
    }

    #[test]
    fn let_without_initializer_rejected() {
        let err = parse_err("defn f() { let x; return; }");
        assert!(err.contains("initializer"), "{err}");
    }

    #[test]
    fn unterminated_bad_block_is_loud() {
        let err = parse_err("bootstrap B() { bad { nop; }");
        assert!(err.contains("}"), "{err}");
    }

    #[test]
    fn stray_else_is_loud() {
        let err = parse_err("bootstrap B() { else { x(); } }");
        assert!(err.contains("stray `else`"), "{err}");
    }

    #[test]
    fn loop_without_block_is_loud() {
        let err = parse_err("bootstrap B() { loop x(); }");
        assert!(err.contains("loop { … }"), "{err}");
    }

    #[test]
    fn non_assignable_lhs_is_loud() {
        let err = parse_err("defn f() { foo() = 1; }");
        assert!(err.contains("not assignable"), "{err}");
    }

    #[test]
    fn bare_identifier_statement_is_loud() {
        let err = parse_err("defn f() { x; }");
        assert!(err.contains("does nothing"), "{err}");
    }

    #[test]
    fn pure_expression_statement_is_loud() {
        let err = parse_err("defn f() { 1 + 2; }");
        assert!(err.contains("no effect"), "{err}");
    }

    #[test]
    fn duplicate_bootstrap_is_loud() {
        let err = parse_err("bootstrap A() { } bootstrap B() { }");
        assert!(err.contains("at most ONE"), "{err}");
    }

    #[test]
    fn bootstrap_with_parameters_is_loud() {
        let err = parse_err("bootstrap Reset(x: Int) { }");
        assert!(err.contains("no parameters"), "{err}");
    }

    #[test]
    fn bootstrap_bad_spelling_rejected_with_bld_fix() {
        let err = parse_err("bootstrap bad Reset() { }");
        assert!(err.contains(".bv spelling"), "{err}");
    }

    #[test]
    fn unknown_top_level_names_item_set() {
        let err = parse_err("txn foo { }");
        assert!(err.contains(".bv keyword"), "{err}");
        let err2 = parse_err("widget x");
        assert!(err2.contains("bootstrap"), "{err2}");
    }

    #[test]
    fn bv_keyword_at_top_level_gets_dialect_note() {
        let err = parse_err("foreach x in xs { }");
        assert!(err.contains(".bv keyword"), "{err}");
    }

    #[test]
    fn contracts_rejected_on_defn() {
        let err = parse_err("defn f() [true] { }");
        assert!(err.contains("bad {"), "{err}");
    }

    #[test]
    fn statement_keywords_rejected_inside_body() {
        let err = parse_err("bootstrap B() { import \"x\"; }");
        assert!(err.contains("top-level"), "{err}");
    }

    #[test]
    fn context_keywords_work_as_ordinary_names_in_let() {
        // `if`/`loop` are contextual — legal as VALUES, reserved only at
        // statement heads (where `if` gets a `when` fix). A `let`
        // binding named `loop` parses.
        let p = parse("defn f() { let if = 1; let loop = if; return; }");
        let BldTopLevel::Defn(d) = &p.items[0] else {
            panic!("expected defn")
        };
        assert!(matches!(d.body[0], BldStmt::Let { .. }));
        assert!(matches!(d.body[1], BldStmt::Let { .. }));
    }
}
