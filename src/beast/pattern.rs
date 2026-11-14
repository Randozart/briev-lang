// ── BEAST Pattern Compiler ──────────────────────────────────────────────
//
// 2026-07-15: Phase 6 — S-expression pattern matching for Collect$ and
// MatchIR$ intrinsics. Pattern variables use ? prefix: ?x matches any single
// sub-expression, ?* matches any single (wildcard), ??* matches remaining
// children (rest wildcard).
//
// Flat dispatch: parse -> match tree -> walk AST -> collect/substitute.
// Max 2 nesting levels. Extract helpers.

use std::collections::HashMap;

/// A compiled pattern that can be matched against S-expressions.
/// 2026-07-15: Built from a BEAST S-expression string with ? variable support.
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// Exact atom value match: 42, "hello", true
    Atom(String),
    /// Wildcard: ?* matches any single sub-expression
    Wildcard,
    /// Rest wildcard: ??* matches zero or more trailing sub-expressions
    WildcardRest,
    /// Variable: ?name matches any single sub-expression, bound to name
    Var(String),
    /// Rest variable: ?rest matches zero or more trailing, bound as list
    VarRest(String),
    /// Tagged list: (tag child1 child2 ...) — tag must match, children matched in order
    List {
        tag: Option<String>,
        children: Vec<Pattern>,
    },
}

/// A match result: maps variable names to matched S-expressions.
pub type MatchResult = Option<HashMap<String, Vec<SExpr>>>;

use super::sexpr::{Atom, SExpr};

/// 2026-07-15: Parse a pattern string into a compiled Pattern tree.
/// The pattern syntax is a superset of BEAST S-expressions with ? variables:
///   ?x      — match any single subtree, bind to x
///   ?*      — wildcard (match any single, no binding)
///   ??*     — rest wildcard (match remaining children, no binding)
///   (?tag ?x ?y) — list with tag, children matched in order
pub fn parse_pattern(input: &str) -> Result<Pattern, String> {
    let tokens = tokenize_pattern(input)?;
    let sexpr = parse_one_pattern(&tokens, &mut 0)?;
    sexpr_to_pattern(&sexpr)
}

/// 2026-07-15: Tokenize a pattern string, handling ? variables.
/// Reuses the existing tokenizer but adds ? prefix handling.
fn tokenize_pattern(input: &str) -> Result<Vec<String>, String> {
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
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
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
                current.push('"');
                loop {
                    match chars.next() {
                        Some('"') => {
                            current.push('"');
                            tokens.push(std::mem::take(&mut current));
                            break;
                        }
                        Some('\\') => {
                            current.push('\\');
                            if let Some(c) = chars.next() {
                                current.push(c);
                            }
                        }
                        Some(c) => current.push(c),
                        None => return Err("unterminated string literal in pattern".into()),
                    }
                }
            }
            '?' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                // Collect the variable name after ?
                let mut var_name = String::from("?");
                // Peek ahead to collect the rest of the variable name
                while let Some(&next) = chars.peek() {
                    if next.is_alphanumeric() || next == '_' || next == '*' {
                        var_name.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(var_name);
            }
            _ => current.push(ch),
        }
    }
}

/// 2026-07-15: Parse a single S-expression (with ? variables) from token stream.
fn parse_one_pattern(tokens: &[String], pos: &mut usize) -> Result<SExpr, String> {
    let tok = tokens
        .get(*pos)
        .ok_or_else(|| "unexpected end of pattern".to_string())?;
    if tok == ")" {
        return Err("unexpected ')' in pattern".into());
    }
    if tok != "(" {
        // Atom or variable
        let atom = if tok.starts_with('?') {
            Atom::String(tok.clone()) // Keep ? prefix for pattern identification
        } else {
            parse_pattern_atom(tok)?
        };
        *pos += 1;
        return Ok(SExpr::Atom(atom));
    }
    *pos += 1;
    let mut children = Vec::new();
    loop {
        let next = tokens
            .get(*pos)
            .ok_or_else(|| "unterminated list in pattern".to_string())?;
        if next == ")" {
            *pos += 1;
            return Ok(SExpr::List(children));
        }
        children.push(parse_one_pattern(tokens, pos)?);
    }
}

/// 2026-07-15: Parse an atom from a pattern token.
fn parse_pattern_atom(tok: &str) -> Result<Atom, String> {
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
    Ok(Atom::String(tok.to_string()))
}

/// 2026-07-15: Convert a parsed SExpr to a compiled Pattern.
fn sexpr_to_pattern(expr: &SExpr) -> Result<Pattern, String> {
    match expr {
        SExpr::Atom(atom) => {
            match atom {
                Atom::String(s) if s == "?*" => Ok(Pattern::Wildcard),
                Atom::String(s) if s == "??*" => Ok(Pattern::WildcardRest),
                Atom::String(s) if s.starts_with('?') => {
                    // Variable: ?name or ?rest
                    let name = s[1..].to_string();
                    if name == "*" {
                        Ok(Pattern::Wildcard)
                    } else if name.starts_with('?') && name.len() > 1 {
                        Ok(Pattern::VarRest(name[1..].to_string()))
                    } else {
                        Ok(Pattern::Var(name))
                    }
                }
                _ => Ok(Pattern::Atom(atom_to_string(atom)?)),
            }
        }
        SExpr::List(children) => {
            if children.is_empty() {
                return Err("empty list pattern".into());
            }
            // First child determines if there's a tag
            let (tag, child_start) = match &children[0] {
                SExpr::Atom(Atom::String(s)) => {
                    // 2026-07-15: Support * and ?* as wildcard tag (match any tag)
                    // child_start = 1 to skip the wildcard tag in the children list
                    if s == "*" || s == "?*" {
                        (None, 1)
                    } else if s.starts_with('?') {
                        return Err(format!(
                            "variable tag '{}' not supported — use * for any tag",
                            s
                        ));
                    } else {
                        (Some(s.clone()), 1)
                    }
                }
                _ => (None, 0),
            };
            let mut patterns = Vec::new();
            for i in child_start..children.len() {
                patterns.push(sexpr_to_pattern(&children[i])?);
            }
            Ok(Pattern::List {
                tag,
                children: patterns,
            })
        }
    }
}

/// 2026-07-15: Convert an atom to its string representation.
fn atom_to_string(atom: &Atom) -> Result<String, String> {
    match atom {
        Atom::String(s) => Ok(s.clone()),
        Atom::Int(n) => Ok(n.to_string()),
        Atom::Float(f) => Ok(f.to_string()),
        Atom::Bool(b) => Ok(b.to_string()),
    }
}

/// 2026-07-15: Match a compiled Pattern against an S-expression.
/// Returns bindings on success, None on failure.
pub fn match_pattern(pattern: &Pattern, expr: &SExpr) -> MatchResult {
    let mut bindings = HashMap::new();
    if match_recursive(pattern, expr, &mut bindings, false).is_ok() {
        Some(bindings)
    } else {
        None
    }
}

/// 2026-07-15: Match a Pattern against a list of S-expressions (children).
/// Returns bindings for all variables found.
pub fn match_pattern_children(pattern: &Pattern, exprs: &[SExpr]) -> MatchResult {
    let mut bindings = HashMap::new();
    if match_children_recursive(pattern, exprs, 0, &mut bindings, false).is_ok() {
        Some(bindings)
    } else {
        None
    }
}

/// 2026-07-15: Recursive match helper.
fn match_recursive(
    pattern: &Pattern,
    expr: &SExpr,
    bindings: &mut HashMap<String, Vec<SExpr>>,
    in_rest: bool,
) -> Result<(), ()> {
    match pattern {
        Pattern::Wildcard => Ok(()),
        Pattern::WildcardRest => Ok(()),
        Pattern::Var(name) => {
            bindings.entry(name.clone()).or_default().push(expr.clone());
            Ok(())
        }
        Pattern::VarRest(name) => {
            bindings.entry(name.clone()).or_default().push(expr.clone());
            Ok(())
        }
        Pattern::Atom(expected) => {
            let actual = atom_to_sexpr_str(expr).map_err(|_| ())?;
            if actual == *expected { Ok(()) } else { Err(()) }
        }
        Pattern::List { tag, children } => {
            let list = match expr {
                SExpr::List(items) => items,
                _ => return Err(()),
            };
            // Check tag if specified
            if let Some(expected_tag) = tag {
                let actual_tag = match list.first() {
                    Some(SExpr::Atom(Atom::String(s))) => s,
                    _ => return Err(()),
                };
                if actual_tag != expected_tag {
                    return Err(());
                }
            }
            // Match children starting from index 1 (skip tag position).
            // In BEAST every list has a tag at position 0, so even with a
            // wildcard tag (*) we must skip the actual tag in the target.
            match_children_recursive_list(children, list, 1, bindings)
        }
    }
}

/// 2026-07-15: Match child patterns against S-expression list items.
fn match_children_recursive_list(
    children: &[Pattern],
    items: &[SExpr],
    start: usize,
    bindings: &mut HashMap<String, Vec<SExpr>>,
) -> Result<(), ()> {
    let mut child_idx = 0;
    let mut item_idx = start;
    while child_idx < children.len() {
        let child = &children[child_idx];
        match child {
            Pattern::WildcardRest | Pattern::VarRest(_) => {
                // Consume all remaining items
                let mut rest_items = Vec::new();
                while item_idx < items.len() {
                    rest_items.push(items[item_idx].clone());
                    item_idx += 1;
                }
                if let Pattern::VarRest(name) = child {
                    bindings
                        .entry(name.clone())
                        .or_default()
                        .push(SExpr::List(rest_items));
                }
                child_idx += 1;
            }
            _ => {
                if item_idx >= items.len() {
                    return Err(());
                }
                match_recursive(child, &items[item_idx], bindings, false)?;
                item_idx += 1;
                child_idx += 1;
            }
        }
    }
    Ok(())
}

/// 2026-07-15: Match children patterns against a slice with offset.
fn match_children_recursive(
    pattern: &Pattern,
    items: &[SExpr],
    start: usize,
    bindings: &mut HashMap<String, Vec<SExpr>>,
    in_rest: bool,
) -> Result<(), ()> {
    match pattern {
        Pattern::List { tag, children } => {
            if start >= items.len() {
                return Err(());
            }
            // Check tag
            if let Some(expected_tag) = tag {
                let actual_tag = match &items[start] {
                    SExpr::Atom(Atom::String(s)) => s,
                    _ => return Err(()),
                };
                if actual_tag != expected_tag {
                    return Err(());
                }
            }
            let child_start = if tag.is_some() { start + 1 } else { start };
            match_children_recursive_list(children, items, child_start, bindings)
        }
        _ => match_recursive(pattern, &items[start], bindings, in_rest),
    }
}

/// 2026-07-15: Convert an S-expression atom to a string representation for matching.
fn atom_to_sexpr_str(expr: &SExpr) -> Result<String, ()> {
    match expr {
        SExpr::Atom(atom) => atom_to_string(atom).map_err(|_| ()),
        _ => Err(()),
    }
}

// ── Collect & Substitute ──────────────────────────────────────────────

/// 2026-07-15: Collect all sub-expressions matching a pattern from an S-expression.
/// Recursively walks the entire tree, returning all matches.
pub fn collect_matches<'a>(pattern: &Pattern, expr: &'a SExpr) -> Vec<&'a SExpr> {
    let mut results = Vec::new();
    collect_recursive(pattern, expr, &mut results);
    results
}

fn collect_recursive<'a>(pattern: &Pattern, expr: &'a SExpr, results: &mut Vec<&'a SExpr>) {
    if match_pattern(pattern, expr).is_some() {
        results.push(expr);
    }
    match expr {
        SExpr::List(children) => {
            for child in children {
                collect_recursive(pattern, child, results);
            }
        }
        SExpr::Atom(_) => {}
    }
}

/// 2026-07-15: Apply a Replacement to an S-expression tree.
/// Returns a new S-expression with all matches replaced.
pub fn apply_replacement(pattern: &Pattern, replacement: &Pattern, expr: &SExpr) -> SExpr {
    apply_recursive(pattern, replacement, expr, false)
}

fn apply_recursive(
    pattern: &Pattern,
    replacement: &Pattern,
    expr: &SExpr,
    in_match: bool,
) -> SExpr {
    // Try to match at this level
    if let Some(bindings) = match_pattern(pattern, expr) {
        if !in_match {
            // Build replacement from bindings
            return build_replacement(replacement, &bindings);
        }
    }
    // Recurse into children
    match expr {
        SExpr::List(children) => {
            let new_children: Vec<SExpr> = children
                .iter()
                .map(|c| apply_recursive(pattern, replacement, c, false))
                .collect();
            SExpr::List(new_children)
        }
        _ => expr.clone(),
    }
}

/// 2026-07-15: Build a replacement S-expression from a pattern and bindings.
/// ?x in the replacement is substituted with the bound value.
fn build_replacement(pattern: &Pattern, bindings: &HashMap<String, Vec<SExpr>>) -> SExpr {
    match pattern {
        Pattern::Atom(s) => {
            // Check if it references a variable
            SExpr::Atom(Atom::String(s.clone()))
        }
        Pattern::Var(name) => {
            // Substitute with the bound value
            if let Some(values) = bindings.get(name) {
                if let Some(val) = values.first() {
                    return val.clone();
                }
            }
            SExpr::Atom(Atom::String(format!("?{}", name)))
        }
        Pattern::VarRest(name) => {
            if let Some(values) = bindings.get(name) {
                if let Some(val) = values.first() {
                    return val.clone();
                }
            }
            SExpr::Atom(Atom::String(format!("?{}", name)))
        }
        Pattern::Wildcard | Pattern::WildcardRest => {
            // Wildcards in replacement position: emit a placeholder
            SExpr::Atom(Atom::String("?*".to_string()))
        }
        Pattern::List { tag, children } => {
            let mut new_children = Vec::new();
            if let Some(t) = tag {
                // If tag is a variable reference, substitute
                if t.starts_with('?') {
                    let name = &t[1..];
                    if let Some(values) = bindings.get(name) {
                        if let Some(SExpr::Atom(Atom::String(resolved_tag))) = values.first() {
                            new_children.push(SExpr::Atom(Atom::String(resolved_tag.clone())));
                        } else {
                            new_children.push(SExpr::Atom(Atom::String(t.clone())));
                        }
                    } else {
                        new_children.push(SExpr::Atom(Atom::String(t.clone())));
                    }
                } else {
                    new_children.push(SExpr::Atom(Atom::String(t.clone())));
                }
            }
            for child in children {
                new_children.push(build_replacement(child, bindings));
            }
            SExpr::List(new_children)
        }
    }
}

/// 2026-07-15: Find and replace all matches of a pattern in an S-expression tree.
/// Returns (new_tree, count_of_replacements)
pub fn replace_all(pattern: &Pattern, replacement: &Pattern, expr: &SExpr) -> (SExpr, usize) {
    let mut count = 0;
    let result = replace_recursive(pattern, replacement, expr, &mut count, false);
    (result, count)
}

fn replace_recursive(
    pattern: &Pattern,
    replacement: &Pattern,
    expr: &SExpr,
    count: &mut usize,
    in_replaced: bool,
) -> SExpr {
    // Try to match at this level (don't match inside a replacement)
    if !in_replaced {
        if let Some(bindings) = match_pattern(pattern, expr) {
            *count += 1;
            return build_replacement(replacement, &bindings);
        }
    }
    match expr {
        SExpr::List(children) => {
            let new_children: Vec<SExpr> = children
                .iter()
                .map(|c| replace_recursive(pattern, replacement, c, count, false))
                .collect();
            SExpr::List(new_children)
        }
        _ => expr.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_wildcard_matches_any() {
        let pat = parse_pattern("?*").unwrap();
        let expr = SExpr::Atom(Atom::Int(42));
        assert!(match_pattern(&pat, &expr).is_some());
        let expr2 = SExpr::List(vec![SExpr::Atom(Atom::String("hello".into()))]);
        assert!(match_pattern(&pat, &expr2).is_some());
    }

    #[test]
    fn test_pattern_var_binds() {
        let pat = parse_pattern("?x").unwrap();
        let expr = SExpr::Atom(Atom::Int(42));
        let bindings = match_pattern(&pat, &expr).unwrap();
        assert_eq!(bindings.len(), 1);
        assert!(bindings.contains_key("x"));
    }

    #[test]
    fn test_pattern_list_match_tag() {
        let pat = parse_pattern("(call ?*)").unwrap();
        let expr = SExpr::List(vec![
            SExpr::Atom(Atom::String("call".into())),
            SExpr::Atom(Atom::String("Sqrt#".into())),
        ]);
        assert!(match_pattern(&pat, &expr).is_some());
    }

    #[test]
    fn test_pattern_list_mismatch_tag() {
        let pat = parse_pattern("(call ?*)").unwrap();
        let expr = SExpr::List(vec![
            SExpr::Atom(Atom::String("defn".into())),
            SExpr::Atom(Atom::String("foo".into())),
        ]);
        assert!(match_pattern(&pat, &expr).is_none());
    }

    #[test]
    fn test_pattern_var_captures_value() {
        let pat = parse_pattern("(ident ?name)").unwrap();
        let expr = SExpr::List(vec![
            SExpr::Atom(Atom::String("ident".into())),
            SExpr::Atom(Atom::String("my_var".into())),
        ]);
        let bindings = match_pattern(&pat, &expr).unwrap();
        let captured = bindings.get("name").unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0], SExpr::Atom(Atom::String("my_var".into())));
    }

    #[test]
    fn test_exact_atom_match() {
        let pat = parse_pattern("\"hello\"").unwrap();
        let expr = SExpr::Atom(Atom::String("hello".into()));
        assert!(match_pattern(&pat, &expr).is_some());
    }

    #[test]
    fn test_exact_atom_mismatch() {
        let pat = parse_pattern("42").unwrap();
        let expr = SExpr::Atom(Atom::Int(99));
        assert!(match_pattern(&pat, &expr).is_none());
    }

    #[test]
    fn test_collect_matches() {
        let pat = parse_pattern("(ident ?*)").unwrap();
        let expr = SExpr::List(vec![
            SExpr::Atom(Atom::String("list".into())),
            SExpr::List(vec![
                SExpr::Atom(Atom::String("ident".into())),
                SExpr::Atom(Atom::String("a".into())),
            ]),
            SExpr::List(vec![
                SExpr::Atom(Atom::String("ident".into())),
                SExpr::Atom(Atom::String("b".into())),
            ]),
        ]);
        let matches = collect_matches(&pat, &expr);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_replace_all() {
        let pat = parse_pattern("(ident ?name)").unwrap();
        let repl = parse_pattern("(reuse ?name)").unwrap();
        let expr = SExpr::List(vec![
            SExpr::Atom(Atom::String("list".into())),
            SExpr::List(vec![
                SExpr::Atom(Atom::String("ident".into())),
                SExpr::Atom(Atom::String("x".into())),
            ]),
        ]);
        let (result, count) = replace_all(&pat, &repl, &expr);
        assert_eq!(count, 1);
        // Result should have the matched ident replaced: (list (reuse x))
        let result_str = crate::beast::sexpr::to_string(&result);
        assert!(result_str.contains("x"));
        assert!(result_str.contains("reuse"));
    }

    #[test]
    fn test_pattern_rest_wildcard() {
        let pat = parse_pattern("(call ?fn ??*)").unwrap();
        let expr = SExpr::List(vec![
            SExpr::Atom(Atom::String("call".into())),
            SExpr::Atom(Atom::String("printf".into())),
            SExpr::Atom(Atom::String("arg1".into())),
            SExpr::Atom(Atom::String("arg2".into())),
        ]);
        assert!(match_pattern(&pat, &expr).is_some());
    }

    #[test]
    fn test_pattern_rest_var() {
        let pat = parse_pattern("(call ?fn ?args)").unwrap();
        let expr = SExpr::List(vec![
            SExpr::Atom(Atom::String("call".into())),
            SExpr::Atom(Atom::String("foo".into())),
            SExpr::Atom(Atom::Int(42)),
        ]);
        let bindings = match_pattern(&pat, &expr);
        assert!(bindings.is_some());
    }

    #[test]
    fn test_roundtrip_pattern_tokenize() {
        let input = "(call ?fn (string ?msg))";
        let pat = parse_pattern(input).unwrap();
        let expr = SExpr::List(vec![
            SExpr::Atom(Atom::String("call".into())),
            SExpr::Atom(Atom::String("Print#".into())),
            SExpr::List(vec![
                SExpr::Atom(Atom::String("string".into())),
                SExpr::Atom(Atom::String("hello".into())),
            ]),
        ]);
        let bindings = match_pattern(&pat, &expr);
        assert!(bindings.is_some());
        let b = bindings.unwrap();
        assert!(b.contains_key("fn"));
        assert!(b.contains_key("msg"));
    }

    #[test]
    fn test_dynamic_tag_with_wildcard() {
        // (* ?x) — matches any tag with any child
        let pat = parse_pattern("(* ?x)").unwrap();
        let expr = SExpr::List(vec![
            SExpr::Atom(Atom::String("any_tag".into())),
            SExpr::Atom(Atom::Int(99)),
        ]);
        assert!(match_pattern(&pat, &expr).is_some());
    }

    #[test]
    fn test_replacement_with_var_substitution() {
        let pat = parse_pattern("(ident ?name)").unwrap();
        let repl = parse_pattern("(call ?name ?name)").unwrap();
        let expr = SExpr::List(vec![
            SExpr::Atom(Atom::String("ident".into())),
            SExpr::Atom(Atom::String("x".into())),
        ]);
        let bindings = match_pattern(&pat, &expr).unwrap();
        let result = build_replacement(&repl, &bindings);
        let result_str = crate::beast::sexpr::to_string(&result);
        assert!(result_str.contains("call"));
        assert!(result_str.contains("x"));
    }
}
