// ── Statement Parser ───────────────────────────────────────────────────
// 2026-07-12: Phase 1.3 — Parse statement-level constructs.
// Flat code: each function is max 2 levels.
// Handles: let, assign, term, if, guard, foreach, trg, asm, sync, return, escape, metadata.

use super::helpers::Parser;
use crate::ast::{Expr, PropertyValue, Statement, TopLevel, Type};
use crate::errors::{Span, SyntaxError};
use crate::lexer::Token;

impl<'a> Parser<'a> {
    /// Parse a single statement.
    pub fn parse_statement(&mut self) -> Result<Statement, SyntaxError> {
        match self.peek() {
            Some(Token::Let) => self.parse_let_statement(),
            // 2026-09-22 (order-free-modifiers plan): `vol`/`out`/`mem`/
            // `reg` compose in any order before `let` (`vol let`,
            // `out vol let`, `mem reg let`). The shared scanner consumes the
            // run; the let is required and the annotations are recorded.
            Some(Token::Mem) | Some(Token::Reg) | Some(Token::Vol) | Some(Token::Out)
                if self.peek_targets_let() =>
            {
                let prefix = self.consume_modifier_prefix()?;
                let mut stmt = self.parse_let_statement()?;
                if let Statement::Let { modifiers, .. } = &mut stmt {
                    modifiers.extend(prefix.annotations);
                }
                Ok(stmt)
            }
            Some(Token::Term) => self.parse_term_statement(),
            Some(Token::Trap) => {
                self.pos += 1;
                self.eat(&Token::Semicolon);
                Ok(Statement::Trap)
            }
            // 2026-09-06 (halt slice): halt; — the bare-metal stop.
            Some(Token::Halt) => {
                self.pos += 1;
                self.eat(&Token::Semicolon);
                Ok(Statement::Halt)
            }
            Some(Token::EndProgram) => self.parse_endprogram_statement(),
            Some(Token::Rollback) => self.parse_rollback_statement(),
            Some(Token::Foreach) => self.parse_foreach_statement(),
            Some(Token::Break) => {
                self.pos += 1;
                self.eat(&Token::Semicolon);
                Ok(Statement::Break)
            }
            Some(Token::Trg) => self.parse_trg_binding(),
            Some(Token::Sync) => self.parse_sync_block(),
            Some(Token::Defer) => self.parse_defer_statement(),
            Some(Token::Mutex) => self.parse_mutex_statement(),
            // 2026-08-23 (F1 unified match dispatch): route ALL match to the
            // EXPRESSION form (parse_match_expr) and wrap in Statement::
            // Expression. The old parse_match_statement used block-body arms
            // with `;` separators — a different grammar that caused cascading
            // parse errors when used in defn bodies expecting expression-form
            // arms. SPEC §11.3 defines match as an expression.
            Some(Token::Match) => {
                self.advance(); // consume 'match'
                let expr = self.parse_match_expr()?;
                Ok(crate::ast::Statement::Expression(expr))
            }
            Some(Token::LBrace) => self.parse_block_statement(),
            Some(Token::LBracket) => self.parse_guard_statement_bracket(),
            Some(Token::When) => self.parse_guard_statement_when(),
            Some(Token::Semicolon) => {
                self.pos += 1;
                Ok(Statement::Expression(crate::ast::Expr::Decimal(0)))
            }
            // 2026-08-01 (Phase 3): Discard — `<- queue;` (read discard) /
            // `~<- queue;` (destructive discard). The `&` marker is removed.
            Some(Token::ArrowLeft) | Some(Token::TildeArrowLeft) => self.parse_arrow_discard_statement(),
            _ => {
                // 2026-07-24: Skip doc comments inside blocks too
                if let Some(&Token::DocComment(_) | &Token::DocCommentBang(_)) = self.peek() {
                    self.pos += 1;
                    return self.parse_statement();
                }
                // Keywords that lex as identifiers: if, $defn, $txn
                // 2026-08-22 (Phase 8, SPEC §12.2): `yield;` — cooperative
                // cancellation point. Contextual keyword; identifiers named
                // yield elsewhere are untouched.
                if self.check_identifier("yield") {
                    self.pos += 1;
                    self.expect(Token::Semicolon)?;
                    Ok(Statement::Yield)
                } else if self.check_identifier("check") {
                    // 2026-08-23 (SPEC §10.x): liveness check — assert expr
                    // holds at this point. Compile-time proven/rejected, or
                    // runtime assertion for unprovable loops.
                    self.pos += 1;
                    let cond = self.parse_expression()?;
                    self.expect(Token::Semicolon)?;
                    Ok(Statement::Check(cond))
                } else if self.check_identifier("return") {
                    let span = self
                        .peek_with_span()
                        .map(|(_, r)| self.make_span(r.clone()))
                        .unwrap_or_else(crate::errors::Span::dummy);
                    self.pos += 1; // consume `return`
                    return Err(SyntaxError::InvalidStatement {
                        reason: "Briev has no `return` statement. To return a value \
                                 from a defn use `term <value>`; to mark a convergence \
                                 checkpoint use bare `term;`; `term!` closes the program."
                            .to_string(),
                        span,
                    });
                } else if self.check_identifier("if") || self.check_identifier("else") {
                    // 2026-08-22 (spec-conformance plan Phase 1b): SPEC §11.1 —
                    // Briev has no `if`/`else`; branching is exhaustive `match`,
                    // one-sided execution is `when` or inline guards. The parser
                    // previously accepted full if/else trees (deviation D1);
                    // the if-statement AST variant and every consumer were
                    // excised alongside.
                    // Undo: restore parse_if_statement + this dispatch to it.
                    let span = self
                        .peek_with_span()
                        .map(|(_, r)| self.make_span(r.clone()))
                        .unwrap_or_else(crate::errors::Span::dummy);
                    Err(SyntaxError::InvalidStatement {
                        reason: "`if`/`else` do not exist in Briev. Use an exhaustive \
                                 `match cond { true => …, false => … };` for two-way \
                                 branching, or `when cond { … };` / `[cond] stmt;` for \
                                 one-sided guarded execution."
                            .to_string(),
                        span,
                    })
                } else if self.check_identifier("$defn") {
                    self.parse_inline_defn()
                } else if self.check(&Token::ExclaimArrow) {
                    self.parse_metadata_statement()
                } else if self.check_identifier("free") {
                    self.parse_lifetime_hint(true)
                } else if self.check_identifier("keep") {
                    self.parse_lifetime_hint(false)
                } else if self.check_identifier("open") {
                    // 2026-09-22 (D16 p3b): `open a.pin, b.pin;` — disconnection.
                    // Comma separates the two pins; parse_expression stops at `,`.
                    self.pos += 1; // consume 'open'
                    let lhs = self.parse_expression()?;
                    self.expect(Token::Comma)?;
                    let rhs = self.parse_expression()?;
                    self.expect(Token::Semicolon)?;
                    Ok(Statement::Open(Box::new(lhs), Box::new(rhs)))
                } else {
                    // 2026-08-22 (spec-conformance plan Phase 2): a
                    // declaration-shaped misspelled keyword (`nod ready { … }`)
                    // dies here as a generic expression error — intercept the
                    // Ident Ident shape and suggest. `Ident { … }` is left to
                    // fall through: it is a struct literal.
                    if let Some(Token::Identifier(name)) = self.peek() {
                        if matches!(
                            self.tokens.get(self.pos + 1).map(|(t, _)| t),
                            Some(Token::Identifier(_))
                        ) {
                            return self.error_unknown_item(name, "statement");
                        }
                    }
                    self.parse_expression_statement()
                }
            }
        }
    }

    /// let name: Type = expr;
    pub fn parse_let_statement(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1;
        // 2026-07-25: Tuple destructuring: let (a, b) = expr;
        // Also: let _ = expr; (discard binding)
        let names = if self.eat(&Token::LParen) {
            let mut names = Vec::new();
            while !self.check(&Token::RParen) {
                if !names.is_empty() {
                    self.expect(Token::Comma)?;
                }
                names.push(self.expect_identifier()?);
            }
            self.expect(Token::RParen)?;
            names
        } else {
            vec![self.expect_identifier()?]
        };
        let ty = self.parse_optional_type()?;
        let expr = if self.eat(&Token::Eq) {
            Some(self.parse_expression()?)
        } else {
            None
        };
        self.expect(Token::Semicolon)?;
        let first = names.first().cloned().unwrap_or_default();
        Ok(Statement::Let {
            name: first,
            names,
            ty,
            expr,
            modifiers: Vec::new(),
        })
    }

    /// E1 — bounded instance arrays: `let r_pu[2]: Resistor = Resistor { … };`,
    /// `let c[i:5]: Capacitor = Capacitor { … };`, multi-dim
    /// `let mesh[i:16][j:8]: Resistor = Resistor { … };`.
    ///
    /// Expands eagerly into ONE plain top-level `Statement::Let` per element,
    /// named with literal indices (`r_pu[0]`, `mesh[0][3]`) — the E11
    /// discipline: every downstream consumer (netlist derivation, emitter,
    /// contracts via `resolve_pin`) stays array-blind. The optional
    /// `name:` prefix inside a dimension (`[i:5]`) labels the index for
    /// generator tooling; E1 parses and discards it — population decisions
    /// live in generator programs, never the compiler (plan T1).
    ///
    /// Only component literals are accepted: an instance array exists to
    /// mass-instantiate parts. Scalar/aggregate bindings keep the type-side
    /// form `name: T[N]`. A total of 1 element yields the bare name
    /// (`let x[1]: T` → `x`), mirroring E11's single-pin rule.
    pub fn parse_let_array(&mut self) -> Result<Vec<TopLevel>, SyntaxError> {
        self.pos += 1; // `let`
        let base = self.expect_identifier()?;
        let dims = self.parse_instance_array_dims()?;
        let total: i64 = dims.iter().product();
        if total > 4096 {
            return self.error_at_current(&format!(
                "an instance array expands to {} elements (max 4096) — split the declaration",
                total
            ));
        }
        let (ty, expr) = self.parse_instance_array_init(&base)?;
        let elements = Self::expand_instance_names(&base, &dims, total)
            .into_iter()
            .map(|name| {
                TopLevel::Statement(Box::new(Statement::Let {
                    name: name.clone(),
                    names: vec![name],
                    ty: ty.clone(),
                    expr: Some(expr.clone()),
                    modifiers: Vec::new(),
                }))
            });
        Ok(elements.collect())
    }

    /// Parse one or more `[<binder>:]<extent>]` dimensions after an
    /// instance-array name; a zero/negative extent is an error.
    fn parse_instance_array_dims(&mut self) -> Result<Vec<i64>, SyntaxError> {
        let mut dims: Vec<i64> = Vec::new();
        while self.eat(&Token::LBracket) {
            // Optional index binder: `[i:5]` — `i:` labels the dimension.
            if matches!(self.peek(), Some(Token::Identifier(_)))
                && matches!(self.peek_next(), Some(Token::Colon))
            {
                self.pos += 1;
                self.expect(Token::Colon)?;
            }
            let n = self.expect_integer()?;
            if n < 1 {
                return self.error_at_current(&format!(
                    "instance-array dimension must have at least 1 element, got {}",
                    n
                ));
            }
            self.expect(Token::RBracket)?;
            dims.push(n);
        }
        Ok(dims)
    }

    /// Parse `: T = <expr>;` after an instance-array header; the
    /// initializer must be a component literal (mass-instantiation only).
    fn parse_instance_array_init(
        &mut self,
        base: &str,
    ) -> Result<(Option<Type>, Expr), SyntaxError> {
        let ty = self.parse_optional_type()?;
        let Some(expr) = (if self.eat(&Token::Eq) {
            Some(self.parse_expression()?)
        } else {
            None
        }) else {
            return self.error_at_current(&format!(
                "instance array '{}' needs a component literal: `let {}[N]: T = T {{ value: … }};`",
                base, base
            ));
        };
        if !matches!(expr, Expr::StructLiteral { .. }) {
            return self.error_at_current(&format!(
                "an instance array binds a component literal — the initializer of '{}' is not one; scalar arrays use `{}: T[N]`",
                base, base
            ));
        }
        self.expect(Token::Semicolon)?;
        Ok((ty, expr))
    }

    /// Expand instance-array dimensions into element names, row-major with
    /// the last dimension varying fastest. A total of 1 yields the bare
    /// name (E11's single-element rule mirrored).
    fn expand_instance_names(base: &str, dims: &[i64], total: i64) -> Vec<String> {
        let mut suffixes: Vec<String> = vec![String::new()];
        for &sz in dims {
            let mut next = Vec::with_capacity(suffixes.len() * sz as usize);
            for prefix in &suffixes {
                for k in 0..sz {
                    next.push(format!("{}[{}]", prefix, k));
                }
            }
            suffixes = next;
        }
        if total == 1 {
            return vec![base.to_string()];
        }
        suffixes.iter().map(|s| format!("{}{}", base, s)).collect()
    }

    /// term expr;
    fn parse_term_statement(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1;
        let val = if !self.check(&Token::Semicolon) && !self.check(&Token::RBrace) {
            Some(self.parse_expression()?)
        } else {
            None
        };
        // 2026-07-25: term expr? — conditional term (desugars to when-guard).
        // Check for ? AFTER the expression (before semicolon).
        if let Some(ref expr) = val {
            if let Expr::Exists(name) = expr {
                // term fn? → when fn? { term fn; };
                self.expect(Token::Semicolon)?;
                let call = Expr::Call(name.clone(), vec![], None);
                return Ok(Statement::Guarded(
                    expr.clone(),
                    vec![Statement::Term(Some(call))],
                ));
            }
        }
        self.expect(Token::Semicolon)?;
        Ok(Statement::Term(val))
    }

    /// endprogram; or endprogram code; — process boundary (SPEC §11.5).
    /// 2026-08-06: renamed from `exit program` (and, before that, `term!`).
    fn parse_endprogram_statement(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1; // consume 'endprogram'
        let val = if !self.check(&Token::Semicolon) {
            Some(self.parse_expression()?)
        } else {
            None
        };
        self.expect(Token::Semicolon)?;
        Ok(Statement::EndProgram(val))
    }

    /// rollback expr;
    fn parse_rollback_statement(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1;
        let val = if !self.check(&Token::Semicolon) {
            Some(self.parse_expression()?)
        } else {
            None
        };
        self.expect(Token::Semicolon)?;
        Ok(Statement::Rollback(val))
    }

    /// foreach(item in list) { ... }
    fn parse_foreach_statement(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1;
        // 2026-08-12 (Iterable protocol): the PARENLESS form `foreach x in
        // expr { ... }` is primary — the `in` keyword IS the binding; the
        // parens were redundant call-lookalike syntax (SPEC §11.4, and the
        // `()`-means-application delimiter rule). The `( item in list )` paren
        // form remains a tolerated legacy form during migration.
        let had_paren = self.eat(&Token::LParen);
        let item = self.expect_identifier()?;
        self.eat_identifier("in");
        let list = self.parse_expression()?;
        if had_paren {
            self.expect(Token::RParen)?;
        }
        let body = self.parse_block()?;
        // 2026-08-05 (Phase 2 canonical formatter): consume optional `;`
        // (matches when/match termination).
        self.eat(&Token::Semicolon);
        Ok(Statement::Foreach {
            item,
            list: Box::new(list),
            body,
        })
    }

    /// trg name @ instance.port;
    fn parse_trg_binding(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1;
        let name = self.expect_identifier()?;
        self.expect(Token::At)?;
        let instance = self.parse_expression()?;
        // 2026-08-01 (Phase 4): the `.port` is removed — whole-target form.
        self.expect(Token::Semicolon)?;
        Ok(Statement::TrgBinding {
            name,
            instance,
        })
    }

    /// `free x;` (free_hint = true) / `keep x;` (free_hint = false) — a
    /// lifetime-hint statement (2026-08-01, Phase 5).
    fn parse_lifetime_hint(&mut self, free_hint: bool) -> Result<Statement, SyntaxError> {
        self.pos += 1; // consume 'free' / 'keep'
        let name = self.expect_identifier()?;
        self.expect(Token::Semicolon)?;
        if free_hint {
            Ok(Statement::FreeHint(name))
        } else {
            Ok(Statement::KeepHint(name))
        }
    }

    /// sync { ... }
    fn parse_sync_block(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1;
        let body = self.parse_block()?;
        // 2026-08-05 (Phase 2 canonical formatter): consume optional `;`.
        self.eat(&Token::Semicolon);
        Ok(Statement::SyncBlock(body))
    }

    /// `defer { ... }` — cleanup registered for the current transaction;
    /// runs LIFO on `term`, `rollback`, and `endprogram` (2026-08-09, Phase 10).
    fn parse_defer_statement(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1;
        let body = self.parse_block()?;
        self.eat(&Token::Semicolon);
        Ok(Statement::Defer(body))
    }

    /// `mutex { ... }` — a serial section (2026-08-09, Phase 10).
    fn parse_mutex_statement(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1;
        let body = self.parse_block()?;
        self.eat(&Token::Semicolon);
        Ok(Statement::Mutex(body))
    }

    /// { stmt; stmt; ... }
    fn parse_block_statement(&mut self) -> Result<Statement, SyntaxError> {
        let stmts = self.parse_block()?;
        // 2026-08-05 (Phase 2 canonical formatter): consume optional `;`.
        self.eat(&Token::Semicolon);
        Ok(Statement::Block(stmts))
    }

    /// [condition]; — convergence gate (Statement::Gate)
    /// [condition] stmt; — prefix guarded single statement (Statement::Guarded)
    /// [condition] { body } — REJECTED (use `when condition { body }`)
    fn parse_guard_statement_bracket(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1; // consume '['
        let cond = self.parse_expression()?;
        self.expect(Token::RBracket)?;
        match self.peek() {
            Some(Token::LBrace) => {
                // 2026-07-26: Hard reject — block bodies require `when` keyword.
                // The bracket prefix [cond] is for gates ([cond];) and prefix
                // guarded single statements ([cond] stmt;) only.
                self.error_at_current(
                    "block bodies require `when` keyword: use `when expr { ... }` instead of `[expr] { ... }`"
                )
            }
            Some(Token::Semicolon) => {
                // [cond]; — standalone convergence gate
                self.pos += 1; // consume ';'
                Ok(Statement::Gate(cond))
            }
            _ => {
                // [cond] stmt; — prefix guarded single statement
                let stmt = self.parse_statement()?;
                Ok(Statement::Guarded(cond, vec![stmt]))
            }
        }
    }

    /// when condition { body }
    fn parse_guard_statement_when(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1; // consume 'when'
        let cond = self.parse_expression()?;
        let mut body = self.parse_block()?;
        // 2026-09-21 (D16 phase 2): trailing strategy clause —
        // `when cond { ... } thru <Name>;`. The selection desugars into a
        // marker statement the analysis reads; zero new Statement
        // variants. `thru` is a contextual identifier.
        // 2026-09-23 (D16 revision): `via` → `thru` — "via" is the settled
        // PCB term for a layer-jumping hole (our own .kicad_pcb routing
        // emits `(via ...)`); `thru` keeps the "passing through the
        // mechanism" meaning with no collision.
        if self.check_identifier("thru") {
            self.pos += 1;
            let name = self.expect_identifier()?;
            body.push(Statement::MetadataAssignment(
                "thru".to_string(),
                crate::ast::PropertyValue::Identifier(name),
            ));
        }
        // 2026-07-17: Same trailing semicolon fix as bracket guard.
        self.expect(Token::Semicolon)?;
        Ok(Statement::Guarded(cond, body))
    }

    /// !> key: value;
    fn parse_metadata_statement(&mut self) -> Result<Statement, SyntaxError> {
        self.expect(Token::ExclaimArrow)?;
        let key = self.expect_identifier()?;
        self.expect(Token::Colon)?;
        let val = self.parse_metadata_value_standalone()?;
        self.expect(Token::Semicolon)?;
        Ok(Statement::MetadataAssignment(key, val))
    }

    /// Parse a single metadata value (not wrapped in a HashMap loop).
    pub fn parse_metadata_value_standalone(&mut self) -> Result<PropertyValue, SyntaxError> {
        match self.peek() {
            Some(Token::Identifier(s)) => {
                let s = s.clone();
                self.pos += 1;
                // 2026-07-18: Property function call: ring_push(#Lh, #Rh).
                // Parse as List([Identifier("ring_push"), HashL, HashR]).
                if self.eat(&Token::LParen) {
                    let mut items = vec![PropertyValue::Identifier(s)];
                    if !self.check(&Token::RParen) {
                        loop {
                            items.push(self.parse_metadata_value_standalone()?);
                            if !self.eat(&Token::Comma) { break; }
                        }
                    }
                    self.expect(Token::RParen)?;
                    return Ok(PropertyValue::List(items));
                }
                Ok(PropertyValue::Identifier(s))
            }
            Some(Token::Integer(n)) => {
                let n = *n;
                self.pos += 1;
                Ok(PropertyValue::Int(n))
            }
            // 2026-09-11 (electrical proving): float metadata values —
            // `!> Tolerance: 3.3` (Electronics Briev pin ratings).
            Some(Token::Float(f)) => {
                let f = *f;
                self.pos += 1;
                Ok(PropertyValue::Float(f))
            }
            Some(Token::BoolTrue) => {
                self.pos += 1;
                Ok(PropertyValue::Bool(true))
            }
            Some(Token::BoolFalse) => {
                self.pos += 1;
                Ok(PropertyValue::Bool(false))
            }
            Some(Token::String(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(PropertyValue::String(s))
            }
            Some(Token::LBracket) => {
                self.pos += 1;
                let mut items = Vec::new();
                if !self.check(&Token::RBracket) {
                    loop {
                        items.push(self.parse_metadata_value_standalone()?);
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                }
                self.expect(Token::RBracket)?;
                Ok(PropertyValue::List(items))
            }
            // 2026-07-18: Hash-prefixed compiler words for strategy op bindings
            Some(Token::HashL) => { self.pos += 1; Ok(PropertyValue::HashL) }
            Some(Token::HashR) => { self.pos += 1; Ok(PropertyValue::HashR) }
            Some(Token::HashT) => { self.pos += 1; Ok(PropertyValue::HashT) }
            _ => self.error_at_current(
                "expected metadata value (identifier, int, bool, string, list, or #L/#R/#T)",
            ),
        }
    }

    /// Discard: `<- &queue;` — pop from collection, discard result.
    /// 2026-08-01 (Phase 3): leading arrow discard — `<- value;` (read
    /// discard) or `~<- value;` (destructive discard). The old `&` fake-pointer
    /// marker is removed: `<- &queue;` is now `<- queue;`.
    fn parse_arrow_discard_statement(&mut self) -> Result<Statement, SyntaxError> {
        let consume = if self.eat(&Token::TildeArrowLeft) {
            true
        } else {
            self.pos += 1; // consume '<-'
            false
        };
        let target = self.parse_expression()?;
        self.expect(Token::Semicolon)?;
        Ok(Statement::ArrowAssign {
            target: None,
            value: Box::new(target),
            consume,
        })
    }

    /// Fallback: parse as expression statement: expr;
    /// Also handles infix `<-` for push/pop: `&queue <- value` or `x <- &queue`.
    fn parse_expression_statement(&mut self) -> Result<Statement, SyntaxError> {
        let lhs = self.parse_expression()?;
            // 2026-08-01 (Phase 3): Arrow — `dest <- value` (copy into lhs),
            // `dest ~<- value` (destructive extract). The dispatch to
            // insert/read/extract is by the op binding on each side (done in
            // the typechecker/codegen); the parser records the ArrowAssign.
            if self.eat(&Token::ArrowLeft) {
                let rhs = self.parse_expression()?;
                self.expect(Token::Semicolon)?;
                return Ok(Statement::ArrowAssign {
                    target: Some(Box::new(lhs)),
                    value: Box::new(rhs),
                    consume: false,
                });
            }
            if self.eat(&Token::TildeArrowLeft) {
                let rhs = self.parse_expression()?;
                self.expect(Token::Semicolon)?;
                return Ok(Statement::ArrowAssign {
                    target: Some(Box::new(lhs)),
                    value: Box::new(rhs),
                    consume: true,
                });
            }
        // 2026-07-24: Compound assignment += and -=.
        // x += 1 → Statement::Assign(id("x"), BinaryOp(Add, id("x"), 1))
        let compound_kind = if self.eat(&Token::PlusEq) {
            Some(crate::ast::BinaryOpKind::Add)
        } else if self.eat(&Token::MinusEq) {
            Some(crate::ast::BinaryOpKind::Sub)
        } else if self.eat(&Token::StarEq) {
            Some(crate::ast::BinaryOpKind::Mul)
        } else if self.eat(&Token::SlashEq) {
            Some(crate::ast::BinaryOpKind::Div)
        } else {
            None
        };
        if let Some(op) = compound_kind {
            let rhs = self.parse_expression()?;
            self.expect(Token::Semicolon)?;
            let value = Expr::BinaryOp(op, Box::new(lhs.clone()), Box::new(rhs));
            return Ok(Statement::Assign(lhs, value));
        }
        self.expect(Token::Semicolon)?;
        // 2026-07-17: Detect assignment at statement level. The expression
        // parser treats both `=` and `==` as BinaryOpKind::Eq (lowest
        // precedence through parse_assignment). At the statement level,
        // `identifier = expr;` must produce Statement::Assign so the backend
        // knows to emit a store for the computed value. A standalone
        // equality check as an expression statement never appears in real
        // programs — the result would be discarded.
        if let Expr::BinaryOp(crate::ast::BinaryOpKind::Eq, lhs, rhs) = &lhs {
            let target: crate::ast::Expr = (**lhs).clone();
            let value: crate::ast::Expr = (**rhs).clone();
            return Ok(Statement::Assign(target, value));
        }
        Ok(Statement::Expression(lhs))
    }

    /// $defn name(params) -> Type { body } — compile-time-only definition.
    /// 2026-07-23: Only valid inside $(Stage) blocks.
    fn parse_inline_defn(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1;
        let defn = self.parse_definition()?;
        Ok(Statement::InlineDefn(defn))
    }

    /// match expr { pattern => body; pattern => body; };
    /// 2026-07-24: Compile-time pattern matching for $defn bodies.
    /// 2026-08-22 (spec-conformance plan Phase 4a): unified onto the rich
    /// expression-match pattern grammar — literals, strings, wildcards,
    /// typed bindings, tuples, ranges, enum variants, `|` alternatives.
    fn parse_match_statement(&mut self) -> Result<Statement, SyntaxError> {
        self.pos += 1; // consume 'match'
        let expr = Box::new(self.parse_expression()?);
        self.expect(Token::LBrace)?;

        let mut arms = Vec::new();
        while !self.check(&Token::RBrace) {
            // | -separated alternatives: 0x30 | 0x31 => body; — first wins.
            let mut patterns: Vec<crate::ast::Pattern> = Vec::new();
            loop {
                patterns.push(self.parse_pattern()?);
                if !self.eat(&Token::Pipe) {
                    break;
                }
            }

            self.expect(crate::lexer::Token::FatArrow)?;
            let body = self.parse_block()?;
            self.expect(Token::Semicolon)?;
            arms.push(crate::ast::StmtMatchArm { patterns, body });
        }
        self.pos += 1; // consume '}'
        self.expect(Token::Semicolon)?;
        Ok(Statement::Match { expr, arms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    // 2026-08-04 (remove-vestigial-return): Briev has no `return` statement —
    // using it must fail with a helpful error pointing at `term`, not parse to
    // a Statement::Return whose semantics disagreed across engines.
    #[test]
    fn return_statement_errors_with_helpful_message() {
        for src in ["defn f(x: Int) -> Int { return x; };", "return;", "node n [a==0][a==1] { a = 1; return; };"] {
            let tokens = tokenize(src).unwrap();
            let mut p = Parser::new(tokens, src);
            let err = p.parse_program().unwrap_err();
            assert!(
                err.to_string().contains("has no `return`"),
                "expected the helpful 'has no `return`' message, got: {err}"
            );
            assert!(
                err.to_string().contains("`term <value>`"),
                "message must suggest `term <value>`, got: {err}"
            );
        }
    }

    #[test]
    fn term_statements_still_parse() {
        let src = "defn f(x: Int) -> Int { term x; };\nnode n [a==0][a==1] { a = 1; term; };";
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        let program = p.parse_program().unwrap();
        assert_eq!(program.len(), 2, "both defn and node must parse");
    }

    // 2026-08-22 (spec-conformance plan Phase 1b): SPEC §11.1 — no `if`/`else`.
    // The parser previously accepted full if/else trees; every form must now
    // fail with the message naming `match` / `when` as the replacements.
    #[test]
    fn if_else_statements_are_rejected_with_guidance() {
        for src in [
            "defn f(x: Int) -> Int { if x > 0 { term 1; }; term 0; };",
            "defn g(x: Int) -> Int { if x > 0 { term 1; } else { term 0; }; };",
            "defn h(x: Int) -> Int { if x > 0 { term 1; } else if x < 0 { term 2; } else { term 3; }; };",
        ] {
            let tokens = tokenize(src).unwrap();
            let mut p = Parser::new(tokens, src);
            let err = p.parse_program().unwrap_err();
            assert!(
                err.to_string().contains("`if`/`else` do not exist"),
                "expected the no-if/else message, got: {err}"
            );
            assert!(
                err.to_string().contains("match") && err.to_string().contains("when"),
                "message must name both replacements, got: {err}"
            );
        }
    }

    // 2026-08-22 (spec-conformance plan Phase 2): SPEC §4.1 — misspelled
    // keywords get a suggested correction; reserved words are not usable
    // as names.
    #[test]
    fn misspelled_keyword_gets_suggestion() {
        let src = "nod ready [x < 3][x == 3] { x = x + 1; term; };";
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        let err = p.parse_program().unwrap_err();
        assert!(
            err.to_string().contains("did you mean `node`?"),
            "expected a node suggestion, got: {err}"
        );
    }

    #[test]
    fn reserved_words_are_rejected_as_names() {
        for (src, word) in [
            ("let sed: Int = 1;", "sed"),
            ("defn f(pvt: Int) -> Int { term pvt; };", "pvt"),
            ("let reg: Int = 2;", "reg"),
        ] {
            let tokens = tokenize(src).unwrap();
            let mut p = Parser::new(tokens, src);
            let err = p.parse_program().unwrap_err();
            assert!(
                err.to_string().contains(&format!("`{word}` is reserved")),
                "expected reserved-word rejection for {word}, got: {err}"
            );
        }
    }

    // One-sided `when` (the sanctioned conditional) still parses.
    // 2026-08-22 (spec-conformance plan Phase 4a): statement match carries
    // the unified rich pattern grammar — | alternatives, tuples, bools.
    #[test]
    fn statement_match_parses_rich_patterns() {
        // 2026-08-23 (F1 unified dispatch): match is always parsed via
        // parse_match_expr (expression form). Arms are comma-separated
        // expressions; block bodies use { ... } with tail expressions.
        let src = "$defn pick(v: Int) -> Int { match v { 1 | 2 => 10, _ => 0 } };";
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        assert!(p.parse_program().is_ok(), "int-or patterns must parse");

        let src2 = "defn pick(flag: Bool, other: Bool) -> Int { match flag { true => 1, _ => 0 } };";
        let tokens2 = tokenize(src2).unwrap();
        let mut p2 = Parser::new(tokens2, src2);
        let prog = p2.parse_program().unwrap();
        assert!(matches!(prog.first(), Some(crate::ast::TopLevel::Definition(_))), "bool-pattern match parses inside a defn");
    }

    #[test]
    fn when_statement_still_parses() {
        let src = "defn f(ready: Bool) -> Int { when ready { term 1; }; term 0; };";
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        assert!(p.parse_program().is_ok(), "`when` remains valid");
    }

    #[test]
    fn endprogram_statements_parse() {
        // 2026-08-06 (endprogram plan): `endprogram;` and `endprogram <expr>;`
        // are process-boundary statements (SPEC §11.5) — a single keyword.
        let src = "node n [a==0][a==1] { endprogram 5; };\nnode m [a==0][a==1] { endprogram; };";
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        let program = p.parse_program().unwrap();
        assert_eq!(program.len(), 2);
        let mut saw_value = false;
        let mut saw_bare = false;
        for item in &program {
            if let crate::ast::TopLevel::Transaction(t) = item {
                for s in &t.body {
                    if let Statement::EndProgram(v) = s {
                        if v.is_some() {
                            saw_value = true;
                        } else {
                            saw_bare = true;
                        }
                    }
                }
            }
        }
        assert!(saw_value, "endprogram 5; must parse as EndProgram(Some)");
        assert!(saw_bare, "endprogram; must parse as EndProgram(None)");
    }

    // ── 2026-08-09 (Phase 10): defer/mutex statements ─────────────────

    #[test]
    fn defer_mutex_parse() {
        let src = "defn f(x: Int) -> Int {
            defer { term 1; };
            mutex { let y: Int = x; };
            term x;
        };";
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        let program = p.parse_program().unwrap();
        let body = match &program[0] {
            crate::ast::TopLevel::Definition(d) => &d.body,
            _ => panic!("expected defn"),
        };
        let mut saw_defer = false;
        let mut saw_mutex = false;
        for s in body {
            match s {
                Statement::Defer(b) => {
                    saw_defer = true;
                    assert_eq!(b.len(), 1);
                }
                Statement::Mutex(b) => {
                    saw_mutex = true;
                    assert_eq!(b.len(), 1);
                }
                _ => {}
            }
        }
        assert!(saw_defer, "defer block must parse");
        assert!(saw_mutex, "mutex block must parse");
    }

    #[test]
    fn test_halt_statement_parses_and_round_trips() {
        // 2026-09-06 (halt slice): `halt;` parses to Statement::Halt and
        // round-trips through the canonical printer (trap's sibling).
        let tokens = crate::lexer::tokenize("defn f() -> Int { halt; term 1; };").unwrap();
        let mut p = crate::parser::Parser::new(tokens, "");
        let items = p.parse_program().unwrap();
        let crate::ast::TopLevel::Definition(d) = &items[0] else { panic!("expected Definition") };
        assert!(matches!(d.body[0], crate::ast::Statement::Halt), "halt; must parse to Statement::Halt");
        let printed = crate::ast::format_item(&items[0]);
        assert!(printed.contains("halt;"), "canonical form must round-trip, got: {printed}");
    }

    // ── 2026-09-23 (E1): bounded instance arrays ────────────────────────

    /// Parse `src` and collect the top-level `Statement::Let` names in order.
    fn let_names(src: &str) -> Vec<String> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        let items = p.parse_program().unwrap();
        items
            .iter()
            .filter_map(|i| match i {
                crate::ast::TopLevel::Statement(s) => match s.as_ref() {
                    Statement::Let { name, .. } => Some(name.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn instance_array_expands_row_major() {
        let src = r#"let r[3]: Resistor = Resistor { value: "10k" };"#;
        assert_eq!(let_names(src), ["r[0]", "r[1]", "r[2]"]);
    }

    #[test]
    fn instance_array_multi_dim_with_index_binder() {
        let src = r#"let m[i:2][j:3]: Mesh = Mesh { value: "x" };"#;
        assert_eq!(
            let_names(src),
            ["m[0][0]", "m[0][1]", "m[0][2]", "m[1][0]", "m[1][1]", "m[1][2]"]
        );
    }

    #[test]
    fn instance_array_single_element_keeps_bare_name() {
        let src = r#"let x[1]: T = T { value: "y" };"#;
        assert_eq!(let_names(src), ["x"]);
    }

    #[test]
    fn instance_array_zero_dim_is_an_error() {
        let src = r#"let r[0]: T = T { value: "y" };"#;
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        let err = p.parse_program().unwrap_err();
        let msg = format!("{:?}", err);
        assert!(msg.contains("at least 1 element"), "{}", msg);
    }

    #[test]
    fn instance_array_requires_a_component_literal() {
        for src in [
            r#"let r[2]: Int = 5;"#,
            r#"let r[2]: Int;"#,
            r#"let r[2] = [1, 2];"#,
        ] {
            let tokens = crate::lexer::tokenize(src).unwrap();
            let mut p = Parser::new(tokens, src);
            let err = p.parse_program().unwrap_err();
            let msg = format!("{:?}", err);
            assert!(msg.contains("component literal"), "for `{}`: {}", src, msg);
        }
    }

    #[test]
    fn plain_and_tuple_lets_bypass_the_array_lookahead() {
        assert_eq!(let_names("let x: Int = 1;"), ["x"]);
        let tokens = crate::lexer::tokenize("let (a, b) = pair;").unwrap();
        let mut p = Parser::new(tokens, "let (a, b) = pair;");
        assert!(p.parse_program().is_ok(), "tuple let must not take the array path");
    }
}
