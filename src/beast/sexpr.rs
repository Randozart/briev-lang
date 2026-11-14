// ── S-Expression Parser ─────────────────────────────────────────────────
// 2026-07-14: Tokenizer + recursive descent for (.beast) format.
// Every node is (tag child1 child2 ...).
// Max 2 nesting levels. Extract helpers.

use std::fmt::Write;

#[derive(Debug, Clone, PartialEq)]
pub enum Atom {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SExpr {
    Atom(Atom),
    List(Vec<SExpr>),
}

/// 2026-07-14: Read a quoted string from the character stream.
fn read_string(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<String, String> {
    let mut s = String::new();
    s.push('"');
    loop {
        match chars.next() {
            Some('"') => {
                s.push('"');
                return Ok(s);
            }
            Some('\\') => {
                s.push('\\');
                if let Some(c) = chars.next() {
                    s.push(c);
                }
            }
            Some(c) => s.push(c),
            None => return Err("unterminated string literal".into()),
        }
    }
}

/// 2026-07-14: Skip a comment to end of line.
fn skip_comment(chars: &mut std::iter::Peekable<std::str::Chars>) {
    for c in chars.by_ref() {
        if c == '\n' {
            return;
        }
    }
}

/// Tokenize an S-expression string into a stream of tokens.
pub fn tokenize(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut current = String::new();
    loop {
        let ch = match chars.next() {
            Some(c) => c,
            None => {
                if !current.is_empty() {
                    tokens.push(current);
                }
                return Ok(tokens);
            }
        };
        match ch {
            '(' | ')' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(ch.to_string());
            }
            ';' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                skip_comment(&mut chars);
            }
            ' ' | '\t' | '\n' | '\r' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '"' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                current = read_string(&mut chars)?;
                tokens.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
}

fn peek_token<'a>(tokens: &'a [String], pos: usize) -> Result<&'a str, String> {
    tokens
        .get(pos)
        .map(|s| s.as_str())
        .ok_or_else(|| "unexpected end of input".into())
}

fn parse_atom(tok: &str) -> Result<Atom, String> {
    if let Some(s) = tok.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return Ok(Atom::String(s.to_string()));
    }
    if tok == "true" {
        return Ok(Atom::Bool(true));
    }
    if tok == "false" {
        return Ok(Atom::Bool(false));
    }
    if let Ok(n) = tok.parse::<i64>() {
        return Ok(Atom::Int(n));
    }
    if let Ok(f) = tok.parse::<f64>() {
        return Ok(Atom::Float(f));
    }
    // Fallback: bare identifier — treat as string
    Ok(Atom::String(tok.to_string()))
}

/// 2026-07-14: Parse a single S-expression from token stream at position pos.
fn parse_one(tokens: &[String], pos: &mut usize) -> Result<SExpr, String> {
    let tok = peek_token(tokens, *pos)?;
    if tok == ")" {
        return Err("unexpected ')".into());
    }
    if tok != "(" {
        let atom = parse_atom(tok)?;
        *pos += 1;
        return Ok(SExpr::Atom(atom));
    }
    *pos += 1;
    let mut children = Vec::new();
    loop {
        let next = peek_token(tokens, *pos)?;
        if next == ")" {
            *pos += 1;
            return Ok(SExpr::List(children));
        }
        children.push(parse_one(tokens, pos)?);
    }
}

/// Parse a list of tokens into a single S-expression.
pub fn parse(tokens: &[String]) -> Result<SExpr, String> {
    parse_one(tokens, &mut 0)
}

/// 2026-07-14: Compute character width of an S-expression (for single-line heuristic).
fn expr_width(expr: &SExpr) -> usize {
    match expr {
        SExpr::Atom(a) => match a {
            Atom::String(s) => s.len() + 2,
            Atom::Int(n) => format!("{}", n).len(),
            Atom::Float(f) => format!("{}", f).len(),
            Atom::Bool(_) => 5,
        },
        SExpr::List(children) => {
            let inner: usize = children.iter().map(expr_width).sum();
            inner + children.len().saturating_sub(1) + 2
        }
    }
}

/// 2026-07-14: Write an S-expression atom to a string.
fn write_atom(a: &Atom, out: &mut String) {
    let _ = match a {
        Atom::String(s) => write!(out, "\"{}\"", s),
        Atom::Int(n) => write!(out, "{}", n),
        Atom::Float(f) => write!(out, "{}", f),
        Atom::Bool(b) => write!(out, "{}", b),
    };
}

/// 2026-07-14: Write children on one line: (a b c)
fn write_short(children: &[SExpr], out: &mut String) {
    let _ = write!(out, "(");
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            let _ = write!(out, " ");
        }
        emit(child, out);
    }
    let _ = write!(out, ")");
}

/// 2026-07-14: Write children indented, one per line.
fn write_long(children: &[SExpr], out: &mut String) {
    let _ = writeln!(out, "(");
    for child in children {
        let _ = write!(out, "  ");
        emit(child, out);
        let _ = writeln!(out);
    }
    let _ = write!(out, ")");
}

/// Pretty-print an S-expression as a string.
pub fn to_string(expr: &SExpr) -> String {
    let mut out = String::new();
    emit(expr, &mut out);
    out
}

fn emit(expr: &SExpr, out: &mut String) {
    match expr {
        SExpr::Atom(a) => write_atom(a, out),
        SExpr::List(children) => {
            if children.is_empty() {
                write!(out, "()").ok();
                return;
            }
            if expr_width(expr) < 60 {
                write_short(children, out);
            } else {
                write_long(children, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_simple() {
        let tokens = tokenize("(a b c)").unwrap();
        assert_eq!(tokens, vec!["(", "a", "b", "c", ")"]);
    }

    #[test]
    fn test_tokenize_string() {
        let tokens = tokenize("(k \"hello world\")").unwrap();
        assert_eq!(tokens, vec!["(", "k", "\"hello world\"", ")"]);
    }

    #[test]
    fn test_parse_atoms() {
        let tokens = tokenize("(int 42 float 3.14 bool true key kw)").unwrap();
        let expr = parse(&tokens).unwrap();
        let list = match expr {
            SExpr::List(ref l) => l,
            _ => panic!("expected list"),
        };
        assert_eq!(list.len(), 8);
    }

    #[test]
    fn test_roundtrip() {
        let input = "(universe \"Int\" (bytes 8) (alignment 8) (properties (primitive \"Int\")))";
        let tokens = tokenize(input).unwrap();
        let expr = parse(&tokens).unwrap();
        let output = to_string(&expr);
        assert!(output.contains("universe"));
        assert!(output.contains("Int"));
    }

    #[test]
    fn test_comment_ignored() {
        let tokens = tokenize("(a ; comment\n b)").unwrap();
        assert_eq!(tokens, vec!["(", "a", "b", ")"]);
    }
}
