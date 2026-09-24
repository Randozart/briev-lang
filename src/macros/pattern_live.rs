// ── Live AST Pattern Engine ─────────────────────────────────────────────
// 2026-07-21: Matches .beast patterns directly against live AST types
// (TopLevel, Statement, Expr) without S-expression serialization.
//
// Reuses the Pattern enum from src/beast/pattern.rs for parsing.
// Max 2 levels. Flat dispatch with early returns.

use std::collections::HashMap;
use crate::ast::{Expr, Statement, TopLevel};
use crate::beast::pattern::{self, Pattern};
use super::selection::{NodeRef, Selection, node_tag, top_level_tag, stmt_tag, top_level_name};

/// A binding map from ?variable names to their matched AST values.
/// Values are stored as NodeRefs pointing into the live AST.
#[derive(Debug, Clone)]
pub struct LiveBindings {
    pub vars: HashMap<String, Vec<NodeRef>>,
}

impl LiveBindings {
    pub fn new() -> Self {
        LiveBindings { vars: HashMap::new() }
    }

    /// Get the first value bound to a variable.
    pub fn get(&self, name: &str) -> Option<&NodeRef> {
        self.vars.get(name)?.first()
    }

    /// Get all values bound to a variable.
    pub fn get_all(&self, name: &str) -> Option<&Vec<NodeRef>> {
        self.vars.get(name)
    }
}

/// A match result from applying a pattern to the live AST.
pub struct LiveMatch {
    pub bindings: LiveBindings,
    pub matched_node: NodeRef,
}

/// Try to match a compiled Pattern against a specific TopLevel node.
/// Returns bindings on success, None on failure.
pub fn match_top_level(pattern: &Pattern, item: &TopLevel, idx: usize) -> Option<LiveBindings> {
    let mut bindings = LiveBindings::new();
    if match_toplevel_recursive(pattern, item, &mut bindings).is_ok() {
        Some(bindings)
    } else {
        None
    }
}

/// Collect all TopLevel items matching a pattern across an entire program.
pub fn collect_matches_toplevel(pattern: &Pattern, items: &[TopLevel]) -> Vec<LiveMatch> {
    let mut results = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if let Some(bindings) = match_top_level(pattern, item, i) {
            results.push(LiveMatch {
                bindings,
                matched_node: NodeRef::TopLevel(i),
            });
        }
    }
    results
}

/// Find all TopLevel items and statements matching a pattern recursively.
pub fn collect_matches_recursive(pattern: &Pattern, items: &[TopLevel]) -> Vec<LiveMatch> {
    let mut results = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let node = NodeRef::TopLevel(i);
        if let Some(bindings) = match_top_level(pattern, item, i) {
            results.push(LiveMatch { bindings, matched_node: node.clone() });
        }
        collect_stmt_matches_in_body(pattern, item, i, &mut results);
    }
    results
}

fn collect_stmt_matches_in_body(
    pattern: &Pattern,
    item: &TopLevel,
    item_idx: usize,
    results: &mut Vec<LiveMatch>,
) {
    let Some(body) = get_toplevel_body(item) else { return };
    for (j, stmt) in body.iter().enumerate() {
        let stmt_node = NodeRef::Stmt(vec![], j);
        if let Some(bindings) = match_stmt(pattern, stmt) {
            results.push(LiveMatch { bindings, matched_node: stmt_node });
        }
    }
}

/// Match a pattern against a statement.
pub fn match_stmt(pattern: &Pattern, stmt: &Statement) -> Option<LiveBindings> {
    let mut bindings = LiveBindings::new();
    if match_stmt_recursive(pattern, stmt, &mut bindings).is_ok() {
        Some(bindings)
    } else {
        None
    }
}

// ── Recursive Matching Helpers ─────────────────────────────────────────

fn match_toplevel_recursive(
    pattern: &Pattern,
    item: &TopLevel,
    bindings: &mut LiveBindings,
) -> Result<(), ()> {
    match pattern {
        Pattern::Wildcard => Ok(()),
        Pattern::Var(name) => {
            // Can't bind without a NodeRef here — will be handled at collector level
            Ok(())
        }
        Pattern::Atom(expected) => {
            let tag = top_level_tag(item).ok_or(())?;
            if tag == expected { Ok(()) } else { Err(()) }
        }
        Pattern::List { tag, children } => {
            // Check tag
            if let Some(expected_tag) = tag {
                let actual_tag = top_level_tag(item).ok_or(())?;
                if actual_tag != expected_tag { return Err(()); }
            }
            // Match tag-implied children from the struct fields
            match_toplevel_children(children, item, bindings)
        }
        Pattern::WildcardRest | Pattern::VarRest(_) => Err(()),
    }
}

fn match_stmt_recursive(
    pattern: &Pattern,
    stmt: &Statement,
    bindings: &mut LiveBindings,
) -> Result<(), ()> {
    match pattern {
        Pattern::Wildcard => Ok(()),
        Pattern::Var(name) => Ok(()),
        Pattern::Atom(expected) => {
            let tag = stmt_tag(stmt);
            if tag == expected { Ok(()) } else { Err(()) }
        }
        Pattern::List { tag, children } => {
            if let Some(expected_tag) = tag {
                let actual_tag = stmt_tag(stmt);
                if actual_tag != expected_tag { return Err(()); }
            }
            match_stmt_children(children, stmt, bindings)
        }
        Pattern::WildcardRest | Pattern::VarRest(_) => Err(()),
    }
}

fn match_toplevel_children(
    children: &[Pattern],
    item: &TopLevel,
    bindings: &mut LiveBindings,
) -> Result<(), ()> {
    let fields: Vec<&str> = match item {
        TopLevel::Definition(d) => vec![&d.name],
        TopLevel::Transaction(t) => vec![&t.name],
        _ => return Ok(()),
    };
    for (i, child) in children.iter().enumerate() {
        if i >= fields.len() { break; }
        match_field_pattern(child, fields[i], bindings)?;
    }
    Ok(())
}

fn match_stmt_children(
    children: &[Pattern],
    stmt: &Statement,
    bindings: &mut LiveBindings,
) -> Result<(), ()> {
    let mut child_idx = 0;
    match stmt {
        Statement::Let { name, .. } => {
            let fields = vec![name.as_str()];
            for (i, pat) in children.iter().enumerate() {
                if i < fields.len() {
                    match_field_pattern(pat, fields[i], bindings)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn match_field_pattern(
    pattern: &Pattern,
    field_value: &str,
    bindings: &mut LiveBindings,
) -> Result<(), ()> {
    match pattern {
        Pattern::Wildcard => Ok(()),
        Pattern::Var(name) => {
            // Field values can't be stored as NodeRef — skip binding for now
            Ok(())
        }
        Pattern::Atom(expected) => {
            if field_value == expected { Ok(()) } else { Err(()) }
        }
        _ => Err(()),
    }
}

/// Get the body statements of a TopLevel item, if it has a body.
fn get_toplevel_body(item: &TopLevel) -> Option<&Vec<Statement>> {
    match item {
        TopLevel::Definition(d) => Some(&d.body),
        TopLevel::Transaction(t) => Some(&t.body),
        _ => None,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::top::*;
    use crate::ast::{Expr, Statement, Type};
    use crate::beast::pattern;

    fn sample_program() -> Vec<TopLevel> {
        vec![
            TopLevel::Import(Import {
                kind: ImportKind::Literal("std/io.bv".into()),
                symbols: vec![],
                alias: None,
                span: None,
            }),
            TopLevel::Definition(Definition {
                variadic_param: None,
                name: "main".into(),
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![Type::Custom("Int".into())],
                contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
                body: vec![
                    Statement::Term(Some(Expr::Decimal(0))),
                ],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                annotations: vec![],
                span: None,
                doc: None,
            }),
        ]
    }

    #[test]
    fn test_match_tag_wildcard() {
        let items = sample_program();
        let pat = pattern::parse_pattern("(?* ?*)").unwrap();
        let matches = collect_matches_recursive(&pat, &items);
        assert_eq!(matches.len(), 3); // import + defn + term statement
    }

    #[test]
    fn test_match_defn_tag() {
        let items = sample_program();
        let pat = pattern::parse_pattern("(defn ?name ?contract)").unwrap();
        let matches = collect_matches_toplevel(&pat, &items);
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_no_match_wrong_tag() {
        let items = sample_program();
        let pat = pattern::parse_pattern("(txn ?name)").unwrap();
        let matches = collect_matches_toplevel(&pat, &items);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_pattern_parse_and_match() {
        let pat = pattern::parse_pattern("(?* ?*)").unwrap();
        assert!(matches!(pat, crate::beast::pattern::Pattern::List { .. }));
    }
}
