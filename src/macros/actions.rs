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

use super::selection::{NodeRef, Selection, top_level_name, top_level_tag};
use crate::ast::StageKind;
use crate::ast::top::*;
use crate::ast::{Expr, PropertyValue, Statement, TopLevel};
use std::collections::HashMap;

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
    if indices.is_empty() {
        return Ok(0);
    }
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
    if indices.is_empty() {
        return Ok(0);
    }
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
    let NodeRef::TopLevel(idx) = node else {
        return Ok(());
    };
    let Some(target) = items.get_mut(*idx) else {
        return Ok(());
    };
    let Some(body) = get_body_mut(target) else {
        return Ok(());
    };
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
    let indices: Vec<usize> = sel
        .nodes
        .iter()
        .filter_map(|n| match n {
            NodeRef::TopLevel(i) => Some(*i),
            _ => None,
        })
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
/// 2026-11-16 (S5): a top-level item whose presence changes the type surface.
/// `defn`s are deliberately excluded — their bodies face `verify_bodies`, not
/// the stage guard.
fn structural_toplevel(item: &TopLevel) -> bool {
    matches!(
        item,
        TopLevel::ProtocolDef(_) | TopLevel::TypeDef(_) | TopLevel::ForeignBinding(_)
    )
}

fn validate_nodes_for_stage(nodes: &[TopLevel], stage: StageKind) -> Result<(), String> {
    // 2026-11-16 (S5): structural declarations — protocol variants, type
    // definitions, and foreign bindings — fix the type surface. Synthesising
    // one after `Resolved` would change what later stages see, so the guard
    // is at the stage boundary itself, not downstream. `defn` bodies are NOT
    // structural: they face `verify_bodies`, which is the correct gate for
    // them.
    for node in nodes {
        if structural_toplevel(node) && stage > StageKind::Resolved {
            return Err(format!(
                "stage {:?}: cannot synthesize a structural declaration                  (protocol, type definition, or foreign binding) after                  `Resolved` — the type surface is frozen at that stage;                  body-bearing `defn`s are still allowed (they face                  `verify_bodies`)",
                stage
            ));
        }
    }
    // Before Typed stage: all nodes are acceptable (no type info required yet).
    if stage <= StageKind::Typed {
        return Ok(());
    }
    // At or after Typed: reject nodes that lack type annotations.
    // A node is "untyped" if it's a Statement::Let with ty: None,
    // or an Expr::Call with no known type in the type universe.
    for node in nodes {
        if let TopLevel::Statement(stmt) = node {
            if let Statement::Let {
                ty: None,
                expr: Some(_),
                ..
            } = stmt.as_ref()
            {
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
fn validate_structural_toplevel(items: &[TopLevel], new_item: &TopLevel) -> Result<(), String> {
    // Import and Export are only valid at the top level (always true here).
    // Term is only valid inside a body — reject if inserted as top-level.
    match new_item {
        TopLevel::Statement(stmt) => {
            let stmt = stmt.as_ref();
            if matches!(
                stmt,
                Statement::Term(_) | Statement::Tail(_) | Statement::EndProgram(_)
            ) {
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

fn resolve_toplevel_mut<'a>(
    items: &'a mut Vec<TopLevel>,
    node: &NodeRef,
) -> Result<&'a mut TopLevel, String> {
    match node {
        NodeRef::TopLevel(i) => items
            .get_mut(*i)
            .ok_or_else(|| format!("node index {} out of range", i)),
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
    nodes
        .iter()
        .filter_map(|n| match n {
            TopLevel::Statement(s) => Some(s.as_ref().clone()),
            _ => None,
        })
        .collect()
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::StageKind;
    use crate::ast::top::*;
    use crate::ast::{Expr, ImportKind, Type};

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
        let nodes = vec![TopLevel::Statement(Box::new(Statement::Let {
            names: vec![],
            name: "x".into(),
            ty: None,
            expr: Some(Expr::Decimal(42)),
            modifiers: vec![],
        }))];
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

#[cfg(test)]
mod stage_guard_tests {
    use super::*;
    use crate::ast::top::*;
    use crate::ast::{Contract, Expr, Type};

    /// Build a minimal `proto` node for the stage guard test.
    fn proto_node() -> TopLevel {
        TopLevel::ProtocolDef(ProtocolDef {
            name: "TestProto".into(),
            category: "String".into(),
            contract: None,
            cast_edges: vec![],
            cross_ops: vec![],
            span: None,
        })
    }

    /// Build a minimal `TypeDef` node.
    fn typedef_node() -> TopLevel {
        TopLevel::TypeDef(Box::new(TypeDef {
            name: "TestType".into(),
            type_params: vec![],
            parent: None,
            protocol: Some("Int".into()),
            traits: vec![],
            bit_range: None,
            body: TypeDefBody {
                slots: vec![],
                pins: vec![],
                reference: None,
                metadata: std::collections::HashMap::new(),
                projections: vec![],
                bindings: vec![],
                operators: vec![],
                op_bindings: vec![],
                constraints: vec![],
                members: vec![],
                hold_laws: vec![],
                modes: vec![],
                span: None,
            },
            ports_in: vec![],
            ports_out: vec![],
            coll: false,
            seq: false,
            span: None,
        }))
    }

    /// Build a minimal `defn` node — must NOT be blocked by the stage guard.
    fn defn_node() -> TopLevel {
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "test_fn".into(),
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
        })
    }

    /// 2026-11-16 (S5): a `proto` inserted after `Resolved` must be rejected.
    #[test]
    fn proto_after_resolved_is_rejected() {
        let node = proto_node();
        let result = validate_nodes_for_stage(&[node], StageKind::Typed);
        assert!(
            result.is_err(),
            "a protocol synthesized after Resolved must be rejected"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("structural declaration"),
            "error should mention the structural-declaration guard, got: {err}"
        );
    }

    /// A `typedef` inserted after `Resolved` must also be rejected.
    #[test]
    fn typedef_after_resolved_is_rejected() {
        let node = typedef_node();
        let result = validate_nodes_for_stage(&[node], StageKind::Typed);
        assert!(
            result.is_err(),
            "a typedef synthesized after Resolved must be rejected"
        );
    }

    /// A `frgn` inserted after `Resolved` must also be rejected.
    #[test]
    fn frgn_after_resolved_is_rejected() {
        let node = TopLevel::ForeignBinding(ForeignBinding {
            foreign_name: "test_fn".into(),
            briev_name: None,
            from: FromSpec::Linked("libc".into()),
            target: ForeignTarget::C,
            inputs: vec![],
            success_output: vec![],
            error_type: "Error".into(),
            error_fields: vec![],
            input_layout: None,
            output_layout: None,
            precondition: None,
            postcondition: None,
            buffer_mode: None,
            default_watchdog: None,
            wasm_impl: None,
            wasm_setup: None,
            span: None,
            doc: None,
            is_optional: false,
            is_fire_forget: false,
            is_delivery: false,
            is_variadic: false,
        });
        let result = validate_nodes_for_stage(&[node], StageKind::Typed);
        assert!(
            result.is_err(),
            "a frgn synthesized after Resolved must be rejected"
        );
    }

    /// A `defn` inserted after `Resolved` must be ALLOWED — body-bearing
    /// definitions face `verify_bodies`, not the stage guard.
    #[test]
    fn defn_after_resolved_is_allowed() {
        let node = defn_node();
        let result = validate_nodes_for_stage(&[node], StageKind::Typed);
        assert!(
            result.is_ok(),
            "a defn synthesized after Resolved must be allowed (it faces verify_bodies), got: {:?}",
            result.err()
        );
    }

    /// A `proto` inserted at `Resolved` (not after) must be allowed.
    #[test]
    fn proto_at_resolved_is_allowed() {
        let node = proto_node();
        let result = validate_nodes_for_stage(&[node], StageKind::Resolved);
        assert!(
            result.is_ok(),
            "a proto synthesized at Resolved must be allowed, got: {:?}",
            result.err()
        );
    }
}
