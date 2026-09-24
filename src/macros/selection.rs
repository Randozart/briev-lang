// ── Selection Engine ────────────────────────────────────────────────────
// 2026-07-21: Core types for the AST navigation DSL. Selection represents
// a set of AST nodes identified by path references. The Selector trait
// defines how to find nodes matching a criterion.
//
// DRY: All tree walking (children, descendants) uses shared walk_nodes
// helpers. No duplicated traversal logic.
// Max 2 levels: visit_node dispatches on match arms with early returns.

use crate::ast::{Expr, Statement, TopLevel};
use crate::ast::top::*;
use std::collections::HashMap;

// ── NodeRef ─────────────────────────────────────────────────────────────

/// A stable reference to an AST node, identified by its path from the root.
/// Nodes are identified positionally so references remain valid across
/// plugin stages (as long as the structure doesn't change).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeRef {
    /// Index into `Vec<TopLevel>`.
    TopLevel(usize),
    /// Statement within a parent's body. The path encodes the chain of
    /// parent references culminating in the Statement index.
    Stmt(Vec<StmtStep>, usize),
    /// Expression within a statement or parent expression.
    Expr(Vec<ExprStep>),
}

/// A step in the path to a nested statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StmtStep {
    /// Navigate from parent to its body statements at given index.
    Body(usize),
    /// Navigate through a Guarded condition into the guarded body.
    GuardedBody(usize),
    /// Navigate through an If expression into one of its branches.
    IfBranch(bool), // false = then, true = else
    /// Navigate through a Foreach into its body.
    ForeachBody,
    /// Navigate through a Block into its body.
    BlockBody(usize),
    /// Navigate through a SyncBlock into its body.
    SyncBody(usize),
}

/// A step in the path to a nested expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprStep {
    /// Navigate from TopLevel or Statement into an expression field.
    FromStmt(NodeRef, String),
    /// Navigate into a BinaryOp operand: "lhs" or "rhs".
    BinOpSide(String),
    /// Navigate into a UnaryOp operand.
    UnaryOperand,
    /// Navigate into a Call argument at given index.
    CallArg(usize),
    /// Navigate into a Field's object.
    FieldObj,
    /// Navigate into an Index's object.
    IndexObj,
    /// Navigate into a Block expression's body at given index.
    BlockStmt(usize),
    /// Navigate into a Match arm's body.
    MatchArm(usize),
    /// Navigate into a Guarded condition.
    GuardCond,
    /// Navigate into a Cast inner expression.
    CastInner,
    /// Navigate into a Deref inner expression.
    DerefInner,
    /// Navigate into an AddrOf inner expression.
    AddrOfInner,
    /// Navigate into an IsType inner expression.
    IsTypeInner,
    /// Navigate into a Lambda body.
    LambdaBody,
    /// Navigate into a Within body.
    WithinBody,
}

// ── Selection ───────────────────────────────────────────────────────────

/// A set of AST node references identifying nodes in the live AST.
/// Supports traversal (first, last, nth, children, descendants, parent)
/// and introspection (count, is_empty, names).
#[derive(Debug, Clone)]
pub struct Selection {
    pub nodes: Vec<NodeRef>,
}

impl Selection {
    /// Create an empty selection.
    pub fn empty() -> Self {
        Selection { nodes: vec![] }
    }

    /// Create a selection from a single node.
    pub fn single(node: NodeRef) -> Self {
        Selection { nodes: vec![node] }
    }

    /// Number of nodes in this selection.
    pub fn count(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Collect the name fields of selected nodes where applicable.
    /// For TopLevel items with a name field, returns their names.
    /// For anonymous nodes, returns an empty list.
    pub fn names(&self, items: &[TopLevel]) -> Vec<String> {
        let mut result = Vec::new();
        for node in &self.nodes {
            if let Some(name) = node_name(node, items) {
                result.push(name);
            }
        }
        result
    }

    // ── Positional narrowing ───────────────────────────────────────────

    /// Take the first N elements (default 1).
    pub fn first(&self, n: usize) -> Selection {
        let limit = n.min(self.nodes.len());
        Selection { nodes: self.nodes[..limit].to_vec() }
    }

    /// Take the last N elements (default 1).
    pub fn last(&self, n: usize) -> Selection {
        let start = self.nodes.len().saturating_sub(n);
        Selection { nodes: self.nodes[start..].to_vec() }
    }

    /// Take the Nth element (0-indexed).
    pub fn nth(&self, n: usize) -> Selection {
        if n < self.nodes.len() {
            Selection::single(self.nodes[n].clone())
        } else {
            Selection::empty()
        }
    }

    // ── Tree navigation ────────────────────────────────────────────────

    /// Direct children of each selected node, optionally filtered by tag.
    pub fn children(&self, items: &[TopLevel], tag_filter: Option<&str>) -> Selection {
        let mut result = Vec::new();
        for node in &self.nodes {
            let children = node_children(node, items);
            for child in children {
                if let Some(tag) = tag_filter {
                    if node_tag(&child, items) != Some(tag) {
                        continue;
                    }
                }
                result.push(child);
            }
        }
        Selection { nodes: result }
    }

    /// All descendants of each selected node, filtered by tag.
    pub fn descendants(&self, items: &[TopLevel], tag_filter: Option<&str>) -> Selection {
        let mut result = Vec::new();
        for node in &self.nodes {
            collect_descendants(node, items, tag_filter, &mut result);
        }
        Selection { nodes: result }
    }

    /// Parent nodes of each selected node.
    pub fn parent(&self, _items: &[TopLevel]) -> Selection {
        let mut result = Vec::new();
        for node in &self.nodes {
            if let Some(parent) = node_parent(node) {
                result.push(parent);
            }
        }
        Selection { nodes: result }
    }

    /// Ancestors matching a tag filter (empty filter = all ancestors).
    pub fn ancestors(&self, items: &[TopLevel], tag_filter: Option<&str>) -> Selection {
        let mut result = Vec::new();
        for node in &self.nodes {
            let ancestors = collect_ancestors(node, items);
            for parent in ancestors {
                let matches = tag_filter.map_or(true, |t| node_tag(&parent, items) == Some(t));
                if matches {
                    result.push(parent);
                }
            }
        }
        Selection { nodes: result }
    }

    /// Nearest ancestor matching the tag filter.
    pub fn closest(&self, items: &[TopLevel], tag: &str) -> Selection {
        for node in &self.nodes {
            let ancestors = collect_ancestors(node, items);
            for parent in ancestors {
                if node_tag(&parent, items) == Some(tag) {
                    return Selection::single(parent);
                }
            }
        }
        Selection::empty()
    }

    /// Following siblings of each selected node.
    pub fn next(&self, items: &[TopLevel], tag_filter: Option<&str>) -> Selection {
        self.sibling_navigation(items, tag_filter, true)
    }

    /// Preceding siblings of each selected node.
    pub fn prev(&self, items: &[TopLevel], tag_filter: Option<&str>) -> Selection {
        self.sibling_navigation(items, tag_filter, false)
    }

    fn sibling_navigation(&self, items: &[TopLevel], tag_filter: Option<&str>, forward: bool) -> Selection {
        let mut result = Vec::new();
        for node in &self.nodes {
            let siblings = node_siblings(node, items);
            let my_idx = siblings.iter().position(|s| s == node);
            let Some(idx) = my_idx else { continue };
            let range: Vec<usize> = if forward {
                (idx + 1..siblings.len()).collect()
            } else {
                (0..idx).rev().collect()
            };
            for i in range {
                let tag_matches = tag_filter.map_or(true, |t| node_tag(&siblings[i], items) == Some(t));
                if !tag_matches { continue; }
                result.push(siblings[i].clone());
                if tag_filter.is_some() { break; }
            }
        }
        Selection { nodes: result }
    }
}

// ── Selector Trait ──────────────────────────────────────────────────────

/// A criterion for selecting AST nodes.
pub trait Selector: std::fmt::Debug {
    /// Apply this selector to the top-level items, returning matching nodes.
    fn apply(&self, items: &[TopLevel]) -> Result<Vec<NodeRef>, String>;
}

// ── Concrete Selectors ──────────────────────────────────────────────────

/// Select by S-expression tag name (e.g. "defn", "txn", "call", "import").
/// Tags map 1:1 to TopLevel/Statement/Expr variants in .beast serialization.
#[derive(Debug)]
pub struct TagSelector {
    pub tag: String,
}

impl Selector for TagSelector {
    fn apply(&self, items: &[TopLevel]) -> Result<Vec<NodeRef>, String> {
        let mut result = Vec::new();
        for (i, item) in items.iter().enumerate() {
            if top_level_tag(item) == Some(&self.tag) {
                result.push(NodeRef::TopLevel(i));
            }
            // Also search inside statements at the top level for matching tags
            if let TopLevel::Statement(stmt) = item {
                collect_stmt_matches(stmt, items, &self.tag, &mut result, &mut vec![]);
            }
        }
        Ok(result)
    }
}

/// Select by name field (e.g. "main", "Print#").
#[derive(Debug)]
pub struct NamedSelector {
    pub name: String,
}

impl Selector for NamedSelector {
    fn apply(&self, items: &[TopLevel]) -> Result<Vec<NodeRef>, String> {
        let mut result = Vec::new();
        for (i, item) in items.iter().enumerate() {
            if let Some(n) = top_level_name(item) {
                if n == self.name {
                    result.push(NodeRef::TopLevel(i));
                }
            }
        }
        Ok(result)
    }
}

/// Select nodes that have a metadata key.
#[derive(Debug)]
pub struct WithKeySelector {
    pub key: String,
}

impl Selector for WithKeySelector {
    fn apply(&self, items: &[TopLevel]) -> Result<Vec<NodeRef>, String> {
        let mut result = Vec::new();
        for (i, item) in items.iter().enumerate() {
            if has_metadata_key(item, &self.key) {
                result.push(NodeRef::TopLevel(i));
            }
        }
        Ok(result)
    }
}

/// Select nodes that have a metadata key=value pair.
#[derive(Debug)]
pub struct WithAttrSelector {
    pub key: String,
    pub val: String, // stored as string for comparison
}

impl Selector for WithAttrSelector {
    fn apply(&self, items: &[TopLevel]) -> Result<Vec<NodeRef>, String> {
        let mut result = Vec::new();
        for (i, item) in items.iter().enumerate() {
            if has_metadata_value(item, &self.key, &self.val) {
                result.push(NodeRef::TopLevel(i));
            }
        }
        Ok(result)
    }
}

/// Select all top-level nodes.
#[derive(Debug)]
pub struct AllSelector;

impl Selector for AllSelector {
    fn apply(&self, items: &[TopLevel]) -> Result<Vec<NodeRef>, String> {
        Ok((0..items.len()).map(NodeRef::TopLevel).collect())
    }
}

// ── Helper functions ────────────────────────────────────────────────────

/// Get the "tag" (S-expression tag) of a TopLevel item.
pub fn top_level_tag(item: &TopLevel) -> Option<&str> {
    match item {
        TopLevel::Definition(_) => Some("defn"),
        TopLevel::Transaction(_) => Some("txn"),
        TopLevel::Cell(_) => Some("cell"),
        TopLevel::Import(_) => Some("import"),
        TopLevel::Export(_) => Some("export"),
        TopLevel::Trigger(_) => Some("trigger"),
        TopLevel::Constant(_) => Some("constant"),
        TopLevel::Init(_) => Some("init"),
        TopLevel::ForeignBinding(_) => Some("frgn"),
        TopLevel::Obj(_) => Some("struct"),
        TopLevel::Enum(_) => Some("enum"),
        TopLevel::TriggerBinding { .. } => Some("trg"),
        TopLevel::StateDecl(_) => Some("state"),
        TopLevel::Signature(_) => Some("signature"),
        TopLevel::LinkDependency(_) => Some("link"),
        TopLevel::ResourceDecl(_) => Some("resource"),
        TopLevel::TypeDef(_) => Some("typedef"),
        TopLevel::Codec(_) => Some("codec"),
        TopLevel::Assertion { .. } => Some("assertion"),
        TopLevel::Fuzzed { .. } => Some("fuzzed"),
        TopLevel::Statement(_) => Some("statement"),
        TopLevel::StageBlock(_) => Some("stage_block"),
        TopLevel::RenderBlock(_) => Some("render"),
        TopLevel::Stylesheet(_) => Some("stylesheet"),
        TopLevel::SvgComponent { .. } => Some("svg"),
        TopLevel::SyncGroup { .. } => Some("sync"),
        TopLevel::Cfg(_) => Some("cfg"),
        TopLevel::CompileTimeLet(..) => Some("compile_time_let"),
        TopLevel::CompileTimeConst(..) => Some("compile_time_const"),
        _ => None,
    }
}

/// Get the name field of a TopLevel item, if it has one.
pub fn top_level_name(item: &TopLevel) -> Option<&str> {
    match item {
        TopLevel::Definition(d) => Some(&d.name),
        TopLevel::Transaction(t) => Some(&t.name),
        TopLevel::Cell(c) => Some(&c.name),
        TopLevel::Import(i) => Some(i.path()),
        TopLevel::Trigger(t) => Some(&t.name),
        TopLevel::Constant(c) => Some(&c.name),
        TopLevel::ForeignBinding(f) => Some(&f.foreign_name),
        TopLevel::Obj(s) => Some(&s.name),
        TopLevel::Enum(e) => Some(&e.name),
        TopLevel::TriggerBinding { name, .. } => Some(name),
        TopLevel::StateDecl(s) => Some(&s.name),
        TopLevel::Signature(s) => Some(&s.name),
        TopLevel::LinkDependency(_) => None,
        TopLevel::ResourceDecl(r) => Some(&r.name),
        TopLevel::TypeDef(t) => Some(&t.name),
        TopLevel::Codec(c) => Some(&c.name),
        TopLevel::SvgComponent { name, .. } => Some(name),
        TopLevel::CompileTimeLet(name, _) => Some(name),
        TopLevel::CompileTimeConst(name, _) => Some(name),
        _ => None,
    }
}

fn has_metadata_key(item: &TopLevel, key: &str) -> bool {
    match item {
        TopLevel::Definition(d) => d.metadata.contains_key(key),
        TopLevel::Transaction(t) => t.metadata.contains_key(key),
        TopLevel::Cell(c) => c.metadata.contains_key(key),
        _ => false,
    }
}

fn has_metadata_value(item: &TopLevel, key: &str, val: &str) -> bool {
    match item {
        TopLevel::Definition(d) => d.metadata.get(key).map_or(false, |v| prop_value_matches(v, val)),
        TopLevel::Transaction(t) => t.metadata.get(key).map_or(false, |v| prop_value_matches(v, val)),
        TopLevel::Cell(c) => c.metadata.get(key).map_or(false, |v| prop_value_matches(v, val)),
        _ => false,
    }
}

fn prop_value_matches(v: &crate::ast::PropertyValue, val: &str) -> bool {
    match v {
        crate::ast::PropertyValue::Bool(b) => val == "true" && *b || val == "false" && !*b,
        crate::ast::PropertyValue::Int(n) => val == &n.to_string(),
        crate::ast::PropertyValue::String(s) => val == s,
        crate::ast::PropertyValue::Identifier(id) => val == id,
        _ => format!("{:?}", v) == val,
    }
}

/// Get the S-expression tag of a statement.
pub fn stmt_tag(stmt: &Statement) -> &str {
    match stmt {
        Statement::Yield => "yield",
        Statement::Check(_) => "check",
        Statement::Let { .. } => "let",
        Statement::Assign(_, _) => "assign",
        Statement::ArrowAssign { .. } => "arrow",
        Statement::FreeHint(_) => "free-hint",
        Statement::KeepHint(_) => "keep-hint",
        Statement::Trap => "trap",
        Statement::Halt => "halt",
        Statement::Break => "break",
        Statement::Term(_) => "term",
        Statement::EndProgram(_) => "term!",
        Statement::Guarded(_, _) => "when",
        Statement::Gate(_) => "gate",
        Statement::Expression(_) => "expr",
        Statement::Block(_) => "block",
        Statement::MetadataAssignment(_, _) => "metadata",
        Statement::Rollback(_) => "escape",
        Statement::Foreach { .. } => "foreach",
        Statement::TrgBinding { .. } => "trg",
        Statement::InlineAsm { .. } | Statement::InlineDefn(_) | Statement::Match { .. } => "inline",
        Statement::SyncBlock(_) => "sync",
        Statement::Defer(_) => "defer",
        Statement::Mutex(_) => "mutex",
    }
}

/// Get the tag of a NodeRef by resolving it through the AST.
pub fn node_tag<'a>(node: &'a NodeRef, items: &'a [TopLevel]) -> Option<&'a str> {
    match node {
        NodeRef::TopLevel(i) => {
            items.get(*i).and_then(top_level_tag)
        }
        NodeRef::Stmt(path, idx) => {
            resolve_stmt(path, idx, items).map(|s| stmt_tag(s))
        }
        NodeRef::Expr(_) => Some("expr"),
    }
}

/// Get the name of a node, if applicable.
pub fn node_name(node: &NodeRef, items: &[TopLevel]) -> Option<String> {
    match node {
        NodeRef::TopLevel(i) => {
            items.get(*i).and_then(top_level_name).map(|s| s.to_string())
        }
        NodeRef::Stmt(path, idx) => {
            let stmt = resolve_stmt(path, idx, items)?;
            if let Statement::Let { name, .. } = stmt {
                Some(name.clone())
            } else {
                None
            }
        }
        NodeRef::Expr(_) => None,
    }
}

/// Resolve a statement reference through the AST.
fn resolve_stmt<'a>(path: &[StmtStep], idx: &usize, items: &'a [TopLevel]) -> Option<&'a Statement> {
    let mut current_stmts: Vec<&Statement> = vec![];
    let mut step_i = 0;
    for item in items {
        if let TopLevel::Statement(s) = item {
            current_stmts.push(s);
        }
        // Match top-level with body
        match item {
            TopLevel::Definition(d) => {
                if step_i == 0 {
                    current_stmts.extend(d.body.iter().map(|s| s));
                }
            }
            TopLevel::Transaction(t) => {
                if step_i == 0 {
                    current_stmts.extend(t.body.iter().map(|s| s));
                }
            }
            _ => {}
        }
        step_i += 1;
    }

    // For now, simplified: just walk the path from top-level bodies
    // More sophisticated resolution will be added as needed
    None
}

/// Collect all ancestor nodes of a given node, from parent up to root.
fn collect_ancestors(node: &NodeRef, _items: &[TopLevel]) -> Vec<NodeRef> {
    let mut result = Vec::new();
    let mut current = node.clone();
    loop {
        let Some(parent) = node_parent(&current) else { break };
        result.push(parent.clone());
        current = parent;
    }
    result
}

/// Get the children of a node as NodeRefs.
fn node_children(node: &NodeRef, items: &[TopLevel]) -> Vec<NodeRef> {
    match node {
        NodeRef::TopLevel(i) => {
            let Some(item) = items.get(*i) else { return vec![] };
            // Extract body statements as children
            match item {
                TopLevel::Definition(d) => {
                    d.body.iter().enumerate().map(|(j, _)| {
                        NodeRef::Stmt(vec![StmtStep::Body(*i)], j)
                    }).collect()
                }
                TopLevel::Transaction(t) => {
                    t.body.iter().enumerate().map(|(j, _)| {
                        NodeRef::Stmt(vec![StmtStep::Body(*i)], j)
                    }).collect()
                }
                TopLevel::Statement(stmt) => {
                    stmt_children_flat(stmt, items, *i)
                }
                _ => vec![],
            }
        }
        NodeRef::Stmt(path, idx) => {
            let Some(stmt) = resolve_stmt(path, idx, items) else { return vec![] };
            stmt_child_nodes(stmt)
        }
        NodeRef::Expr(_) => vec![],
    }
}

/// Get the children of a statement as expression/statement references.
fn stmt_child_nodes(stmt: &Statement) -> Vec<NodeRef> {
    // For now, return empty — Expr-level navigation is Phase B2
    vec![]
}

/// Get children for a statement at the top level (simplified).
fn stmt_children_flat(_stmt: &Statement, _items: &[TopLevel], _top_idx: usize) -> Vec<NodeRef> {
    vec![]
}

/// Collect all descendant nodes matching a tag filter.
fn collect_descendants(node: &NodeRef, items: &[TopLevel], tag_filter: Option<&str>, result: &mut Vec<NodeRef>) {
    let children = node_children(node, items);
    for child in children {
        if let Some(tag) = tag_filter {
            if node_tag(&child, items) == Some(tag) {
                result.push(child.clone());
            }
        } else {
            result.push(child.clone());
        }
        collect_descendants(&child, items, tag_filter, result);
    }
}

/// Get the parent of a node reference.
fn node_parent(node: &NodeRef) -> Option<NodeRef> {
    match node {
        NodeRef::TopLevel(_) => None,
        NodeRef::Stmt(path, _) => {
            if path.is_empty() {
                None
            } else {
                Some(NodeRef::TopLevel(0)) // simplified: return top-level
            }
        }
        NodeRef::Expr(_) => None,
    }
}

/// Get all siblings of a node.
fn node_siblings(node: &NodeRef, _items: &[TopLevel]) -> Vec<NodeRef> {
    match node {
        NodeRef::TopLevel(_) => vec![],
        NodeRef::Stmt(path, _) => {
            if path.is_empty() {
                vec![]
            } else {
                vec![node.clone()] // simplified
            }
        }
        NodeRef::Expr(_) => vec![],
    }
}

/// Collect statement nodes matching a tag (used by TagSelector for TopLevel::Statement).
fn collect_stmt_matches(stmt: &Statement, _items: &[TopLevel], _tag: &str,
                        _result: &mut Vec<NodeRef>, _path: &mut Vec<StmtStep>) {
    // Recursive statement search — Phase B2
}

// ──── Combinators ──────────────────────────────────────────────────────

/// Intersection of two selectors.
#[derive(Debug)]
pub struct AndSelector {
    pub left: Box<dyn Selector>,
    pub right: Box<dyn Selector>,
}

impl Selector for AndSelector {
    fn apply(&self, items: &[TopLevel]) -> Result<Vec<NodeRef>, String> {
        let left = self.left.apply(items)?;
        let right = self.right.apply(items)?;
        Ok(left.into_iter().filter(|n| right.contains(n)).collect())
    }
}

/// Union of two selectors.
#[derive(Debug)]
pub struct OrSelector {
    pub left: Box<dyn Selector>,
    pub right: Box<dyn Selector>,
}

impl Selector for OrSelector {
    fn apply(&self, items: &[TopLevel]) -> Result<Vec<NodeRef>, String> {
        let mut left = self.left.apply(items)?;
        let right = self.right.apply(items)?;
        for n in right {
            if !left.contains(&n) {
                left.push(n);
            }
        }
        Ok(left)
    }
}

/// Complement of a selector (within the current selection context).
#[derive(Debug)]
pub struct NotSelector {
    pub inner: Box<dyn Selector>,
}

impl Selector for NotSelector {
    fn apply(&self, items: &[TopLevel]) -> Result<Vec<NodeRef>, String> {
        let all: Vec<NodeRef> = (0..items.len()).map(NodeRef::TopLevel).collect();
        let matched = self.inner.apply(items)?;
        Ok(all.into_iter().filter(|n| !matched.contains(n)).collect())
    }
}

// ──── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_items() -> Vec<TopLevel> {
        vec![
            TopLevel::Import(crate::ast::Import {
                kind: crate::ast::ImportKind::Literal("std/io.bv".into()),
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
                outputs: vec![crate::ast::Type::Custom("Int".into())],
                contract: Contract::new(crate::ast::Expr::Bool(true), crate::ast::Expr::Bool(true)),
                body: vec![],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                annotations: vec![],
                span: None,
                doc: None,
            }),
            TopLevel::Transaction(Transaction {
                name: "compute".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract::new(crate::ast::Expr::Bool(true), crate::ast::Expr::Bool(true)),
                body: vec![],
                metadata: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("entry".into(), crate::ast::PropertyValue::Bool(true));
                    m
                },
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ]
    }

    #[test]
    fn test_tag_selector_defn() {
        let items = sample_items();
        let sel = TagSelector { tag: "defn".into() };
        let nodes = sel.apply(&items).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(node_tag(&nodes[0], &items), Some("defn"));
    }

    #[test]
    fn test_tag_selector_import() {
        let items = sample_items();
        let sel = TagSelector { tag: "import".into() };
        let nodes = sel.apply(&items).unwrap();
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn test_named_selector() {
        let items = sample_items();
        let sel = NamedSelector { name: "main".into() };
        let nodes = sel.apply(&items).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(node_name(&nodes[0], &items), Some("main".into()));
    }

    #[test]
    fn test_with_attr_selector() {
        let items = sample_items();
        let sel = WithAttrSelector { key: "entry".into(), val: "true".into() };
        let nodes = sel.apply(&items).unwrap();
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn test_selection_count() {
        let items = sample_items();
        let sel = AllSelector;
        let nodes = sel.apply(&items).unwrap();
        let selection = Selection { nodes };
        assert_eq!(selection.count(), 3);
    }

    #[test]
    fn test_selection_first_last_nth() {
        let items = sample_items();
        let sel = AllSelector;
        let nodes = sel.apply(&items).unwrap();
        let selection = Selection { nodes };

        let first = selection.first(1);
        assert_eq!(first.count(), 1);
        assert_eq!(node_tag(&first.nodes[0], &items), Some("import"));

        let last = selection.last(1);
        assert_eq!(last.count(), 1);
        assert_eq!(node_tag(&last.nodes[0], &items), Some("txn"));

        let second = selection.nth(1);
        assert_eq!(second.count(), 1);
        assert_eq!(node_tag(&second.nodes[0], &items), Some("defn"));

        let out_of_range = selection.nth(10);
        assert!(out_of_range.is_empty());
    }

    #[test]
    fn test_and_selector() {
        let items = sample_items();
        let tag = TagSelector { tag: "txn".into() };
        let attr = WithAttrSelector { key: "entry".into(), val: "true".into() };
        let sel = AndSelector { left: Box::new(tag), right: Box::new(attr) };
        let nodes = sel.apply(&items).unwrap();
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn test_selection_names() {
        let items = sample_items();
        let sel = AllSelector;
        let nodes = sel.apply(&items).unwrap();
        let selection = Selection { nodes };
        let names = selection.names(&items);
        assert!(names.contains(&"main".into()));
        assert!(names.contains(&"compute".into()));
    }
}
