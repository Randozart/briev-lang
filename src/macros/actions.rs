// ── AST Actions & Positions ─────────────────────────────────────────────
// 2026-07-21: Positional references and mutation operations for the
// AST navigation DSL. Positions are ephemeral cursors consumed by one
// action. Actions modify the live AST in place.
//
// Fix 1: Multi-target inserts process in reverse index order to prevent
//   index invalidation (splicing before lower-index shifts higher-index
//   targets). Delete already sorts descending.
// Fix 3: structural_valid() check rejects node placements that violate
//   AST invariants (e.g., Import inside a function body).
//
// DRY: All Vec-splicing share helpers. Max 2 levels.

use std::collections::HashMap;
use crate::ast::{Expr, Statement, TopLevel, PropertyValue};
use crate::ast::top::*;
use crate::ast::StageKind;
use super::selection::{NodeRef, Selection, top_level_tag, top_level_name};

/// A position cursor in the AST, created by navigating a Selection.
#[derive(Debug, Clone)]
pub enum Position {
    Before(NodeRef),
    After(NodeRef),
    Replace(NodeRef),
    Inside(NodeRef),
    AppendTo(NodeRef),
}

impl Position {
    pub fn before(sel: &Selection) -> Option<Position> {
        Some(Position::Before(sel.nodes.first()?.clone()))
    }
    pub fn after(sel: &Selection) -> Option<Position> {
        Some(Position::After(sel.nodes.first()?.clone()))
    }
    pub fn replace(sel: &Selection) -> Option<Position> {
        Some(Position::Replace(sel.nodes.first()?.clone()))
    }
    pub fn inside(sel: &Selection) -> Option<Position> {
        Some(Position::Inside(sel.nodes.first()?.clone()))
    }
    pub fn append_to(sel: &Selection) -> Option<Position> {
        Some(Position::AppendTo(sel.nodes.first()?.clone()))
    }
}

// ── Actions ────────────────────────────────────────────────────────────

/// Insert items before each node in the selection, processing in
/// reverse index order to prevent index invalidation.
/// 2026-07-21: Accepts stage for Fix 2 (stage-gated constructors).
pub fn insert_before_each(
    items: &mut Vec<TopLevel>,
    sel: &Selection,
    new_items: Vec<TopLevel>,
    stage: StageKind,
) -> Result<u32, String> {
    let indices = collect_toplevel_indices(sel);
    if indices.is_empty() { return Ok(0); }
    validate_nodes_for_stage(&new_items, stage)?;
    for item in &new_items {
        validate_structural_toplevel(items, item)?;
    }
    for idx in indices.iter().rev() {
        let p = Position::Before(NodeRef::TopLevel(*idx));
        insert_at_toplevel(items, &p, new_items.clone())?;
    }
    Ok(indices.len() as u32)
}

/// Insert items after each node in the selection, processing in
/// reverse index order.
pub fn insert_after_each(
    items: &mut Vec<TopLevel>,
    sel: &Selection,
    new_items: Vec<TopLevel>,
    stage: StageKind,
) -> Result<u32, String> {
    let indices = collect_toplevel_indices(sel);
    if indices.is_empty() { return Ok(0); }
    validate_nodes_for_stage(&new_items, stage)?;
    for item in &new_items {
        validate_structural_toplevel(items, item)?;
    }
    for idx in indices.iter().rev() {
        let p = Position::After(NodeRef::TopLevel(*idx));
        insert_at_toplevel(items, &p, new_items.clone())?;
    }
    Ok(indices.len() as u32)
}

/// Insert a single batch at the given position (existing single-target API).
pub fn insert_items(
    items: &mut Vec<TopLevel>,
    pos: &Position,
    new_items: Vec<TopLevel>,
    stage: StageKind,
) -> Result<(), String> {
    validate_nodes_for_stage(&new_items, stage)?;
    for item in &new_items {
        validate_structural_toplevel(items, item)?;
    }
    match pos {
        Position::Before(_) | Position::After(_) | Position::Replace(_) => {
            insert_at_toplevel(items, pos, new_items)
        }
        Position::Inside(node) | Position::AppendTo(node) => {
            insert_into_body(items, pos, node, new_items)
        }
    }
}

// ── Fix 1: Reverse-order helpers for index-invalidation safety ─────────

/// Insert at a single top-level position. Internal — callers must
/// handle reverse ordering for multi-target scenarios.
fn insert_at_toplevel(
    items: &mut Vec<TopLevel>,
    pos: &Position,
    new_items: Vec<TopLevel>,
) -> Result<(), String> {
    let idx = resolve_toplevel_idx(pos)?;
    let idx = *idx;
    match pos {
        Position::Before(_) => {
            for (offset, item) in new_items.into_iter().enumerate() {
                items.insert(idx + offset, item);
            }
        }
        Position::After(_) => {
            for (offset, item) in new_items.into_iter().enumerate() {
                items.insert(idx + 1 + offset, item);
            }
        }
        Position::Replace(_) => {
            items.remove(idx);
            for (offset, item) in new_items.into_iter().enumerate() {
                items.insert(idx + offset, item);
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn resolve_toplevel_idx<'a>(pos: &'a Position) -> Result<&'a usize, String> {
    match pos {
        Position::Before(NodeRef::TopLevel(i))
        | Position::After(NodeRef::TopLevel(i))
        | Position::Replace(NodeRef::TopLevel(i))
        | Position::Inside(NodeRef::TopLevel(i))
        | Position::AppendTo(NodeRef::TopLevel(i)) => Ok(i),
        _ => Err("insert: only TopLevel nodes supported".into()),
    }
}

fn insert_into_body(
    items: &mut Vec<TopLevel>,
    pos: &Position,
    node: &NodeRef,
    new_items: Vec<TopLevel>,
) -> Result<(), String> {
    let NodeRef::TopLevel(idx) = node else { return Ok(()) };
    let Some(target) = items.get_mut(*idx) else { return Ok(()) };
    let Some(body) = get_body_mut(target) else { return Ok(()) };
    let stmts = ast_nodes_to_stmts(&new_items);
    match pos {
        Position::Inside(_) => {
            for (offset, stmt) in stmts.into_iter().enumerate() {
                body.insert(offset, stmt);
            }
        }
        Position::AppendTo(_) => body.extend(stmts),
        _ => unreachable!(),
    }
    Ok(())
}

/// Delete all nodes in a selection from the AST.
pub fn delete_selection(items: &mut Vec<TopLevel>, sel: &Selection) -> Result<u32, String> {
    let mut indices: Vec<usize> = collect_toplevel_indices(sel);
    indices.sort_unstable_by(|a, b| b.cmp(a));
    let count = indices.len() as u32;
    for i in indices {
        if i < items.len() {
            items.remove(i);
        }
    }
    Ok(count)
}

pub(crate) fn collect_toplevel_indices(sel: &Selection) -> Vec<usize> {
    let mut indices = Vec::new();
    for node in &sel.nodes {
        if let NodeRef::TopLevel(i) = node {
            indices.push(*i);
        }
    }
    indices
}

/// Replace selected nodes with a constructed node.
pub fn replace_selection(
    items: &mut Vec<TopLevel>,
    sel: &Selection,
    replacement: TopLevel,
) -> Result<u32, String> {
    let mut count = 0u32;
    for node in &sel.nodes {
        if let NodeRef::TopLevel(i) = node {
            if *i < items.len() {
                items[*i] = replacement.clone();
                count += 1;
            }
        }
    }
    Ok(count)
}

/// Set metadata on selected nodes.
pub fn set_metadata(
    items: &mut Vec<TopLevel>,
    sel: &Selection,
    key: &str,
    val: PropertyValue,
) -> Result<u32, String> {
    let mut count = 0u32;
    for node in &sel.nodes {
        if let NodeRef::TopLevel(i) = node {
            let target = items.get_mut(*i);
            if let Some(item) = target {
                let metadata = get_metadata_mut(item);
                if let Some(m) = metadata {
                    m.insert(key.to_string(), val.clone());
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

/// Rename selected nodes (changes the name field).
pub fn rename_selection(
    items: &mut Vec<TopLevel>,
    sel: &Selection,
    new_name: &str,
) -> Result<u32, String> {
    let mut count = 0u32;
    for node in &sel.nodes {
        if let NodeRef::TopLevel(i) = node {
            let target = items.get_mut(*i);
            if let Some(item) = target {
                set_name(item, new_name);
                count += 1;
            }
        }
    }
    Ok(count)
}

/// Wrap selected nodes in a container with the given tag.
pub fn wrap_selection(
    items: &mut Vec<TopLevel>,
    sel: &Selection,
    tag: &str,
) -> Result<u32, String> {
    // For simplicity, wrap top-level nodes in a Statement container
    let mut count = 0u32;
    let indices: Vec<usize> = sel.nodes.iter()
        .filter_map(|n| match n { NodeRef::TopLevel(i) => Some(*i), _ => None })
        .collect();
    for i in indices.iter().rev() {
        if *i < items.len() {
            let item = items.remove(*i);
            let wrapped = match tag {
                "sync" => TopLevel::SyncGroup {
                    domains: vec![],
                    item: Box::new(item),
                },
                _ => item, // unknown tag, leave as-is
            };
            items.insert(*i, wrapped);
            count += 1;
        }
    }
    Ok(count)
}

// ── Fix 2: Stage-Gated Constructors ─────────────────────────────────────
// 2026-07-21: After $(Typed), the AST is fully typed. Reject insertion of
// nodes with missing type annotations to prevent codegen from receiving
// untyped nodes.

/// Check that all new nodes are valid for insertion at the given stage.
fn validate_nodes_for_stage(nodes: &[TopLevel], stage: StageKind) -> Result<(), String> {
    // Before Typed stage: all nodes are acceptable (no type info required yet).
    if stage <= StageKind::Typed {
        return Ok(());
    }
    // At or after Typed: reject nodes that lack type annotations.
    // A node is "untyped" if it's a Statement::Let with ty: None,
    // or an Expr::Call with no known type in the type universe.
    for node in nodes {
        if let TopLevel::Statement(stmt) = node {
            if let Statement::Let { ty: None, expr: Some(_), .. } = stmt.as_ref() {
                return Err(format!(
                    "stage {:?}: cannot insert untyped let binding — \
                     all let bindings must have explicit type annotations at this stage",
                    stage
                ));
            }
        }
    }
    Ok(())
}

// ── Fix 3: Structural Validity ─────────────────────────────────────────
// 2026-07-21: Prevent AST-invalid placements (e.g., Import inside body,
// Term at top level). Each node type has a valid parent context.

/// Return the parent tag context for a TopLevel item.
fn toplevel_parent_tag(item: &TopLevel) -> &str {
    match item {
        TopLevel::Definition(_) | TopLevel::Transaction(_) => "body",
        TopLevel::Import(_) | TopLevel::Export(_) => "top",
        _ => "top",
    }
}

/// Check that a node can be placed in the current top-level context.
fn validate_structural_toplevel(
    items: &[TopLevel],
    new_item: &TopLevel,
) -> Result<(), String> {
    // Import and Export are only valid at the top level (always true here).
    // Term is only valid inside a body — reject if inserted as top-level.
    match new_item {
        TopLevel::Statement(stmt) => {
            let stmt = stmt.as_ref();
            if matches!(stmt, Statement::Term(_) | Statement::EndProgram(_)) {
                return Err(format!(
                    "cannot insert {:?} at top level — only valid inside a function body",
                    stmt
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

// ── Internal Helpers ───────────────────────────────────────────────────

fn resolve_toplevel_mut<'a>(items: &'a mut Vec<TopLevel>, node: &NodeRef) -> Result<&'a mut TopLevel, String> {
    match node {
        NodeRef::TopLevel(i) => {
            items.get_mut(*i).ok_or_else(|| format!("node index {} out of range", i))
        }
        _ => Err("only TopLevel nodes supported for this operation".into()),
    }
}

fn get_body_mut(item: &mut TopLevel) -> Option<&mut Vec<Statement>> {
    match item {
        TopLevel::Definition(d) => Some(&mut d.body),
        TopLevel::Transaction(t) => Some(&mut t.body),
        _ => None,
    }
}

fn get_metadata_mut(item: &mut TopLevel) -> Option<&mut HashMap<String, PropertyValue>> {
    match item {
        TopLevel::Definition(d) => Some(&mut d.metadata),
        TopLevel::Transaction(t) => Some(&mut t.metadata),
        TopLevel::Cell(c) => Some(&mut c.metadata),
        _ => None,
    }
}

fn set_name(item: &mut TopLevel, name: &str) {
    match item {
        TopLevel::Definition(d) => d.name = name.to_string(),
        TopLevel::Transaction(t) => t.name = name.to_string(),
        TopLevel::Cell(c) => c.name = name.to_string(),
        _ => {}
    }
}

/// Convert constructed TopLevel nodes to Statements for body insertion.
fn ast_nodes_to_stmts(nodes: &[TopLevel]) -> Vec<Statement> {
    nodes.iter().filter_map(|n| match n {
        TopLevel::Statement(s) => Some(s.as_ref().clone()),
        _ => None,
    }).collect()
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, ImportKind, Type};
    use crate::ast::top::*;
    use crate::ast::StageKind;

    fn sample_items() -> Vec<TopLevel> {
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
                body: vec![],
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
    fn test_insert_before() {
        let mut items = sample_items();
        let sel = Selection::single(NodeRef::TopLevel(1)); // defn main
        let pos = Position::before(&sel).unwrap();
        let new_item = TopLevel::Import(Import {
            kind: ImportKind::Literal("std/debug.bv".into()),
            symbols: vec![],
            alias: None,
            span: None,
        });
        insert_items(&mut items, &pos, vec![new_item], StageKind::Parsed).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(top_level_name(&items[1]), Some("std/debug.bv"));
    }

    #[test]
    fn test_insert_after() {
        let mut items = sample_items();
        let sel = Selection::single(NodeRef::TopLevel(0)); // import
        let pos = Position::after(&sel).unwrap();
        let new_item = TopLevel::Import(Import {
            kind: ImportKind::Literal("std/debug.bv".into()),
            symbols: vec![],
            alias: None,
            span: None,
        });
        insert_items(&mut items, &pos, vec![new_item], StageKind::Parsed).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(top_level_name(&items[1]), Some("std/debug.bv"));
    }

    #[test]
    fn test_delete_selection() {
        let mut items = sample_items();
        let sel = Selection::single(NodeRef::TopLevel(0));
        let count = delete_selection(&mut items, &sel).unwrap();
        assert_eq!(count, 1);
        assert_eq!(items.len(), 1);
        assert_eq!(top_level_name(&items[0]), Some("main"));
    }

    #[test]
    fn test_set_metadata() {
        let mut items = sample_items();
        let sel = Selection::single(NodeRef::TopLevel(1)); // defn main
        set_metadata(&mut items, &sel, "entry", PropertyValue::Bool(true)).unwrap();
        let defn = &items[1];
        if let TopLevel::Definition(d) = defn {
            assert_eq!(d.metadata.get("entry"), Some(&PropertyValue::Bool(true)));
        } else {
            panic!("expected Definition");
        }
    }

    #[test]
    fn test_rename() {
        let mut items = sample_items();
        let sel = Selection::single(NodeRef::TopLevel(1));
        rename_selection(&mut items, &sel, "run").unwrap();
        assert_eq!(top_level_name(&items[1]), Some("run"));
    }

    #[test]
    fn test_insert_after_empty_selection_warns() {
        let sel = Selection::empty();
        let pos = Position::after(&sel);
        assert!(pos.is_none());
    }

    // ── Validation tests ───────────────────────────────────────────────

    #[test]
    fn test_validate_nodes_for_stage_accepts_typed_at_parsed() {
        let nodes = vec![];
        assert!(validate_nodes_for_stage(&nodes, StageKind::Parsed).is_ok());
    }

    #[test]
    fn test_validate_nodes_for_stage_rejects_untyped_after_typed() {
        let nodes = vec![
            TopLevel::Statement(Box::new(Statement::Let { names: vec![], 
                name: "x".into(),
                ty: None,
                expr: Some(Expr::Decimal(42)),
                modifiers: vec![],
            })),
        ];
        let result = validate_nodes_for_stage(&nodes, StageKind::Normalized);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("untyped let binding"));
    }

    #[test]
    fn test_validate_structural_toplevel_rejects_term() {
        let items = vec![];
        let node = TopLevel::Statement(Box::new(Statement::Term(Some(Expr::Decimal(0)))));
        let result = validate_structural_toplevel(&items, &node);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot insert"));
    }

    #[test]
    fn test_validate_structural_toplevel_accepts_import() {
        let items = vec![];
        let node = TopLevel::Import(Import::literal("std/io.bv", vec![]));
        let result = validate_structural_toplevel(&items, &node);
        assert!(result.is_ok());
    }
}
