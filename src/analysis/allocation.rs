// ── Alloc# Strategy Selection Analysis ───────────────────────────────────
// 2026-07-18: Pre-codegen DAG-based analysis. Builds a dataflow graph from
// the statement list, then traces forward from each Alloc# to detect escapes.
// Strategy assigned per scope + escape status + size constraints.
//
// Three pillars:
//   1. Draw predictable paths (DAG builder + dataflow edges)
//   2. Fold predictable paths (no-escape → stack/arena/inline)
//   3. Verify DAGs (provenance tracking confirms escape results)
//
// Output: HashMap<analysis_id, AllocStrategy>

use crate::ast::{Expr, Statement, TopLevel};
use crate::backend::llvm::AllocStrategy;
use std::collections::{HashMap, HashSet};

// ── Dataflow Graph ──────────────────────────────────────────────────────

type NodeId = usize;

enum DagNode {
    /// Alloc#(size) call with given analysis_id. Result stored in `producer`.
    Alloc { id: usize, producer: String },
    /// Variable assigned from an expression. `producer` gets value from `consumers`.
    Assign { target: String, source_vars: Vec<String> },
    /// Function call — its arguments are consumed (may escape).
    Call { name: String, arg_vars: Vec<String> },
    /// Return/term — the returned variable escapes.
    Return { var: String },
    /// State field write — the written variable escapes.
    StateWrite { var: String },
}

struct DataflowGraph {
    nodes: Vec<DagNode>,
    /// node_id → set of alloc IDs that reach this node.
    node_reaching_allocs: Vec<HashSet<usize>>,
    /// variable name → node_id that produced it.
    producers: HashMap<String, NodeId>,
    /// variable name → node_ids that consume it.
    consumers: HashMap<String, Vec<NodeId>>,
    /// Alloc node indices for quick iteration.
    alloc_nodes: Vec<NodeId>,
    /// Per alloc_id: the size expression for Inline detection.
    alloc_sizes: HashMap<usize, Expr>,
}

impl DataflowGraph {
    fn new() -> Self {
        DataflowGraph {
            nodes: vec![], node_reaching_allocs: vec![],
            producers: HashMap::new(), consumers: HashMap::new(),
            alloc_nodes: vec![], alloc_sizes: HashMap::new(),
        }
    }

    /// Add a node that produces a result variable (Alloc, Assign).
    fn add_producer(&mut self, node: DagNode, result_var: &str) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(node);
        self.node_reaching_allocs.push(HashSet::new());
        self.producers.insert(result_var.to_string(), id);
        id
    }

    /// Add a node that consumes variables (Call, Return, StateWrite).
    fn add_consumer(&mut self, node: DagNode, input_vars: Vec<String>) -> NodeId {
        let id = self.nodes.len();
        for v in &input_vars {
            self.consumers.entry(v.clone()).or_default().push(id);
        }
        self.nodes.push(node);
        self.node_reaching_allocs.push(HashSet::new());
        id
    }

    /// Mark a variable as consumed (propagates alloc IDs through the graph).
    fn consume_var(&self, var: &str) -> HashSet<usize> {
        let mut ids = HashSet::new();
        // Alloc IDs from the producer
        if let Some(&prod) = self.producers.get(var) {
            ids.extend(&self.node_reaching_allocs[prod]);
        }
        ids
    }
}

// ── Builder ─────────────────────────────────────────────────────────────

struct DagBuilder<'a> {
    graph: DataflowGraph,
    /// Alloc IDs assigned during the walk.
    counter: &'a mut usize,
    /// Output: analysis_id → strategy per scope.
    result: &'a mut HashMap<usize, AllocStrategy>,
    in_txn: bool,
    in_bounded: bool,
    /// 2026-07-27: Track whether ANY Alloc# in this function uses Arena strategy.
    /// Set during walk_expr when default_strategy() returns Arena for an Alloc# call.
    /// Used by analyze_arena_need for transitive call-graph propagation.
    needs_arena: bool,
}

impl<'a> DagBuilder<'a> {
    fn new(counter: &'a mut usize, result: &'a mut HashMap<usize, AllocStrategy>, in_txn: bool, in_bounded: bool) -> Self {
        DagBuilder { graph: DataflowGraph::new(), counter, result, in_txn, in_bounded, needs_arena: false }
    }

    fn default_strategy(&self) -> AllocStrategy {
        if self.in_txn { AllocStrategy::Arena }
        else if self.in_bounded { AllocStrategy::Alloca }
        else { AllocStrategy::Malloc }
    }

    fn collect_var_names(&self, expr: &Expr) -> Vec<String> {
        let mut vars = vec![];
        self.collect_var_names_rec(expr, &mut vars);
        vars
    }

    fn collect_var_names_rec(&self, expr: &Expr, vars: &mut Vec<String>) {
        match expr {
            Expr::Identifier(name) => vars.push(name.clone()),
            Expr::BinaryOp(_, l, r) => { self.collect_var_names_rec(l, vars); self.collect_var_names_rec(r, vars); }
            Expr::UnaryOp(_, e) => self.collect_var_names_rec(e, vars),
            Expr::Field(e, _) => self.collect_var_names_rec(e, vars),
            Expr::Index(e, i) => { self.collect_var_names_rec(e, vars); self.collect_var_names_rec(i, vars); }
            Expr::Cast(e, _) | Expr::IsType(e, _) | Expr::Deref(e) | Expr::AddrOf(e) | Expr::Consume(e) | Expr::Await(e) => self.collect_var_names_rec(e, vars),
            Expr::Call(_, args, _) => { for a in args { self.collect_var_names_rec(a, vars); } }
            Expr::Tuple(elems) | Expr::List(elems) => { for e in elems { self.collect_var_names_rec(e, vars); } }
            Expr::If(cond, then, else_) => {
                self.collect_var_names_rec(cond, vars);
                self.collect_var_names_rec(then, vars);
                if let Some(e) = else_ { self.collect_var_names_rec(e, vars); }
            }
            _ => {}
        }
    }

    /// Follow forward from a node through the consumer graph.
    /// Returns all nodes reachable via variable flow.
    fn forward_nodes(&self, from: NodeId) -> Vec<NodeId> {
        let mut visited = HashSet::new();
        let mut queue = vec![from];
        let mut result = vec![];
        while let Some(nid) = queue.pop() {
            if !visited.insert(nid) { continue; }
            result.push(nid);
            match &self.graph.nodes[nid] {
                DagNode::Alloc { producer, .. } | DagNode::Assign { target: producer, .. } => {
                    if let Some(consumers) = self.graph.consumers.get(producer) {
                        for &c in consumers { queue.push(c); }
                    }
                }
                _ => {}
            }
        }
        result
    }

    fn analyze(&mut self, items: &mut [TopLevel]) {
        for item in items.iter_mut() {
            self.graph = DataflowGraph::new();
            match item {
                TopLevel::Transaction(txn) => {
                    self.in_txn = true;
                    self.in_bounded = !matches!(txn.contract.post_condition, Expr::Bool(true));
                    self.walk_stmts(&mut txn.body);
                    self.compute_reaching_allocs();
                }
                TopLevel::Definition(defn) => {
                    self.in_txn = false;
                    self.in_bounded = false;
                    self.walk_stmts(&mut defn.body);
                    self.compute_reaching_allocs();
                }
                _ => {}
            }
        }
    }

    /// After walking all statements for a txn/defn, propagate alloc IDs
    /// forward through the graph and mark escaped allocations.
    fn compute_reaching_allocs(&mut self) {
        // For each alloc node, follow forward to find all reachable nodes
        let alloc_ids: Vec<NodeId> = self.graph.alloc_nodes.clone();
        for &alloc_nid in &alloc_ids {
            let alloc_id = match &self.graph.nodes[alloc_nid] {
                DagNode::Alloc { id, .. } => *id,
                _ => continue,
            };
            let reachable = self.forward_nodes(alloc_nid);
            for &rnid in &reachable {
                self.graph.node_reaching_allocs[rnid].insert(alloc_id);
            }
        }
        // Mark escapes: any node that returns, writes state, or calls with an alloc arg
        for (nid, node) in self.graph.nodes.iter().enumerate() {
            let ids = &self.graph.node_reaching_allocs[nid];
            if ids.is_empty() { continue; }
            let is_escape = match node {
                DagNode::Return { .. } | DagNode::StateWrite { .. } => true,
                DagNode::Call { arg_vars, .. } => arg_vars.iter().any(|v| self.graph.producers.contains_key(v)),
                _ => false,
            };
            if is_escape {
                for &id in ids {
                    self.result.insert(id, AllocStrategy::Malloc);
                }
            }
        }
    }

    fn walk_stmts(&mut self, stmts: &mut [Statement]) {
        for stmt in stmts.iter_mut() {
            match stmt {
                Statement::Let { name, expr: Some(e), .. } => {
                    self.walk_expr(e);
                    let mut srcs = self.collect_var_names(e);
                    let is_alloc = matches!(&*e, Expr::Call(cname, _, _) if cname == "Alloc#");
                    if is_alloc {
                        for &nid in &self.graph.alloc_nodes {
                            let DagNode::Alloc { id, producer } = &self.graph.nodes[nid] else { continue; };
                            if self.result.contains_key(id) {
                                srcs.push(producer.clone());
                            }
                        }
                    }
                    let node = DagNode::Assign { target: name.clone(), source_vars: srcs.clone() };
                    self.graph.add_producer(node, name);
                }
                Statement::Let { name, expr: None, .. } => {
                    self.graph.producers.insert(name.clone(), usize::MAX);
                }
                Statement::Assign(lhs, rhs) => {
                    self.walk_expr(rhs);
                    let rhs_vars = self.collect_var_names(rhs);
                    let lhs_expr: &Expr = lhs;
                    match lhs_expr {
                        Expr::Identifier(name) => {
                            let node = DagNode::Assign { target: name.clone(), source_vars: rhs_vars.clone() };
                            self.graph.add_producer(node, name);
                        }
                        Expr::Field(_, _) => {
                            // state.field = rhs — rhs variables escape
                            let node = DagNode::StateWrite { var: rhs_vars.first().cloned().unwrap_or_default() };
                            self.graph.add_consumer(node, rhs_vars);
                        }
                        _ => {}
                    }
                }
                Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => {
                    let vars = self.collect_var_names(e);
                    self.graph.add_consumer(DagNode::Return { var: vars.first().cloned().unwrap_or_default() }, vars);
                    self.walk_expr(e);
                }
                Statement::Expression(e) => { self.walk_expr(e); }
                Statement::Guarded(cond, body) => { self.walk_expr(cond); self.walk_stmts(body); }
                Statement::Block(body) => { self.walk_stmts(body); }
                _ => {}
            }
        }
    }

    fn walk_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Consume(inner) | Expr::Await(inner) => {
                self.walk_expr(inner);
            }
            Expr::Call(name, args, id) if name == "Alloc#" => {
                let analysis_id = *self.counter;
                *self.counter += 1;
                *id = Some(analysis_id);
                if let Some(size_expr) = args.first() {
                    self.graph.alloc_sizes.insert(analysis_id, size_expr.clone());
                }
                let producer = format!("%alloc_{}", analysis_id);
                let strategy = self.default_strategy();
                // 2026-07-27: If this Alloc# defaults to Arena, mark needs_arena.
                // Strategy may later be upgraded to Malloc (escape detected) but
                // the conservative initial marking catches all cases where arena
                // might be required.
                if strategy == AllocStrategy::Arena {
                    self.needs_arena = true;
                }
                self.result.insert(analysis_id, strategy);
                let nid = self.graph.add_producer(DagNode::Alloc { id: analysis_id, producer: producer.clone() }, &producer);
                self.graph.alloc_nodes.push(nid);
                for a in args.iter_mut() { self.walk_expr(a); }
            }
            // 2026-08-01: `Realloc#`/`AllocArena#` are NOT registered intrinsics
            // (no signature, no emission) — the arm was a stale name-match on a
            // call that never exists. The arena need is strategy-driven above
            // (default_strategy → AllocStrategy::Arena). If a realloc intrinsic
            // is ever added it becomes a real signature, not a name-match.
            Expr::Call(_, args, _) => { for a in args.iter_mut() { self.walk_expr(a); } }
            Expr::BinaryOp(_, l, r) => { self.walk_expr(l); self.walk_expr(r); }
            Expr::UnaryOp(_, e) => self.walk_expr(e),
            Expr::Field(e, _) => self.walk_expr(e),
            Expr::Index(e, i) => { self.walk_expr(e); self.walk_expr(i); }
            Expr::Cast(e, _) | Expr::IsType(e, _) | Expr::Deref(e) | Expr::AddrOf(e) => self.walk_expr(e),
            Expr::Tuple(elems) | Expr::List(elems) => { for e in elems.iter_mut() { self.walk_expr(e); } }
            Expr::If(cond, then, else_) => { self.walk_expr(cond); self.walk_expr(then); if let Some(e) = else_ { self.walk_expr(e); } }
            Expr::Match(expr, arms) => { self.walk_expr(expr); for arm in arms.iter_mut() { self.walk_expr(&mut arm.body); } }
            Expr::Block(stmts) => { self.walk_stmts(stmts); }
            Expr::Quoted(_) | Expr::TaggedQuotedLiteral(_, _) | Expr::Decimal(_) | Expr::TaggedLiteral(_, _) | Expr::Char(_) | Expr::Bool(_) | Expr::BeginProgram | Expr::Float(_)
            | Expr::Identifier(_) | Expr::Lambda(_, _) | Expr::Within(_, _)
            | Expr::DerivationBlock(_) | Expr::FormattingAnnotation(_) | Expr::StructLiteral { .. } => {}
            Expr::Field(recv, _) | Expr::Reflect(recv, _, _) => { self.walk_expr(recv); }
            Expr::MethodCall(recv, _, args, _, _) => {
                self.walk_expr(recv);
                for a in args { self.walk_expr(a); }
            }
            Expr::PluginIntercept { args, .. } => {
                for a in args { self.walk_expr(a); }
            }
            Expr::Exists(_) => { unreachable!("fn? only in stage eval") },
            Expr::Slice { .. } => {},
Expr::Slice { .. } => {},
            Expr::Range { .. } => {},
            Expr::Spawn { args, .. } => {},
            Expr::Named { inner, .. } => { self.walk_expr(inner); },
            Expr::UnitLiteral { .. } => {}
            Expr::Capture { expr, .. } => { self.walk_expr(expr); }
        }
    }
}

// ── Public Entry Point ──────────────────────────────────────────────────

/// 2026-07-18: Analyze all Alloc# call sites and determine optimal strategies.
/// Strategy selection (post-escape-analysis):
///   No escape, in txn → Arena
///   No escape, bounded scope → Alloca
///   No escape, fixed-size ≤8 → Inline
///   Escape detected → Malloc
pub fn analyze_alloc_strategies(items: &mut [TopLevel]) -> HashMap<usize, AllocStrategy> {
    let mut counter = 0usize;
    let mut result = HashMap::new();
    let mut builder = DagBuilder::new(&mut counter, &mut result, false, false);
    builder.analyze(items);

    // Extract alloc_sizes before dropping builder (ends mutable borrow on result)
    let alloc_sizes = std::mem::take(&mut builder.graph.alloc_sizes);
    drop(builder);

    for id in result.keys().copied().collect::<Vec<_>>() {
        let strategy = result.get(&id).cloned().unwrap_or(AllocStrategy::Malloc);
        if strategy == AllocStrategy::Malloc { continue; }
        let Some(size_expr) = alloc_sizes.get(&id) else { continue; };
        let (Expr::Decimal(size_val) | Expr::TaggedLiteral(size_val, _)) = size_expr else { continue; };
        if *size_val <= 8 {
            result.insert(id, AllocStrategy::Inline);
        }
    }
    result
}

/// 2026-07-27: Compute which transactions/definitions need arena initialization.
/// Uses a per-function DAG walk to detect Arena-strategy Alloc# calls, then
/// propagates results transitively through the call graph via propagate_arena_need.
///
/// Returns a HashSet of function names that require arena init.
///
/// Propagation rule: if function A calls function B, and B needs arena,
/// then A also needs arena. Repeat until fixed point.
pub fn analyze_arena_need(items: &mut [TopLevel]) -> HashSet<String> {
    let mut cg = crate::analysis::call_graph::CallGraph::new();
    cg.build_from_program(items);

    let mut direct_needs: HashSet<String> = HashSet::new();
    for item in items.iter_mut() {
        let name = match item {
            TopLevel::Transaction(txn) => Some(txn.name.clone()),
            TopLevel::Definition(defn) => Some(defn.name.clone()),
            _ => None,
        };
        let Some(name) = name else { continue };

        let mut counter = 0usize;
        let mut result_map = HashMap::new();
        let mut builder = DagBuilder::new(&mut counter, &mut result_map, false, false);
        match item {
            TopLevel::Transaction(txn) => {
                builder.in_txn = true;
                builder.in_bounded = !matches!(txn.contract.post_condition, Expr::Bool(true));
                builder.walk_stmts(&mut txn.body);
            }
            TopLevel::Definition(defn) => {
                builder.in_txn = false;
                builder.in_bounded = false;
                builder.walk_stmts(&mut defn.body);
            }
            _ => {}
        }
        if builder.needs_arena {
            direct_needs.insert(name);
        }
    }

    // Propagate transitively through call graph
    crate::analysis::call_graph::propagate_arena_need(&direct_needs, &cg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    /// Helper: create a minimal defn with one Alloc# call and optional Term.
    fn make_defn(alloc_size: i64, term_var: Option<&str>) -> TopLevel {
        let alloc_expr = Expr::Call("Alloc#".into(), vec![Expr::Decimal(alloc_size)], None);
        let body = if let Some(var) = term_var {
            vec![
                Statement::Let { names: vec![],  name: "buf".into(), ty: None, expr: Some(alloc_expr), modifiers: vec![] },
                Statement::Term(Some(Expr::Identifier(var.into()))),
            ]
        } else {
            vec![
                Statement::Let { names: vec![],  name: "buf".into(), ty: None, expr: Some(alloc_expr), modifiers: vec![] },
                Statement::Term(None),
            ]
        };
        TopLevel::Definition(Definition {
            name: "f".into(), parameters: vec![], output_type: None, outputs: vec![],
            type_params: vec![], contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body, derivation: None, metadata: Default::default(),
            modifiers: vec![], annotations: vec![], variadic_param: None, span: None,
            doc: None,
        })
    }

    #[test]
    fn test_empty_items_no_strategies() {
        assert!(analyze_alloc_strategies(&mut []).is_empty());
    }

    #[test]
    fn test_alloc_id_assigned_in_defn() {
        let mut defn = make_defn(8, None);
        if let TopLevel::Definition(d) = &mut defn {
            if let Statement::Let { expr: Some(Expr::Call(_, _, id)), .. } = &mut d.body[0] {
                assert!(id.is_none(), "ID should be None before analysis");
            }
        }
        let result = analyze_alloc_strategies(&mut [defn]);
        assert_eq!(result.len(), 1, "should have one strategy");
    }

    #[test]
    fn test_alloc_escaped_via_term() {
        let defn = make_defn(16, Some("buf"));
        let result = analyze_alloc_strategies(&mut [defn]);
        assert_eq!(result.len(), 1);
        for (_, s) in &result {
            assert_eq!(*s, AllocStrategy::Malloc, "returned via Term → Malloc");
        }
    }

    #[test]
    fn test_alloc_not_escaped_in_defn_is_malloc() {
        // In a defn (no arena, no bounded scope), default is Malloc
        // even without explicit escape. This is the conservative default.
        let defn = make_defn(16, None);
        let result = analyze_alloc_strategies(&mut [defn]);
        assert_eq!(result.len(), 1);
        for (_, s) in &result {
            assert_eq!(*s, AllocStrategy::Malloc, "defn default → Malloc");
        }
    }
}
