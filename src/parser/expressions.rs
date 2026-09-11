// ── Expression Parser ──────────────────────────────────────────────────
// 2026-07-12: Phase 1.1 — Parse all expression forms.
// Flat code: each function is max 2 levels, nested logic extracted to helpers.
// No IntrinsicCall — Sqrt#(x) is Call("Sqrt#", [x]).
// @ prefix forces any token to Quoted(bytes).

use super::helpers::Parser;
use crate::ast::{BinaryOpKind, Expr, ReflectKind, SpawnStorage, UnaryOpKind};
use crate::errors::{Span, SyntaxError};
use crate::lexer::Token;

impl<'a> Parser<'a> {
    /// Entry point: parse an expression at any precedence level.
    pub fn parse_expression(&mut self) -> Result<Expr, SyntaxError> {
        self.parse_assignment()
    }

    /// Assignment: a = b  (lowest precedence)
    fn parse_assignment(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_or()?;
        if self.eat(&Token::Eq) {
            let value = self.parse_assignment()?;
            expr = Expr::BinaryOp(BinaryOpKind::Eq, Box::new(expr), Box::new(value));
        } else if self.eat(&Token::TildeEq) {
            // 2026-08-01 (Phase 3): `a ~= b` — assign, then consume b.
            let value = self.parse_assignment()?;
            expr = Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(expr),
                Box::new(Expr::Consume(Box::new(value))),
            );
        }
        Ok(expr)
    }

    /// Logical OR: a || b
    fn parse_or(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_and()?;
        while self.eat(&Token::OrOr) {
            let rhs = self.parse_and()?;
            expr = Expr::BinaryOp(BinaryOpKind::Or, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    /// Logical AND: a && b
    fn parse_and(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_equality()?;
        while self.eat(&Token::AndAnd) {
            let rhs = self.parse_equality()?;
            expr = Expr::BinaryOp(BinaryOpKind::And, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    /// Equality: a == b, a != b
    fn parse_equality(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_comparison()?;
        loop {
            if self.eat(&Token::EqEq) {
                let rhs = self.parse_comparison()?;
                expr = Expr::BinaryOp(BinaryOpKind::Eq, Box::new(expr), Box::new(rhs));
            } else if self.eat(&Token::Ne) {
                let rhs = self.parse_comparison()?;
                expr = Expr::BinaryOp(BinaryOpKind::Neq, Box::new(expr), Box::new(rhs));
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// Comparison: a < b, a > b, a <= b, a >= b
    fn parse_comparison(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_bitor()?;
        loop {
            if self.eat(&Token::Lt) {
                let rhs = self.parse_bitor()?;
                expr = Expr::BinaryOp(BinaryOpKind::Lt, Box::new(expr), Box::new(rhs));
            } else if self.eat(&Token::Gt) {
                let rhs = self.parse_bitor()?;
                expr = Expr::BinaryOp(BinaryOpKind::Gt, Box::new(expr), Box::new(rhs));
            } else if self.eat(&Token::Le) {
                let rhs = self.parse_bitor()?;
                expr = Expr::BinaryOp(BinaryOpKind::Le, Box::new(expr), Box::new(rhs));
            } else if self.eat(&Token::Ge) {
                let rhs = self.parse_bitor()?;
                expr = Expr::BinaryOp(BinaryOpKind::Ge, Box::new(expr), Box::new(rhs));
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// 2026-07-18: Bitwise OR: a | b
    fn parse_bitor(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_bitxor()?;
        while self.eat(&Token::Pipe) {
            let rhs = self.parse_bitxor()?;
            expr = Expr::BinaryOp(BinaryOpKind::BitOr, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    /// 2026-07-18: Bitwise XOR: a ^ b
    fn parse_bitxor(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_bitand()?;
        while self.eat(&Token::BitXor) {
            let rhs = self.parse_bitand()?;
            expr = Expr::BinaryOp(BinaryOpKind::BitXor, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    /// 2026-07-18: Bitwise AND: a & b
    /// Note: `&` is also used as unary address-of in parse_unary.
    /// In binary position (between expressions) it's bitwise AND,
    /// in prefix position it's address-of. No ambiguity because
    /// binary `&` only matches after a left-hand expression.
    fn parse_bitand(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_shift()?;
        while self.eat(&Token::Ampersand) {
            let rhs = self.parse_shift()?;
            expr = Expr::BinaryOp(BinaryOpKind::BitAnd, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    /// 2026-07-18: Shift: a << b, a >> b
    fn parse_shift(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_term()?;
        while self.eat(&Token::Shl) {
            let rhs = self.parse_term()?;
            expr = Expr::BinaryOp(BinaryOpKind::Shl, Box::new(expr), Box::new(rhs));
        }
        while self.eat(&Token::Shr) {
            let rhs = self.parse_term()?;
            expr = Expr::BinaryOp(BinaryOpKind::Shr, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    /// Term: a + b, a - b
    fn parse_term(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_factor()?;
        loop {
            if self.eat(&Token::Plus) {
                let rhs = self.parse_factor()?;
                expr = Expr::BinaryOp(BinaryOpKind::Add, Box::new(expr), Box::new(rhs));
            } else if self.eat(&Token::Minus) {
                let rhs = self.parse_factor()?;
                expr = Expr::BinaryOp(BinaryOpKind::Sub, Box::new(expr), Box::new(rhs));
            } else if self.eat(&Token::TildePlus) {
                // 2026-08-01 (Phase 3): `a ~+ b` — add, then consume b.
                let rhs = self.parse_factor()?;
                expr = Expr::BinaryOp(BinaryOpKind::Add, Box::new(expr), Box::new(Expr::Consume(Box::new(rhs))));
            } else if self.eat(&Token::TildeMinus) {
                // 2026-08-01 (Phase 3): `a ~- b` — subtract, then consume b.
                let rhs = self.parse_factor()?;
                expr = Expr::BinaryOp(BinaryOpKind::Sub, Box::new(expr), Box::new(Expr::Consume(Box::new(rhs))));
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// Factor: a * b, a / b, a % b
    fn parse_factor(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_unary()?;
        loop {
            if self.eat(&Token::Star) {
                let rhs = self.parse_unary()?;
                expr = Expr::BinaryOp(BinaryOpKind::Mul, Box::new(expr), Box::new(rhs));
            } else if self.eat(&Token::Slash) {
                let rhs = self.parse_unary()?;
                expr = Expr::BinaryOp(BinaryOpKind::Div, Box::new(expr), Box::new(rhs));
            } else if self.eat(&Token::Percent) {
                let rhs = self.parse_unary()?;
                expr = Expr::BinaryOp(BinaryOpKind::Mod, Box::new(expr), Box::new(rhs));
            } else if self.eat(&Token::TildeStar) {
                // 2026-08-01 (Phase 3): `a ~* b` — multiply, then consume b.
                let rhs = self.parse_unary()?;
                expr = Expr::BinaryOp(BinaryOpKind::Mul, Box::new(expr), Box::new(Expr::Consume(Box::new(rhs))));
            } else if self.eat(&Token::TildeSlash) {
                // 2026-08-01 (Phase 3): `a ~/ b` — divide, then consume b.
                // (TildeSlash was the dead term-until token; now consumptive /.)
                let rhs = self.parse_unary()?;
                expr = Expr::BinaryOp(BinaryOpKind::Div, Box::new(expr), Box::new(Expr::Consume(Box::new(rhs))));
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// Unary: !a, -a, ~a, &a, *a (deref)
    fn parse_unary(&mut self) -> Result<Expr, SyntaxError> {
        if self.eat(&Token::Not) {
            let expr = self.parse_unary()?;
            return Ok(Expr::UnaryOp(UnaryOpKind::Not, Box::new(expr)));
        }
        if self.eat(&Token::Minus) {
            let expr = self.parse_unary()?;
            return Ok(Expr::UnaryOp(UnaryOpKind::Neg, Box::new(expr)));
        }
        if self.eat(&Token::Tilde) {
            let expr = self.parse_unary()?;
            return Ok(Expr::UnaryOp(UnaryOpKind::BitNot, Box::new(expr)));
        }
        // 2026-08-09 (Phase 10): `await task` — consume a task handle.
        if self.eat(&Token::Await) {
            let expr = self.parse_unary()?;
            return Ok(Expr::Await(Box::new(expr)));
        }
        // 2026-07-15: Unary * for pointer dereference. Higher precedence than
        // binary * (multiplication) since it's in parse_unary.
        if self.eat(&Token::Star) {
            let expr = self.parse_unary()?;
            return Ok(Expr::Deref(Box::new(expr)));
        }
        // 2026-07-17: Unary & for address-of. Used by <- arrow syntax to mark
        // collection targets for push/pop/discard.
        if self.eat(&Token::Ampersand) {
            let expr = self.parse_unary()?;
            return Ok(Expr::AddrOf(Box::new(expr)));
        }
        self.parse_as()
    }

    /// Type cast: expr as Type. Tighter than unary but looser than postfix.
    /// 2026-07-15: Unblocks volatile-io.bv, target-import.bv, etc.
    fn parse_as(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_postfix()?;
        if self.eat(&Token::As) {
            let ty = self.parse_type()?;
            expr = Expr::Cast(Box::new(expr), ty);
        }
        Ok(expr)
    }

    /// Postfix: a[b], a.f, a(args), a within { }

    /// 2026-08-22 (Phase 6b): multi-dimensional selectors exceed the flat
    /// Slice AST. Name the ellipsis form when one follows the comma.
    fn reject_multidim<T>(&mut self) -> Result<T, SyntaxError> {
        let mut probe = self.pos;
        let mut sees_ellipsis = false;
        while probe < self.tokens.len() {
            match &self.tokens[probe].0 {
                Token::RBracket => break,
                Token::Ellipsis => { sees_ellipsis = true; break; }
                _ => probe += 1,
            }
        }
        if sees_ellipsis {
            self.error_staged("multi-dimensional ellipsis slicing (`t[1:3, ...]`)")
        } else {
            self.error_staged("multi-dimensional indexing (`t[i, j]`)")
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, SyntaxError> {
        let mut expr = self.parse_primary()?;
        loop {
            // 2026-08-07 (Phase 7): iterable ranges — `a..b` (half-open) /
            // `a..=b` (inclusive), consumed by `foreach` (SPEC §11.4).
            if self.eat(&Token::DotDot) {
                let end = self.parse_postfix()?;
                expr = Expr::Range { start: Box::new(expr), end: Box::new(end), inclusive: false };
            } else if self.eat(&Token::DotDotEq) {
                let end = self.parse_postfix()?;
                expr = Expr::Range { start: Box::new(expr), end: Box::new(end), inclusive: true };
            } else if self.eat(&Token::LParen) {
                // Call: f(args)
                let mut args = Vec::new();
                if !self.check(&Token::RParen) {
                    loop {
                        args.push(self.parse_expression()?);
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                }
                self.expect(Token::RParen)?;
                // Extract function name if primary is an identifier
                match expr {
                    Expr::Identifier(name) => {
                        expr = Expr::Call(name, args, None);
                    }
                    _ => {
                        return self.error_at_current("only named functions can be called");
                    }
                }
            } else if self.check(&Token::ColonColon) {
                // 2026-08-26 (qualified enum paths): `Enum::Variant` and
                // `Enum::Variant(args)` — construction via the qualified
                // path. Desugars to a Call whose name carries the `::`
                // (function names never contain `::`, so registries
                // disambiguate cleanly). Bare `Enum::Variant` (no parens)
                // is the zero-arg call form — SPEC §8.3 unit variants.
                let Expr::Identifier(enum_name) = &expr else {
                    return self.error_at_current("expected enum name before '::'");
                };
                let enum_name = enum_name.clone();
                self.pos += 1; // consume '::'
                let variant = self.expect_identifier()?;
                let qualified = format!("{}::{}", enum_name, variant);
                if self.eat(&Token::LParen) {
                    let mut args = Vec::new();
                    if !self.check(&Token::RParen) {
                        loop {
                            args.push(self.parse_expression()?);
                            if !self.eat(&Token::Comma) {
                                break;
                            }
                        }
                    }
                    self.expect(Token::RParen)?;
                    expr = Expr::Call(qualified, args, None);
                } else {
                    expr = Expr::Call(qualified, Vec::new(), None);
                }
            } else if self.eat(&Token::Dot) {
                // Field access: a.f — the receiver is PRESERVED.
                // 2026-08-17 (tuple correctness, plan
                // 2026-08-17-hashmap-storage-tuple-correctness.md): a NUMERIC
                // field name `t.0`/`t.1` is a tuple ELEMENT access (the
                // typechecker's resolve_field_type already supports numeric
                // field names; the parser rejected them with expect_identifier).
                // `a.1$(args)` navigation chains are identifier-only; a numeric
                // name is always an element read.
                let name = if self.peek().is_some_and(|t| matches!(t, Token::Integer(_))) {
                    match self.advance() {
                        Some((Token::Integer(n), span)) => n.to_string(),
                        Some((tok, span)) => {
                            return Err(crate::errors::SyntaxError::UnexpectedToken {
                                expected: "an integer field name after '.'".into(),
                                found: format!("{}", tok),
                                span: self.make_span(span),
                            });
                        }
                        None => return Err(crate::errors::SyntaxError::UnexpectedEOF {
                            expected: "an integer field name after '.'".into(),
                            span: crate::errors::Span::dummy(),
                        }),
                    }
                } else {
                    self.expect_identifier()?
                };
                // 2026-07-21: Navigation chain call: a.first$(args).
                if name.ends_with('$') && self.check(&Token::LParen) {
                    self.expect(Token::LParen)?;
                    let mut args = vec![expr];
                    if !self.check(&Token::RParen) {
                        loop {
                            args.push(self.parse_expression()?);
                            if !self.eat(&Token::Comma) { break; }
                        }
                    }
                    self.expect(Token::RParen)?;
                    expr = Expr::Call(name, args, None);
                } else if self.check(&Token::LParen) {
                    // 2026-07-31: Method call: a.f(x) — receiver preserved.
                    self.expect(Token::LParen)?;
                    let mut args = Vec::new();
                    if !self.check(&Token::RParen) {
                        loop {
                            args.push(self.parse_expression()?);
                            if !self.eat(&Token::Comma) { break; }
                        }
                    }
                    self.expect(Token::RParen)?;
                    expr = Expr::MethodCall(Box::new(expr), name, args, None);
                } else {
                    expr = Expr::Field(Box::new(expr), name);
                }
            } else if self.eat(&Token::DotCaretCaret) {
                // 2026-07-31: Compile-time reflection: a.^^Size → foldable constant.
                let name = self.expect_identifier()?;
                expr = Expr::Reflect(Box::new(expr), name, ReflectKind::CompileTime);
            } else if self.eat(&Token::DotCaret) {
                // 2026-07-31: Runtime reflection: a.^Length, a.^Ptr.
                let name = self.expect_identifier()?;
                expr = Expr::Reflect(Box::new(expr), name, ReflectKind::Runtime);
            } else if self.eat(&Token::LBracket) {
                // 2026-08-22 (spec-conformance plan Phase 6b, SPEC §16.5):
                // `a[...]` — the FULL-RANGE ellipsis. Single-dimension it is
                // exactly `a[:]` (whole copy). A comma after any selector
                // means multi-dimensional indexing, which the flat Slice
                // AST cannot represent: staged error naming the fix.
                if self.eat(&Token::Ellipsis) {
                    if self.eat(&Token::Comma) {
                        return self.reject_multidim();
                    }
                    self.expect(Token::RBracket)?;
                    expr = Expr::Slice { array: Box::new(expr), start: None, end: None, stride: None };
                    continue;
                }
                // 2026-08-07 (Phase 7): NAMED selectors — `arr[name => sel]`
                // (SPEC §16.5). For a 1-D array the name LABELS the single
                // dimension; it lowers to the plain slice/index. The name is
                // validated as an identifier here; cross-checking it against a
                // declared dimension name is a follow-up once named dims
                // (const generics, §16.6) exist.
                let _named_dim = if matches!(self.peek(), Some(Token::Identifier(_)))
                    && matches!(self.peek_next(), Some(&Token::FatArrow))
                {
                    self.pos += 1; // consume the name
                    self.pos += 1; // consume `=>`
                    true
                } else {
                    false
                };
                // Check for slice syntax: arr[start:end:stride]
                if self.check(&Token::Colon) {
                    // Slice with implicit start: arr[:end] or arr[:]
                    self.pos += 1; // consume ':'
                    let end = if self.check(&Token::RBracket) || self.check(&Token::Colon) {
                        None
                    } else {
                        Some(Box::new(self.parse_expression()?))
                    };
                    let stride = if self.eat(&Token::Colon) {
                        if !self.check(&Token::RBracket) {
                            Some(Box::new(self.parse_expression()?))
                        } else { None }
                    } else { None };
                    if self.eat(&Token::Comma) {
                        return self.reject_multidim();
                    }
                    self.expect(Token::RBracket)?;
                    expr = Expr::Slice { array: Box::new(expr), start: None, end, stride };
                } else {
                    // Index or slice with start
                    let first = self.parse_expression()?;
                    if self.eat(&Token::Colon) {
                        // It's a slice: arr[start:end] or arr[start:] or arr[start:end:stride]
                        let end = if self.check(&Token::RBracket) || self.check(&Token::Colon) {
                            None
                        } else {
                            Some(Box::new(self.parse_expression()?))
                        };
                        let stride = if self.eat(&Token::Colon) {
                            if !self.check(&Token::RBracket) {
                                Some(Box::new(self.parse_expression()?))
                            } else { None }
                        } else { None };
                        if self.eat(&Token::Comma) {
                            return self.reject_multidim();
                        }
                        self.expect(Token::RBracket)?;
                        expr = Expr::Slice { array: Box::new(expr), start: Some(Box::new(first)), end, stride };
                    } else {
                        // Simple index: arr[idx]
                        if self.eat(&Token::Comma) {
                            return self.reject_multidim();
                        }
                        self.expect(Token::RBracket)?;
                        expr = Expr::Index(Box::new(expr), Box::new(first));
                    }
                }
            } else if self.eat(&Token::Not) {
                // 2026-07-19: Plugin-intercept: name!(args)
                // ! after an expression is the plugin-intercept marker.
                if !self.eat(&Token::LParen) {
                    return self.error_at_current("expected '(' after '!' for plugin-intercept call");
                }
                let p_name = match &expr {
                    Expr::Identifier(n) => n.clone(),
                    _ => return self.error_at_current("only named functions can be plugin-intercepted"),
                };
                let mut p_args = Vec::new();
                if !self.check(&Token::RParen) {
                    loop {
                        p_args.push(self.parse_expression()?);
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                }
                self.expect(Token::RParen)?;
                expr = Expr::PluginIntercept { name: p_name, args: p_args, type_args: vec![] };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// Primary: literals, identifiers, parenthesized, blocks, if/match/lambda
    /// 2026-07-27: After parsing a literal token at `end_pos`, check if the
    /// next token is an adjacent identifier (no whitespace). If so, it's a
    /// suffix discriminator (e.g., `f` in `3.14f`, `km` in `42km`).
    /// Returns the suffix string if found, advancing past the suffix token.
    fn peek_suffix(&mut self, end_pos: usize) -> Option<String> {
        // self.pos is already past the literal token (advance() incremented it).
        let next_idx = self.pos;
        if next_idx >= self.tokens.len() { return None; }
        let (next_tok, next_span) = &self.tokens[next_idx];
        // Check adjacency: next token starts right where current ends
        if next_span.start != end_pos { return None; }
        match next_tok {
            Token::Identifier(s) => {
                // Consume the suffix token (skip past it)
                self.pos = next_idx + 1;
                Some(s.clone())
            }
            _ => None,
        }
    }

    /// 2026-08-09 (Phase 5): parse `spawn Obj(args)` with a given storage
    /// class. The caller has ALREADY consumed the `spawn` token (pos is at the
    /// type name). `box`/`spill` keywords were consumed by the caller too.
    fn parse_spawn_body(&mut self, storage: SpawnStorage) -> Result<Expr, SyntaxError> {
        let type_name = self.expect_identifier()?;
        self.expect(Token::LParen)?;
        let mut args = Vec::new();
        if !self.check(&Token::RParen) {
            loop {
                args.push(self.parse_expression()?);
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
        }
        self.expect(Token::RParen)?;
        Ok(Expr::Spawn { type_name, args, storage })
    }

    fn parse_primary(&mut self) -> Result<Expr, SyntaxError> {
        match self.advance() {            // ── Literals ────────────────────────────────────────────
            Some((Token::Integer(n), span)) => {
                // 2026-07-27: Check for adjacent suffix identifier (e.g., 42km, 0xFFh)
                if let Some(suf) = self.peek_suffix(span.end) {
                    Ok(Expr::TaggedLiteral(n, suf))
                } else {
                    Ok(Expr::Decimal(n))
                }
            }
            Some((Token::Float(f), span)) => {
                // 2026-07-27: Check for adjacent suffix identifier (e.g., 3.14f, 16.2bf)
                if let Some(suf) = self.peek_suffix(span.end) {
                    Ok(Expr::TaggedLiteral(f as i64, suf))
                } else {
                    Ok(Expr::Float(f))
                }
            }
            Some((Token::String(s), _)) => Ok(Expr::Quoted(s.into_bytes())),
            Some((Token::RawString(s), _)) => Ok(Expr::Quoted(s.into_bytes())),
            Some((Token::ByteString(s), _)) => {
                // 2026-08-06 (Phase 7): `#b"..."` is a Blob byte literal (SPEC
                // 16.2). Tagged with prefix "b" so the typechecker types it as
                // Data, not String.
                Ok(Expr::TaggedQuotedLiteral(s, "b".to_string()))
            }
            Some((Token::Char(c), _)) => Ok(Expr::Char(c)),
            Some((Token::BoolTrue, _)) => Ok(Expr::Bool(true)),
            Some((Token::BoolFalse, _)) => Ok(Expr::Bool(false)),
            // 2026-08-06 (beginprogram plan): the `beginprogram` precondition
            // marker — true exactly once at program start (SPEC entry-loop).
            Some((Token::BeginProgram, _)) => Ok(Expr::BeginProgram),

            // 2026-08-05 (Phase 3): the `@` raw-literal prefix is removed.
            // Raw/byte literals are `#r`/`#b` (SPEC §16.2). `@` in expression
            // position is no longer an expression start; prior-state `@`
            // references are a staged feature and will be implemented with
            // explicit syntax in a later phase.

            // ── Identifiers (including # names like Sqrt#) ──────────
            // 2026-08-07 (object instance pools): `spawn Obj(args)` — create
            // an obj instance + return a linear handle (SPEC §12.2).
            // 2026-08-09 (Phase 5): `box spawn Obj(args)` / `spill spawn
            // Obj(args)` — contextual storage-class keywords, recognized ONLY
            // immediately before `spawn` (elsewhere `box`/`spill` stay legal
            // identifiers — the compiler backend's own .bv uses `spill` as a
            // register word). `advance()` already consumed the `spawn` token,
            // so parse_spawn_body starts at the type name.
            Some((Token::Spawn, _)) => self.parse_spawn_body(SpawnStorage::Pooled),
            Some((Token::Identifier(name), span)) => {                // 2026-08-05 (Phase 3): adjacent prefix-discriminator literals
                // (`sql"SELECT"`) are removed; domain literals use explicit
                // macro calls such as `sql!("SELECT")` (SPEC §16.2).
                // 2026-08-09 (Phase 5): contextual storage-class keywords
                // `box`/`spill` — recognized ONLY when immediately followed by
                // `spawn`. `advance()` already consumed the KEYWORD, so pos is
                // AT `spawn` — consume it, then parse the spawn body.
                if name == "box" && self.peek() == Some(&Token::Spawn) {
                    self.advance();
                    return self.parse_spawn_body(SpawnStorage::Box);
                }
                if name == "spill" && self.peek() == Some(&Token::Spawn) {
                    self.advance();
                    return self.parse_spawn_body(SpawnStorage::Spill);
                }
                // 2026-07-24: Struct literal: TypeName { field: expr; ... }
                // Only parse as struct literal when the name starts with
                // uppercase (PascalCase type names). This prevents `!first { ... }`
                // from being parsed as a struct literal — `{` must remain for
                // `when`/`foreach` block bodies.
                // 2026-07-26: Must also verify content after { looks like a struct
                // field (identifier: expr) to avoid consuming guard/block braces.
                if self.peek() == Some(&Token::LBrace) && name.starts_with(|c: char| c.is_uppercase()) {
                    if self.lookahead_is_struct_literal() {
                        return self.parse_struct_literal(name);
                    }
                }
                self.parse_identifier_or_special(name)
            }

            // 2026-07-23: #Self hashword for protocol contract self-reference.
            Some((Token::HashSelf, _)) => Ok(Expr::Identifier("#Self".to_string())),

            // ── Grouping: (expr) ────────────────────────────────────
            Some((Token::LParen, _)) => self.parse_grouping(),

            // ── Block: { stmts } ────────────────────────────────────
            Some((Token::LBrace, _)) => self.parse_block_expr(),

            // ── If expression ───────────────────────────────────────
            Some((Token::Match, _)) => self.parse_match_expr(),

            // ── List literal: [expr, ...] ───────────────────────────
            Some((Token::LBracket, _)) => self.parse_list_literal(),

            // 2026-07-15: Keywords used as identifiers (input, output, etc.)
            Some((tok, span)) => {
                // 2026-08-22 (spec-conformance Phase 2): removed/reserved
                // words reject with their own message in expression position.
                if let Some(msg) = Parser::removed_word_message(&tok) {
                    return Err(SyntaxError::InvalidExpression {
                        reason: msg.to_string(),
                        span: self.make_span(span),
                    });
                }
                if let Some(name) = self.keyword_as_identifier(&tok) {
                    return Ok(Expr::Identifier(name));
                }
                let msg = format!("unexpected token '{}'", tok);
                Err(SyntaxError::InvalidExpression {
                    reason: msg,
                    span: self.make_span(span),
                })
            }
            None => Err(SyntaxError::UnexpectedEOF {
                expected: "expression".into(),
                span: Span::dummy(),
            }),
        }
    }

    /// Handle identifiers that might be followed by => (lambda) or are keywords.
    fn parse_identifier_or_special(&mut self, name: String) -> Result<Expr, SyntaxError> {
        // Lambda: param => body
        if self.eat(&Token::Arrow) {
            let body = self.parse_expression()?;
            return Ok(Expr::Lambda(vec![name], Box::new(body)));
        }
        // 2026-07-25: fn? — compile-time existence check
        if self.eat(&Token::Question) {
            return Ok(Expr::Exists(name));
        }
        Ok(Expr::Identifier(name))
    }

    /// Parse a parenthesized expression or tuple.
    ///
    /// 2026-08-04 (Phase 1): also handles the C-style cast `(Type) expr`.
    /// `(Type)` is a cast only when (a) the identifier after `(` is a known
    /// type name, (b) it is immediately followed by `)`, and (c) the token
    /// after `)` can start an expression. This is the C typedef-table
    /// disambiguation: `(x) - 1` stays grouping-minus, `(Int) -1` is a cast.
    fn parse_grouping(&mut self) -> Result<Expr, SyntaxError> {
        if let Some(cast) = self.try_parse_c_style_cast()? {
            return Ok(cast);
        }
        let mut exprs = Vec::new();
        if !self.check(&Token::RParen) {
            loop {
                exprs.push(self.parse_expression()?);
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
        }
        self.expect(Token::RParen)?;
        // 2026-08-14 (generic `defn f<T>` dispatch): a MULTI-PARAM lambda —
        // `(a, b) -> body`. Each element must be a bare identifier; the
        // `->` after the `)` marks the lambda (the stdlib's `iter_fold`
        // closures use `(acc, x) -> ...`; the single-param `x -> body` form
        // is handled in parse_identifier_or_special).
        if self.eat(&Token::Arrow) && exprs.len() > 1 {
            let mut params = Vec::with_capacity(exprs.len());
            for e in &exprs {
                match e {
                    Expr::Identifier(n) => params.push(n.clone()),
                    _ => {
                        return self.error_at_current(
                            "lambda parameters must be bare identifiers — `(a, b) -> body`",
                        );
                    }
                }
            }
            let body = self.parse_expression()?;
            return Ok(Expr::Lambda(params, Box::new(body)));
        }
        if exprs.len() == 1 {
            Ok(exprs.into_iter().next().unwrap())
        } else {
            Ok(Expr::Tuple(exprs))
        }
    }

    /// 2026-08-04 (Phase 1): `(Type) expr` — the C-style cast. `(` is already
    /// consumed (parse_primary advanced past it). Returns Some(cast) when the
    /// lookahead proves a cast; None to fall through to grouping/tuple.
    fn try_parse_c_style_cast(&mut self) -> Result<Option<Expr>, SyntaxError> {
        // Pattern: Identifier(name) [ RParen ] <expr-start>
        let Some((Token::Identifier(name), _)) = self.peek_with_span() else {
            return Ok(None);
        };
        if !self.known_types.contains(name) {
            return Ok(None);
        }
        let Some(&Token::RParen) = self.tokens.get(self.pos + 1).map(|(t, _)| t) else {
            return Ok(None);
        };
        let Some(next) = self.tokens.get(self.pos + 2).map(|(t, _)| t) else {
            return Ok(None);
        };
        if !Self::token_starts_expression(next) {
            return Ok(None);
        }
        // Consume `Identifier` then `)`, then parse the operand at UNARY
        // precedence (matching C: `(Int) x + 1` = `((Int) x) + 1`; the outer
        // binary + is applied by the caller's precedence chain).
        let ty_name = name.clone();
        self.pos += 2;
        let ty = Self::simple_type_from_name(&ty_name);
        let operand = self.parse_unary()?;
        Ok(Some(Expr::Cast(Box::new(operand), ty)))
    }

    /// 2026-08-04 (Phase 1): construct the Type for a simple type NAME without
    /// consuming tokens (the `(Type) expr` form only supports bare type names —
    /// `Ptr<T>`/`Int[8]` casts use `expr as Ptr<Int>`). Mirrors parse_type's
    /// primitive dispatch.
    fn simple_type_from_name(name: &str) -> crate::ast::Type {
        match name {
            "Int" => crate::ast::Type::int(),
            "UInt" => crate::ast::Type::Custom("UInt".into()),
            "Float" | "Float32" | "F32" => crate::ast::Type::float(),
            "Float64" | "F64" | "Double" => crate::ast::Type::float64(),
            "String" => crate::ast::Type::string(),
            "Bool" => crate::ast::Type::bool_(),
            "Void" => crate::ast::Type::void(),
            "Char" => crate::ast::Type::char_(),
            "Blob" => crate::ast::Type::blob(),
            other if other.starts_with('#') => {
                // 2026-09-11 (Phase A transition): must mirror parse_type's
                // hashword arm so `b as #String` ≡ `(#String) b` holds until
                // the A4 deletion flips BOTH sites to bare in one commit.
                let variant = match other {
                    "#String" => "UTF8",
                    "#Float" => "IEEE754",
                    "#Char" => "unicode",
                    _ => "",
                };
                if !variant.is_empty() {
                    crate::ast::Type::HashWordVariant(other.to_string(), variant.to_string())
                } else {
                    crate::ast::Type::HashWord(other.to_string())
                }
            }
            other => {
                let bare = other.strip_prefix('#').unwrap_or(other);
                crate::ast::Type::Custom(bare.to_string())
            }
        }
    }

    /// Does this token begin an expression? Used by the C-style cast
    /// disambiguation — the token after `)` must start the cast operand.
    fn token_starts_expression(tok: &Token) -> bool {
        matches!(
            tok,
            Token::Identifier(_)
                | Token::Integer(_)
                | Token::Float(_)
                | Token::String(_)
                | Token::Char(_)
                | Token::BoolTrue
                | Token::BoolFalse
                | Token::LParen
                | Token::LBracket
                | Token::LBrace
                | Token::Not
                | Token::Minus
                | Token::Tilde
                | Token::Star
                | Token::Ampersand
                | Token::HashSelf
        )
    }

    /// Parse a block expression: { stmt; stmt; ... }
    fn parse_block_expr(&mut self) -> Result<Expr, SyntaxError> {
        // 2026-08-23: the `{` was already consumed by parse_primary's
        // advance() before dispatching here — parse statements directly.
        //
        // 2026-08-23 (nested match fix): parse_statement dispatches
        // Token::Match to the STATEMENT match form (block bodies + `;`),
        // but inside a block expression we need the EXPRESSION match form.
        // Route Match tokens to parse_match_expr directly and wrap in
        // Statement::Expression.
        let mut stmts = Vec::new();
        loop {
            if self.check(&Token::RBrace) || self.is_at_end() {
                break;
            }
            if self.check(&Token::Semicolon) {
                self.advance();
                continue;
            }
            // Match expression: use the expression form, not the statement form.
            if self.check(&Token::Match) {
                self.advance(); // consume 'match' keyword
                let expr = self.parse_match_expr()?;
                self.eat(&Token::Semicolon);
                stmts.push(crate::ast::Statement::Expression(expr));
                continue;
            }
            // Try parsing as a full statement (with semicolon).
            let saved = self.pos;
            match self.parse_statement() {
                Ok(stmt) => {
                    stmts.push(stmt);
                    continue;
                }
                Err(_) => {
                    // Restore and try as a bare tail expression.
                    self.pos = saved;
                }
            }
            // Tail expression: parsed WITHOUT semicolon expectation.
            let tail = self.parse_expression()?;
            stmts.push(crate::ast::Statement::Expression(tail));
            break;
        }
        self.expect(Token::RBrace)?;
        Ok(Expr::Block(stmts))
    }

    /// Parse a match expression.
    pub(crate) fn parse_match_expr(&mut self) -> Result<Expr, SyntaxError> {
        let expr = self.parse_expression()?;
        self.expect(Token::LBrace)?;
        let mut arms = Vec::new();
        while !self.check(&Token::RBrace) && !self.is_at_end() {
            // 2026-08-23: | alternatives within one arm — parse first
            // pattern, then loop on Pipe collecting alternatives.
            let mut patterns = vec![self.parse_pattern()?];
            while self.eat(&Token::Pipe) {
                patterns.push(self.parse_pattern()?);
            }
            let pattern = if patterns.len() == 1 {
                patterns.pop().unwrap()
            } else {
                crate::ast::Pattern::Multi(patterns)
            };
            // 2026-08-06: Guards use `when` (Briev has no `if`; SPEC §10.2/§11).
            // Previously parsed a bare identifier "if" — silently accepting a
            // non-keyword and rejecting the normative `when` guard.
            let guard = if self.eat(&Token::When) {
                Some(self.parse_expression()?)
            } else {
                None
            };
            // 2026-08-06: Match arms use `=>` (FatArrow), matching the
            // statement form (SPEC §8, line 194). Previously expected `->`
            // (Arrow), so `=>` failed to parse.
            self.expect(Token::FatArrow)?;
            let body = self.parse_expression()?;
            // 2026-08-06: Accept optional `;` as well as `,`. Canonical arms
            // are comma-separated (last arm may omit); the `.f` layout pass
            // terminates same-indent lines with `;`. Both produce the
            // identical AST.
            self.eat(&Token::Comma);
            self.eat(&Token::Semicolon);
            arms.push(crate::ast::MatchArm {
                pattern,
                guard,
                body: Box::new(body),
            });
        }
        self.expect(Token::RBrace)?;
        Ok(Expr::Match(Box::new(expr), arms))
    }

    /// Parse a list literal: [a, b, c]
    fn parse_list_literal(&mut self) -> Result<Expr, SyntaxError> {
        let mut elems = Vec::new();
        if !self.check(&Token::RBracket) {
            loop {
                elems.push(self.parse_expression()?);
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
        }
        self.expect(Token::RBracket)?;
        Ok(Expr::List(elems))
    }

    /// Parse a pattern for match arms.
    pub(crate) fn parse_pattern(&mut self) -> Result<crate::ast::Pattern, SyntaxError> {
        // 2026-08-22 (spec-conformance plan Phase 4a): a parenthesized
        // pattern is a TUPLE pattern `(p1, p2, …)` — the Bool-tuple form
        // (`(true, true) => …`) and member-wise destructuring. A single
        // element keeps Product shape (`(x)` matches 1-tuples); use `x` for
        // plain bindings.
        if self.eat(&Token::LParen) {
            let mut elems = Vec::new();
            if !self.check(&Token::RParen) {
                loop {
                    elems.push(self.parse_pattern()?);
                    if !self.eat(&Token::Comma) {
                        break;
                    }
                }
            }
            self.expect(Token::RParen)?;
            return Ok(crate::ast::Pattern::Tuple(elems));
        }
        match self.peek() {
            Some(Token::Underscore) => {
                self.pos += 1;
                Ok(crate::ast::Pattern::Wildcard)
            }
            Some(Token::Identifier(name)) => {
                let name = name.clone();
                self.pos += 1;
                // 2026-08-22 (spec-conformance plan Phase 3, SPEC §8.4): a
                // typed binding of a structural sum member — `number: Int =>`.
                // Distinguished from a plain Binding by the colon; the type
                // operand is a full type (union members are single types).
                if self.eat(&Token::Colon) {
                    let ty = self.parse_type()?;
                    return Ok(crate::ast::Pattern::TypedBinding(name, Box::new(ty)));
                }
                // 2026-08-26 (qualified enum paths): `Enum::Variant(subs)` —
                // the pattern name carries the `::` path; matching
                // normalizes to the bare variant (last segment).
                if self.check(&Token::ColonColon) {
                    self.pos += 1;
                    let variant = self.expect_identifier()?;
                    let qualified = format!("{}::{}", name, variant);
                    let mut fields = Vec::new();
                    if self.eat(&Token::LParen) {
                        if !self.check(&Token::RParen) {
                            loop {
                                fields.push(self.parse_pattern()?);
                                if !self.eat(&Token::Comma) {
                                    break;
                                }
                            }
                        }
                        self.expect(Token::RParen)?;
                    }
                    return Ok(crate::ast::Pattern::EnumVariant(qualified, fields));
                }
                // Enum variant with fields: Foo(a, b)
                if self.eat(&Token::LParen) {
                    let mut fields = Vec::new();
                    if !self.check(&Token::RParen) {
                        loop {
                            fields.push(self.parse_pattern()?);
                            if !self.eat(&Token::Comma) {
                                break;
                            }
                        }
                    }
                    self.expect(Token::RParen)?;
                    Ok(crate::ast::Pattern::EnumVariant(name, fields))
                } else {
                    Ok(crate::ast::Pattern::Binding(name))
                }
            }
            Some(Token::Integer(_))
            | Some(Token::String(_))
            | Some(Token::BoolTrue)
            | Some(Token::BoolFalse) => {
                let lit = self.parse_primary()?;
                // Range pattern: 1..5 (half-open) / 1..=5 (inclusive)
                if self.eat(&Token::DotDot) {
                    let end = self.parse_primary()?;
                    Ok(crate::ast::Pattern::Range(lit, end))
                } else if self.eat(&Token::DotDotEq) {
                    let end = self.parse_primary()?;
                    Ok(crate::ast::Pattern::RangeInclusive(lit, end))
                } else {
                    Ok(crate::ast::Pattern::Literal(lit))
                }
            }
            _ => self.error_at_current("expected pattern"),
        }
    }

    /// Parse a struct literal: TypeName { field: expr; ... }
    /// 2026-07-24: Constructs a value of a static struct type.
    /// 2026-07-31: Accepts semicolon OR comma separators, and the bare
    /// shorthand `TypeName { field, other }` where the value is the
    /// identifier `field`.
    fn parse_struct_literal(&mut self, type_name: String) -> Result<Expr, SyntaxError> {
        self.pos += 1; // consume {
        let mut fields = Vec::new();
        while !self.check(&Token::RBrace) && !self.is_at_end() {
            let name = self.expect_identifier()?;
            if self.eat(&Token::Colon) {
                let value = self.parse_expression()?;
                fields.push((name, value));
            } else {
                // 2026-07-31: Bare shorthand: `Arena { base, offset }` means
                // `Arena { base: base, offset: offset }`.
                fields.push((name.clone(), Expr::Identifier(name)));
            }
            // 2026-07-31: Accept either `;` or `,` separators (and a single
            // trailing separator before the closing brace).
            if !self.eat(&Token::Semicolon) && !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(Token::RBrace)?;
        Ok(Expr::StructLiteral { type_name, fields })
    }

    /// 2026-07-26: Peek ahead after PascalCaseName { to check if the content
    /// looks like struct fields (identifier: expr) rather than guard/block
    /// bodies. Prevents `when TOTAL { let x ...` from being parsed as a
    /// struct literal when TOTAL is a PascalCase variable, not a type.
    /// 2026-07-31: Accepts the bare shorthand too: `T { a, b }` (identifier
    /// followed by ',' or '}') as well as `T { a: e }`.
    fn lookahead_is_struct_literal(&self) -> bool {
        // Look at the token after the current position (which is {)
        let after_brace = self.pos + 1;
        if after_brace >= self.tokens.len() { return false; }
        let next_tok = &self.tokens[after_brace].0;
        // 2026-09-11 (Part C): `T { }` — an EMPTY struct literal (pins-only
        // component constructions, zero-value placeholders). An identifier
        // immediately followed by `{}` is a construction, never a block:
        // blocks never attach directly to identifiers in Briev.
        if matches!(next_tok, Token::RBrace) { return true; }
        let next_is_ident = matches!(next_tok, Token::Identifier(_));
        if !next_is_ident { return false; }
        // Check the token after the identifier — must be ':' or ',' for a
        // struct field (comma = bare shorthand); otherwise it's a block body.
        let after_ident = after_brace + 1;
        if after_ident >= self.tokens.len() { return false; }
        matches!(&self.tokens[after_ident].0, Token::Colon | Token::Comma)
    }

    pub fn parse_block(&mut self) -> Result<Vec<crate::ast::Statement>, SyntaxError> {
        self.expect(Token::LBrace)?;
        let mut stmts = Vec::new();
        while !self.check(&Token::RBrace) && !self.is_at_end() {
            stmts.push(self.parse_statement()?);
        }
        self.expect(Token::RBrace)?;
        Ok(stmts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn parse_expr(src: &str) -> Result<Expr, SyntaxError> {
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        p.parse_expression()
    }

    /// Both cast syntaxes lower to the same Expr::Cast(operand, ty).
    fn assert_cast_equiv(as_form: &str, paren_form: &str) {
        let a = parse_expr(as_form).expect(as_form);
        let b = parse_expr(paren_form).expect(paren_form);
        assert_eq!(a, b, "'{as_form}' must parse identically to '{paren_form}'");
    }

    #[test]
    fn c_style_cast_string_matches_as() {
        assert_cast_equiv("n as String", "(String) n");
    }

    #[test]
    fn range_inclusive_pattern_parses() {
        // 2026-08-06 (Phase 7): `a..=b` lexes as DotDotEq and parses to a
        // RangeInclusive pattern; `a..b` stays half-open.
        let expr = parse_expr("match 5 { 1..=5 => 7, 1..5 => 3, _ => 0 }").unwrap();
        match expr {
            Expr::Match(_, arms) => {
                assert_eq!(arms.len(), 3);
                assert!(matches!(arms[0].pattern, crate::ast::Pattern::RangeInclusive(..)),
                    "first arm must be inclusive");
                assert!(matches!(arms[1].pattern, crate::ast::Pattern::Range(..)),
                    "second arm must stay half-open");
            }
            other => panic!("expected match, got {other:?}"),
        }
    }

    #[test]
    fn range_expression_parses() {
        // 2026-08-07 (Phase 7): `a..b` / `a..=b` are iterable range
        // EXPRESSIONS (distinct from range patterns) — consumed by foreach.
        let expr = parse_expr("0..=5").unwrap();
        assert!(matches!(expr, Expr::Range { inclusive: true, .. }),
            "0..=5 must parse as an inclusive range, got {expr:?}");
        let expr = parse_expr("0..5").unwrap();
        assert!(matches!(expr, Expr::Range { inclusive: false, .. }),
            "0..5 must parse as a half-open range, got {expr:?}");
        if let Expr::Range { start, end, .. } = expr {
            assert!(matches!(*start, Expr::Decimal(0)));
            assert!(matches!(*end, Expr::Decimal(5)));
        }
    }

    #[test]
    fn named_selector_lowers_to_plain_form() {
        // 2026-08-07 (Phase 7): `arr[name => sel]` (SPEC §16.5) is a LABEL on
        // the single 1-D dimension — it must parse to the exact same AST as
        // the plain slice/index. Cross-checking the name against a declared
        // dimension arrives with named dims (const generics, §16.6).
        let named = parse_expr("v[width => 1:3]").unwrap();
        let plain = parse_expr("v[1:3]").unwrap();
        assert_eq!(named, plain, "a named slice selector must equal the plain slice");
        let named = parse_expr("v[time => 4]").unwrap();
        let plain = parse_expr("v[4]").unwrap();
        assert_eq!(named, plain, "a named index selector must equal the plain index");
    }

    #[test]
    fn c_style_cast_int_matches_as() {
        assert_cast_equiv("f as Int", "(Int) f");
    }

    #[test]
    fn c_style_cast_float_matches_as() {
        assert_cast_equiv("x as Float", "(Float) x");
    }

    #[test]
    fn c_style_cast_hashword_matches_as() {
        // 2026-09-11 (fundamentals doctrine, Phase A): the #Category
        // spellings are retiring — both cast forms now map to the bare
        // fundamental, and the as/paren equivalence is preserved.
        assert_cast_equiv("b as #String", "(#String) b");
        let a = parse_expr("b as #String").expect("#String cast");
        let bare = parse_expr("b as String").expect("String cast");
        assert_eq!(a, bare, "retired #String must map to the bare fundamental");
    }

    #[test]
    fn c_style_cast_custom_type_prescan() {
        // Custom types: the pre-scan must collect `type MyNum` declarations.
        let src = "type MyNum : #Int { };";
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        let first = p.parse_top_level().expect("type decl");
        assert!(matches!(first, crate::ast::TopLevel::TypeDef(_)));
        assert!(p.known_types.contains("MyNum"));
    }

    #[test]
    fn c_style_cast_binds_tighter_than_binary() {
        // (Int) x + 1 must be ((Int) x) + 1 — binary + applies at the outer level.
        let e = parse_expr("(Int) x + 1").unwrap();
        assert!(
            matches!(e, Expr::BinaryOp(BinaryOpKind::Add, _, _)),
            "expected outer Add, got {e:?}"
        );
    }

    #[test]
    fn grouping_still_parses_for_non_type() {
        // A lowercase name is not a known type → grouping, not cast.
        let e = parse_expr("(x) - 1").unwrap();
        assert!(
            matches!(e, Expr::BinaryOp(BinaryOpKind::Sub, _, _)),
            "expected grouping-minus, got {e:?}"
        );
    }

    #[test]
    fn grouping_single_expr_unchanged() {
        let e = parse_expr("(x)").unwrap();
        assert!(matches!(e, Expr::Identifier(ref n) if n == "x"));
    }

    #[test]
    fn tuple_grouping_unchanged() {
        let e = parse_expr("(a, b)").unwrap();
        assert!(matches!(e, Expr::Tuple(ref v) if v.len() == 2));
    }

    #[test]
    fn match_expr_uses_when_guard_and_fat_arrow() {
        let e = parse_expr("match n { _ when n < 0 => -1, 0 => 0, _ => 1 }").unwrap();
        let Expr::Match(scrutinee, arms) = e else {
            panic!("expected Expr::Match");
        };
        assert!(matches!(*scrutinee, Expr::Identifier(ref n) if n == "n"));
        assert_eq!(arms.len(), 3);
        assert!(arms[0].guard.is_some());
        assert!(arms[1].guard.is_none());
    }

    #[test]
    fn match_expr_accepts_semicolon_separators() {
        // 2026-08-06: The `.f` layout pass terminates same-indent match arms
        // with `;`. Canonical comma-separated form and the `.f` form must
        // produce the identical AST.
        let comma = parse_expr("match n { _ when n < 0 => -1, 0 => 0 }").unwrap();
        let semi = parse_expr("match n { _ when n < 0 => -1; 0 => 0; }").unwrap();
        assert_eq!(comma, semi, "`,` and `;` arm separators must parse identically");
    }

    #[test]
    fn match_expr_single_arm_without_trailing_separator() {
        let e = parse_expr("match n { 0 => 0 }").unwrap();
        let Expr::Match(_, arms) = e else { panic!("expected Expr::Match") };
        assert_eq!(arms.len(), 1);
    }

    #[test]
    fn match_expr_rejects_if_guard() {
        // 2026-08-06: `if` is not a Briev keyword; guards are `when`. A guard
        // written with `if` must fail to parse, not be silently accepted as an
        // identifier.
        assert!(parse_expr("match n { _ if n < 0 => -1 }").is_err());
    }

    #[test]
    fn lambda_with_match_arrow_hints_at_arrow() {
        // 2026-08-06 (diagnostics): `x => body` is a common lambda spelling —
        // match arms use `=>`; lambda parameters use `->`. At statement level
        // the `=>` is unexpected and the error must hint.
        let src = "node s [true][false] { let f = x => x + 1; term; };";
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        let err = p.parse_program().err().expect("`=>` lambda must fail");
        let msg = format!("{}", err);
        assert!(
            msg.contains("hint: match arms use '=>'; lambda parameters use '->'"),
            "expected the arrow hint, got: {msg}"
        );
    }

    // ── 2026-08-09 (Phase 5): box/spill storage-class spawns ──────────

    #[test]
    fn box_spawn_parses_with_box_storage() {
        let expr = parse_expr("box spawn Counter(5)").unwrap();
        match expr {
            Expr::Spawn { type_name, args, storage } => {
                assert_eq!(type_name, "Counter");
                assert_eq!(args.len(), 1);
                assert_eq!(storage, crate::ast::SpawnStorage::Box);
            }
            other => panic!("expected box spawn, got {other:?}"),
        }
    }

    #[test]
    fn spill_spawn_parses_with_spill_storage() {
        let expr = parse_expr("spill spawn Counter()").unwrap();
        match expr {
            Expr::Spawn { type_name, args, storage } => {
                assert_eq!(type_name, "Counter");
                assert!(args.is_empty());
                assert_eq!(storage, crate::ast::SpawnStorage::Spill);
            }
            other => panic!("expected spill spawn, got {other:?}"),
        }
    }

    #[test]
    fn plain_spawn_stays_pooled() {
        let expr = parse_expr("spawn Counter()").unwrap();
        match expr {
            Expr::Spawn { storage, .. } => {
                assert_eq!(storage, crate::ast::SpawnStorage::Pooled);
            }
            other => panic!("expected spawn, got {other:?}"),
        }
    }

    #[test]
    fn box_without_spawn_is_a_plain_identifier() {
        // Contextual keyword: `box` alone (no `spawn` after) stays an
        // identifier — the compiler backend's own .bv uses `spill` as a
        // register word.
        let expr = parse_expr("box").unwrap();
        assert!(matches!(expr, Expr::Identifier(n) if n == "box"));
        let expr = parse_expr("spill").unwrap();
        assert!(matches!(expr, Expr::Identifier(n) if n == "spill"));
    }

    #[test]
    fn spill_identifier_in_call_position_stays_identifier() {
        // `let box = 5; box` — `box` used as a variable name is untouched.
        let expr = parse_expr("box + 1").unwrap();
        assert!(matches!(expr, Expr::BinaryOp(_, l, _) if matches!(l.as_ref(), Expr::Identifier(n) if n == "box")));
    }

    // ── 2026-08-09 (Phase 10): await task ────────────────────────────

    #[test]
    fn await_parses_as_unary() {
        let expr = parse_expr("await t").unwrap();
        assert!(matches!(expr, Expr::Await(inner) if matches!(inner.as_ref(), Expr::Identifier(n) if n == "t")));
    }

    #[test]
    fn spawn_defn_parses_as_task() {
        // `spawn compute(21)` — the callee is a defn, not an obj base; the
        // parser produces the same Spawn form (the typechecker classifies it).
        let expr = parse_expr("spawn compute(21)").unwrap();
        match expr {
            Expr::Spawn { type_name, args, storage } => {
                assert_eq!(type_name, "compute");
                assert_eq!(args.len(), 1);
                assert_eq!(storage, crate::ast::SpawnStorage::Pooled);
            }
            other => panic!("expected spawn, got {other:?}"),
        }
    }
}
