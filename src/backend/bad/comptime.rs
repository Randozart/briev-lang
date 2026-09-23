// ── .bad comptime — constant-expression evaluation ────────────────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-09-21 (application-grade plan Phase B): recursive-descent
// evaluator for constant expressions in `.const` directives, `.struct`
// field arithmetic, and `Expr` operands. Grammar (lowest binds last):
//
//   expr   := term  (('+' | '-') term)*
//   term   := unary (('*' | '/' | '%' | '<<' | '>>') unary)*
//   unary  := ('-' | '~')? primary
//   primary:= INT | NAME | '(' expr ')'
//
// Names resolve through the caller-supplied resolver (const table,
// defn-param immediates). All arithmetic is 64-bit wrapping; division
// and modulo by zero are loud errors with the expression quoted.

/// Name resolver: const name → value, or an error naming the problem.
pub type Resolve<'a> = dyn Fn(&str) -> Result<i64, String> + 'a;

/// Evaluate `expr`; `resolve` answers identifier lookups.
pub fn eval(expr: &str, resolve: &Resolve) -> Result<i64, String> {
    let p = Parser { src: expr, pos: 0 };
    let (v, p) = p.expr(resolve)?;
    let p = p.skip_ws();
    if p.pos < p.src.len() {
        return Err(format!(
            "constant expression `{expr}` has trailing input at `{}` - check the syntax",
            &p.src[p.pos..]
        ));
    }
    Ok(v)
}

#[derive(Clone)]
struct Parser<'s> {
    src: &'s str,
    pos: usize,
}

impl<'s> Parser<'s> {
    fn skip_ws(mut self) -> Self {
        while self.src[self.pos..].starts_with(char::is_whitespace) {
            self.pos += 1;
        }
        self
    }

    fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }

    fn eat(&mut self, tok: &str) -> bool {
        let p = self.clone().skip_ws();
        if p.src[p.pos..].starts_with(tok) {
            self.pos = p.pos + tok.len();
            true
        } else {
            false
        }
    }

    fn expr(self, resolve: &Resolve) -> Result<(i64, Self), String> {
        let (mut v, mut p) = self.clone().skip_ws().term(resolve)?;
        loop {
            let (op, p2) = {
                let mut p2 = p.clone().skip_ws();
                if p2.eat("+") {
                    ('+', p2)
                } else if p2.eat("-") {
                    ('-', p2)
                } else {
                    return Ok((v, p));
                }
            };
            let (rhs, p3) = p2.term(resolve)?;
            v = match op {
                '+' => v.wrapping_add(rhs),
                _ => v.wrapping_sub(rhs),
            };
            p = p3;
        }
    }

    fn term(self, resolve: &Resolve) -> Result<(i64, Self), String> {
        let (mut v, mut p) = self.skip_ws().unary(resolve)?;
        loop {
            let mut p2 = p.clone().skip_ws();
            let Some(op) = take_term_op(&mut p2) else {
                return Ok((v, p));
            };
            let (rhs, p3) = p2.unary(resolve)?;
            v = apply_term_op(op, v, rhs, p.src)?;
            p = p3;
        }
    }

    fn unary(self, resolve: &Resolve) -> Result<(i64, Self), String> {
        let mut p = self.skip_ws();
        if p.eat("-") {
            let (v, p) = p.unary(resolve)?;
            return Ok((v.wrapping_neg(), p));
        }
        if p.eat("~") {
            let (v, p) = p.unary(resolve)?;
            return Ok((!v, p));
        }
        p.primary(resolve)
    }

    fn primary(self, resolve: &Resolve) -> Result<(i64, Self), String> {
        let p = self.skip_ws();
        if p.peek() == Some(b'(') {
            return p.primary_paren(resolve);
        }
        let tok = p.token_at_pos();
        let next = Self { src: p.src, pos: p.pos + tok.len() };
        let v = classify_token(tok, resolve)?;
        Ok((v, next))
    }

    fn primary_paren(self, resolve: &Resolve) -> Result<(i64, Self), String> {
        let mut p = self;
        if !p.eat("(") {
            return Err(format!(
                "constant expression `{}` is missing an opening `(`",
                p.src
            ));
        }
        let (v, p) = p.expr(resolve)?;
        let mut p = p.skip_ws();
        if !p.eat(")") {
            return Err(format!(
                "constant expression `{}` is missing a closing `)`",
                p.src
            ));
        }
        Ok((v, p))
    }

    /// The identifier/number token at the current position ("" at end).
    fn token_at_pos(&self) -> &str {
        let rest = &self.src[self.pos..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        &rest[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names<'a>(pairs: &'a [(&'a str, i64)]) -> impl Fn(&str) -> Result<i64, String> + 'a {
        move |n: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == n)
                .map(|(_, v)| *v)
                .ok_or_else(|| format!("unknown constant `{n}`"))
        }
    }

    #[test]
    fn precedence_and_parens() {
        let r = names(&[("MAX", 64), ("B", 3)]);
        assert_eq!(eval("2 + 3 * 4", &r).unwrap(), 14);
        assert_eq!(eval("(2 + 3) * 4", &r).unwrap(), 20);
        assert_eq!(eval("MAX * 4 - 1", &r).unwrap(), 255);
        assert_eq!(eval("1 << 10", &r).unwrap(), 1024);
        assert_eq!(eval("MAX >> 2", &r).unwrap(), 16);
        assert_eq!(eval("7 % 3", &r).unwrap(), 1);
        assert_eq!(eval("-B + 1", &r).unwrap(), -2);
        assert_eq!(eval("~0", &r).unwrap(), -1);
        assert_eq!(eval("0xFF", &r).unwrap(), 255);
    }

    #[test]
    fn errors_are_loud() {
        let r = names(&[("A", 1)]);
        assert!(eval("1 / 0", &r).unwrap_err().contains("divides by zero"));
        assert!(eval("1 +", &r).unwrap_err().contains("unexpected token"));
        assert!(eval("A + NOPE", &r).unwrap_err().contains("unknown constant"));
        assert!(eval("(1 + 2", &r).unwrap_err().contains("closing `)`"));
    }
}

/// Multiplicative/shift operator at the cursor; advances past it.
fn take_term_op(p: &mut Parser) -> Option<&'static str> {
    for op in ["<<", ">>", "*", "/", "%"] {
        if p.eat(op) {
            return Some(op);
        }
    }
    None
}

/// Apply one multiplicative/shift operator; zero divisors are loud.
fn apply_term_op(op: &str, v: i64, rhs: i64, src: &str) -> Result<i64, String> {
    match op {
        "*" => Ok(v.wrapping_mul(rhs)),
        "/" if rhs == 0 => Err(format!("constant expression `{src}` divides by zero")),
        "/" => Ok(v.wrapping_div(rhs)),
        "%" if rhs == 0 => Err(format!("constant expression `{src}` reduces by zero")),
        "%" => Ok(v.wrapping_rem(rhs)),
        "<<" => Ok(v.wrapping_shl(rhs as u32)),
        _ => Ok(v.wrapping_shr(rhs as u32)),
    }
}

/// Classify one primary token: hex literal, decimal, or const name.
fn classify_token(tok: &str, resolve: &Resolve) -> Result<i64, String> {
    if let Some(hex) = tok.strip_prefix("0x") {
        return i64::from_str_radix(hex, 16)
            .map_err(|_| format!("bad hex literal `{tok}` in constant expression"));
    }
    if let Ok(v) = tok.parse::<i64>() {
        return Ok(v);
    }
    if starts_ident(tok) {
        return resolve(tok);
    }
    Err(format!(
        "unexpected token `{}` in constant expression",
        if tok.is_empty() { "<end>" } else { tok }
    ))
}

fn starts_ident(tok: &str) -> bool {
    tok.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '.')
}
