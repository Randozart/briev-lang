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

// ── Arena calling context ───────────────────────────────────────────────
//
// 2026-11-10 (plain Alloc#): a non-escaping Alloc# inside a helper called
// from a reactor transaction may use the per-tick arena even though the
// helper itself is not a transaction. Compute the transitive set of
// definitions reachable from any transaction; those definitions default to
// Arena (and are made stateful so the arena in %State is addressable).
fn arena_reachable_defns(items: &[TopLevel]) -> HashSet<String> {
    let mut cg = crate::analysis::call_graph::CallGraph::new();
    cg.build_from_program(items);
    let mut txns: Vec<String> = vec![];
    for item in items {
        if let TopLevel::Transaction(t) = item {
            txns.push(t.name.clone());
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = txns;
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur.clone()) {
            continue;
        }
        let callees = cg
            .edges_from(&cur)
            .or_else(|| cg.defn_edges_from(&cur))
            .cloned()
            .unwrap_or_default();
        for c in callees {
            stack.push(c);
        }
    }
    seen.retain(|n| {
        !items
            .iter()
            .any(|i| matches!(i, TopLevel::Transaction(t) if &t.name == n))
    });
    seen
}

// ── Interprocedural parameter-escape summaries ──────────────────────────
//
// 2026-11-10 (memory decisions): the allocation DAG used to see no calls at
// all, so a pointer passed to a helper that stores it into state was still
// classified non-escaping (arena → dangling after the tick). Model calls
// explicitly: a call argument escapes iff the callee's corresponding
// parameter escapes. Summaries are body-derived and iterated to a least
// fixed point over the call graph; unknown/foreign callees stay conservative.
//
// Borrowing intrinsics read/write through the pointer without retaining it.
// Everything else unknown escapes.

#[derive(Clone)]
struct EscapeBody {
    params: Vec<String>,
    body: Vec<Statement>,
}

fn is_borrowed_intrinsic(name: &str) -> bool {
    matches!(
        name,
        "Load#"
            | "Store#"
            | "Copy#"
            | "Fill#"
            | "Free#"
            | "CopyBytes#"
            | "Sqrt#"
            | "Sin#"
            | "Cos#"
            | "Fabs#"
            | "Ceil#"
            | "Floor#"
            | "Exp#"
            | "Pow#"
            | "Max#"
            | "Min#"
            | "Fma#"
            | "Popcount#"
            | "LeadingZeros#"
            | "TrailingZeros#"
    )
}

fn collect_escape_bodies(items: &[TopLevel]) -> HashMap<String, EscapeBody> {
    let mut out = HashMap::new();
    for item in items {
        match item {
            TopLevel::Definition(d) => {
                out.insert(
                    d.name.clone(),
                    EscapeBody {
                        params: d.parameters.iter().map(|(n, _)| n.clone()).collect(),
                        body: d.body.clone(),
                    },
                );
            }
            TopLevel::Transaction(t) => {
                out.insert(
                    t.name.clone(),
                    EscapeBody {
                        params: t.parameters.iter().map(|(n, _)| n.clone()).collect(),
                        body: t.body.clone(),
                    },
                );
            }
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    out.insert(
                        d.name.clone(),
                        EscapeBody {
                            params: d.parameters.iter().map(|(n, _)| n.clone()).collect(),
                            body: d.body.clone(),
                        },
                    );
                }
            }
            _ => {}
        }
    }
    out
}

fn state_field_names(items: &[TopLevel]) -> HashSet<String> {
    let mut names = HashSet::new();
    for item in items {
        match item {
            TopLevel::StateDecl(s) => {
                names.insert(s.name.clone());
            }
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, .. } = stmt.as_ref() {
                    names.insert(name.clone());
                }
            }
            _ => {}
        }
    }
    names
}

fn add_alias(aliases: &mut HashMap<String, HashSet<usize>>, name: &str, bits: HashSet<usize>) {
    aliases.insert(name.to_string(), bits);
}

fn expr_alias_set(
    expr: &Expr,
    aliases: &HashMap<String, HashSet<usize>>,
    summaries: &HashMap<String, Vec<bool>>,
    state_fields: &HashSet<String>,
    escaped: &mut HashSet<usize>,
) -> HashSet<usize> {
    match expr {
        Expr::Identifier(n) => aliases.get(n).cloned().unwrap_or_default(),
        Expr::Cast(e, _)
        | Expr::Deref(e)
        | Expr::AddrOf(e)
        | Expr::Field(e, _)
        | Expr::UnaryOp(_, e) => expr_alias_set(e, aliases, summaries, state_fields, escaped),
        Expr::Index(e, i) => {
            let mut out = expr_alias_set(e, aliases, summaries, state_fields, escaped);
            out.extend(expr_alias_set(i, aliases, summaries, state_fields, escaped));
            out
        }
        Expr::Slice {
            array,
            start,
            end,
            stride,
        } => {
            let mut out = expr_alias_set(array, aliases, summaries, state_fields, escaped);
            for e in [start, end, stride].into_iter().flatten() {
                out.extend(expr_alias_set(e, aliases, summaries, state_fields, escaped));
            }
            out
        }
        Expr::BinaryOp(_, l, r) => {
            let mut out = expr_alias_set(l, aliases, summaries, state_fields, escaped);
            out.extend(expr_alias_set(r, aliases, summaries, state_fields, escaped));
            out
        }
        Expr::Tuple(items) | Expr::List(items) => {
            let mut out = HashSet::new();
            for e in items {
                out.extend(expr_alias_set(e, aliases, summaries, state_fields, escaped));
            }
            out
        }
        Expr::StructLiteral { fields, specs, .. } => {
            let mut out = HashSet::new();
            for (_, e) in fields.iter().chain(specs.iter()) {
                out.extend(expr_alias_set(e, aliases, summaries, state_fields, escaped));
            }
            out
        }
        Expr::Call(name, args, _) => {
            note_call(name, args, aliases, summaries, state_fields, escaped);
            // A call result is not assumed to alias an argument unless the
            // callee returns it; the callee summary marks that argument and
            // the call site already escapes it.
            HashSet::new()
        }
        Expr::MethodCall(recv, _, args, _) => {
            // Unknown member: conservative escape of receiver and args.
            let recv_bits = expr_alias_set(recv, aliases, summaries, state_fields, escaped);
            escaped.extend(recv_bits);
            for a in args {
                let bits = expr_alias_set(a, aliases, summaries, state_fields, escaped);
                escaped.extend(bits);
            }
            HashSet::new()
        }
        Expr::If(cond, then, else_) => {
            let cond_bits = expr_alias_set(cond, aliases, summaries, state_fields, escaped);
            escaped.extend(cond_bits);
            let mut out = expr_alias_set(then, aliases, summaries, state_fields, escaped);
            if let Some(e) = else_ {
                out.extend(expr_alias_set(e, aliases, summaries, state_fields, escaped));
            }
            out
        }
        Expr::Match(scrut, arms) => {
            let scrut_bits = expr_alias_set(scrut, aliases, summaries, state_fields, escaped);
            escaped.extend(scrut_bits);
            let mut out = HashSet::new();
            for arm in arms {
                out.extend(expr_alias_set(
                    &arm.body,
                    aliases,
                    summaries,
                    state_fields,
                    escaped,
                ));
            }
            out
        }
        Expr::Block(stmts) => {
            let mut local = aliases.clone();
            for stmt in stmts {
                stmt_aliases(stmt, &mut local, summaries, state_fields, escaped);
            }
            HashSet::new()
        }
        Expr::Consume(e) | Expr::Await(e) | Expr::Within(e, _) => {
            expr_alias_set(e, aliases, summaries, state_fields, escaped)
        }
        Expr::Spawn { args, .. } => {
            let mut out = HashSet::new();
            for a in args {
                out.extend(expr_alias_set(a, aliases, summaries, state_fields, escaped));
            }
            out
        }
        _ => HashSet::new(),
    }
}

fn note_call(
    name: &str,
    args: &[Expr],
    aliases: &HashMap<String, HashSet<usize>>,
    summaries: &HashMap<String, Vec<bool>>,
    state_fields: &HashSet<String>,
    escaped: &mut HashSet<usize>,
) {
    if name == "Alloc#" || is_borrowed_intrinsic(name) || name == "Free#" {
        return;
    }
    match summaries.get(name) {
        Some(summary) => {
            for (i, arg) in args.iter().enumerate() {
                if summary.get(i).copied().unwrap_or(true) {
                    let bits = expr_alias_set(arg, aliases, summaries, state_fields, escaped);
                    escaped.extend(bits);
                }
            }
        }
        None => {
            // Unknown/foreign/intrinsic: no retention proof.
            for arg in args {
                let bits = expr_alias_set(arg, aliases, summaries, state_fields, escaped);
                escaped.extend(bits);
            }
        }
    }
}

fn stmt_aliases(
    stmt: &Statement,
    aliases: &mut HashMap<String, HashSet<usize>>,
    summaries: &HashMap<String, Vec<bool>>,
    state_fields: &HashSet<String>,
    escaped: &mut HashSet<usize>,
) {
    match stmt {
        Statement::Let {
            name,
            expr: Some(e),
            ..
        } => {
            let bits = expr_alias_set(e, aliases, summaries, state_fields, escaped);
            add_alias(aliases, name, bits);
        }
        Statement::Assign(lhs, rhs) => {
            let bits = expr_alias_set(rhs, aliases, summaries, state_fields, escaped);
            match lhs {
                Expr::Identifier(n) => {
                    if state_fields.contains(n) {
                        escaped.extend(bits);
                    } else {
                        add_alias(aliases, n, bits);
                    }
                }
                _ => escaped.extend(bits),
            }
        }
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Tail(e) => {
            let bits = expr_alias_set(e, aliases, summaries, state_fields, escaped);
            escaped.extend(bits);
        }
        Statement::Expression(e) => {
            let _ = expr_alias_set(e, aliases, summaries, state_fields, escaped);
        }
        Statement::Guarded(cond, body) => {
            let _ = expr_alias_set(cond, aliases, summaries, state_fields, escaped);
            // Guards can fire repeatedly: two passes reach the small fixpoint.
            for _ in 0..2 {
                for s in body {
                    stmt_aliases(s, aliases, summaries, state_fields, escaped);
                }
            }
        }
        Statement::Block(body)
        | Statement::Defer(body)
        | Statement::SyncBlock(body)
        | Statement::Mutex(body) => {
            for s in body {
                stmt_aliases(s, aliases, summaries, state_fields, escaped);
            }
        }
        _ => {}
    }
}

fn summarize_escapes(items: &[TopLevel]) -> HashMap<String, Vec<bool>> {
    let bodies = collect_escape_bodies(items);
    let state_fields = state_field_names(items);
    let mut summaries: HashMap<String, Vec<bool>> = bodies
        .iter()
        .map(|(name, b)| (name.clone(), vec![false; b.params.len()]))
        .collect();
    let mut changed = true;
    while changed {
        changed = false;
        for (name, body) in &bodies {
            let mut aliases: HashMap<String, HashSet<usize>> = HashMap::new();
            for (i, param) in body.params.iter().enumerate() {
                let mut bits = HashSet::new();
                bits.insert(i);
                aliases.insert(param.clone(), bits);
            }
            let mut escaped = HashSet::new();
            for stmt in &body.body {
                stmt_aliases(stmt, &mut aliases, &summaries, &state_fields, &mut escaped);
            }
            let mut next = vec![false; body.params.len()];
            for i in escaped {
                if i < next.len() {
                    next[i] = true;
                }
            }
            if summaries.get(name) != Some(&next) {
                summaries.insert(name.clone(), next);
                changed = true;
            }
        }
    }
    summaries
}

// ── Dataflow Graph ──────────────────────────────────────────────────────

type NodeId = usize;

enum DagNode {
    /// Alloc#(size) call with given analysis_id. Result stored in `producer`.
    Alloc { id: usize, producer: String },
    /// Variable assigned from an expression. `producer` gets value from `consumers`.
    Assign {
        target: String,
        source_vars: Vec<String>,
    },
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
            nodes: vec![],
            node_reaching_allocs: vec![],
            producers: HashMap::new(),
            consumers: HashMap::new(),
            alloc_nodes: vec![],
            alloc_sizes: HashMap::new(),
        }
    }

    /// Add a node that produces a result variable (Alloc, Assign).
    fn add_producer(&mut self, node: DagNode, result_var: &str) -> NodeId {
        let id = self.nodes.len();
        // 2026-11-10: link source variables to the producing assignment. The
        // old graph recorded `source_vars` but never added the consumer edge,
        // so an Alloc# result never reached its Return/Call sinks; transaction
        // allocations that escaped through a local stayed classified Arena.
        if let DagNode::Assign { source_vars, .. } = &node {
            for v in source_vars {
                self.consumers.entry(v.clone()).or_default().push(id);
            }
        }
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
    /// 2026-11-03: stamp `analysis_id` onto `Expr::Call(Alloc#, ..)` nodes.
    /// Exactly one pass may do this (the strategy pass whose map codegen reads);
    /// `analyze_arena_need` runs the same walk but must NOT re-stamp, or it
    /// overwrites codegen's ids with a different (per-function) counter and
    /// codegen looks up the wrong strategy.
    stamp_ids: bool,
    /// 2026-07-27: Track whether ANY Alloc# in this function uses Arena strategy.
    /// Set during walk_expr when default_strategy() returns Arena for an Alloc# call.
    /// Used by analyze_arena_need for transitive call-graph propagation.
    needs_arena: bool,
    /// 2026-11-10: per-callee parameter escape summary. A call argument
    /// escapes only at positions the callee can retain/return/store.
    call_escapes: &'a HashMap<String, Vec<bool>>,
    /// True when this defn is reachable from a transaction; non-escaping
    /// allocations may use the tick arena.
    arena_context: bool,
    /// Names of defns in arena context (looked up while walking items).
    arena_context_set: HashSet<String>,
}

impl<'a> DagBuilder<'a> {
    fn new(
        counter: &'a mut usize,
        result: &'a mut HashMap<usize, AllocStrategy>,
        in_txn: bool,
        in_bounded: bool,
        call_escapes: &'a HashMap<String, Vec<bool>>,
        arena_context: bool,
    ) -> Self {
        DagBuilder {
            graph: DataflowGraph::new(),
            counter,
            result,
            in_txn,
            in_bounded,
            stamp_ids: true,
            needs_arena: false,
            call_escapes,
            arena_context,
            arena_context_set: HashSet::new(),
        }
    }

    fn default_strategy(&self) -> AllocStrategy {
        if self.in_txn {
            AllocStrategy::Arena
        } else if self.in_bounded {
            AllocStrategy::Alloca
        } else {
            AllocStrategy::Malloc
        }
    }

    fn collect_var_names(&self, expr: &Expr) -> Vec<String> {
        let mut vars = vec![];
        self.collect_var_names_rec(expr, &mut vars);
        vars
    }

    fn collect_var_names_rec(&self, expr: &Expr, vars: &mut Vec<String>) {
        match expr {
            Expr::Identifier(name) => vars.push(name.clone()),
            Expr::BinaryOp(_, l, r) => {
                self.collect_var_names_rec(l, vars);
                self.collect_var_names_rec(r, vars);
            }
            Expr::UnaryOp(_, e) => self.collect_var_names_rec(e, vars),
            Expr::Field(e, _) => self.collect_var_names_rec(e, vars),
            Expr::Index(e, i) => {
                self.collect_var_names_rec(e, vars);
                self.collect_var_names_rec(i, vars);
            }
            Expr::Cast(e, _)
            | Expr::IsType(e, _)
            | Expr::Deref(e)
            | Expr::AddrOf(e)
            | Expr::Consume(e)
            | Expr::Await(e) => self.collect_var_names_rec(e, vars),
            Expr::Call(_, args, _) => {
                for a in args {
                    self.collect_var_names_rec(a, vars);
                }
            }
            Expr::Tuple(elems) | Expr::List(elems) => {
                for e in elems {
                    self.collect_var_names_rec(e, vars);
                }
            }
            Expr::If(cond, then, else_) => {
                self.collect_var_names_rec(cond, vars);
                self.collect_var_names_rec(then, vars);
                if let Some(e) = else_ {
                    self.collect_var_names_rec(e, vars);
                }
            }
            _ => {}
        }
    }

    /// Variables whose allocation the *return value* aliases. Unlike
    /// `collect_var_names`, an identifier used only as a non-escaping call
    /// argument does not make the result alias it (`term read_at(p)` returns a
    /// scalar; `term pass(p)` returns p because pass's parameter escapes).
    fn return_alias_vars(&self, expr: &Expr) -> Vec<String> {
        match expr {
            Expr::Identifier(n) => vec![n.clone()],
            Expr::Cast(e, _)
            | Expr::Consume(e)
            | Expr::Await(e)
            | Expr::Deref(e)
            | Expr::AddrOf(e)
            | Expr::UnaryOp(_, e)
            | Expr::Field(e, _) => self.return_alias_vars(e),
            Expr::BinaryOp(_, l, r) => {
                let mut out = self.return_alias_vars(l);
                out.extend(self.return_alias_vars(r));
                out
            }
            Expr::Index(e, i) => {
                let mut out = self.return_alias_vars(e);
                out.extend(self.return_alias_vars(i));
                out
            }
            Expr::Tuple(es) | Expr::List(es) => {
                let mut out = vec![];
                for e in es {
                    out.extend(self.return_alias_vars(e));
                }
                out
            }
            Expr::StructLiteral { fields, specs, .. } => {
                let mut out = vec![];
                for (_, e) in fields.iter().chain(specs.iter()) {
                    out.extend(self.return_alias_vars(e));
                }
                out
            }
            Expr::If(cond, then, else_) => {
                let mut out = self.return_alias_vars(cond);
                out.extend(self.return_alias_vars(then));
                if let Some(e) = else_ {
                    out.extend(self.return_alias_vars(e));
                }
                out
            }
            Expr::Match(scrut, arms) => {
                let mut out = self.return_alias_vars(scrut);
                for arm in arms {
                    out.extend(self.return_alias_vars(&arm.body));
                }
                out
            }
            Expr::Call(name, args, _) => {
                if name == "Alloc#" || is_borrowed_intrinsic(name) || name == "Free#" {
                    return vec![];
                }
                let mut out = vec![];
                match self.call_escapes.get(name) {
                    Some(summary) => {
                        for (i, a) in args.iter().enumerate() {
                            if summary.get(i).copied().unwrap_or(true) {
                                out.extend(self.return_alias_vars(a));
                            }
                        }
                    }
                    None => {
                        for a in args {
                            out.extend(self.return_alias_vars(a));
                        }
                    }
                }
                out
            }
            Expr::MethodCall(recv, _, args, _) => {
                let mut out = self.return_alias_vars(recv);
                for a in args {
                    out.extend(self.return_alias_vars(a));
                }
                out
            }
            _ => vec![],
        }
    }

    /// Does this expression contain an `Alloc#`, possibly under a cast or
    /// other transparent wrapper? `let p: Int = Alloc#(n) as Int;` parses as
    /// Cast(Call(Alloc#)), so matching only the top-level Call missed the
    /// producer link entirely.
    fn expr_contains_alloc(expr: &Expr) -> bool {
        match expr {
            Expr::Call(name, _, _) => name == "Alloc#",
            Expr::Cast(e, _)
            | Expr::Consume(e)
            | Expr::Await(e)
            | Expr::Deref(e)
            | Expr::AddrOf(e)
            | Expr::UnaryOp(_, e)
            | Expr::Field(e, _) => Self::expr_contains_alloc(e),
            Expr::BinaryOp(_, l, r) => Self::expr_contains_alloc(l) || Self::expr_contains_alloc(r),
            Expr::Index(e, i) => Self::expr_contains_alloc(e) || Self::expr_contains_alloc(i),
            Expr::Tuple(es) | Expr::List(es) => es.iter().any(Self::expr_contains_alloc),
            Expr::StructLiteral { fields, specs, .. } => fields
                .iter()
                .chain(specs.iter())
                .any(|(_, e)| Self::expr_contains_alloc(e)),
            _ => false,
        }
    }

    /// Follow forward from a node through the consumer graph.
    /// Returns all nodes reachable via variable flow.
    fn forward_nodes(&self, from: NodeId) -> Vec<NodeId> {
        let mut visited = HashSet::new();
        let mut queue = vec![from];
        let mut result = vec![];
        while let Some(nid) = queue.pop() {
            if !visited.insert(nid) {
                continue;
            }
            result.push(nid);
            match &self.graph.nodes[nid] {
                DagNode::Alloc { producer, .. }
                | DagNode::Assign {
                    target: producer, ..
                } => {
                    if let Some(consumers) = self.graph.consumers.get(producer) {
                        for &c in consumers {
                            queue.push(c);
                        }
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
                    self.arena_context = self.arena_context_set.contains(&defn.name);
                    self.in_txn = self.arena_context;
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
            if ids.is_empty() {
                continue;
            }
            let is_escape = match node {
                DagNode::Return { .. } | DagNode::StateWrite { .. } => true,
                DagNode::Call { arg_vars, .. } => arg_vars
                    .iter()
                    .any(|v| self.graph.producers.contains_key(v)),
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
                Statement::Let {
                    name,
                    expr: Some(e),
                    ..
                } => {
                    self.walk_expr(e);
                    let mut srcs = self.collect_var_names(e);
                    let is_alloc = Self::expr_contains_alloc(e);
                    if is_alloc {
                        for &nid in &self.graph.alloc_nodes {
                            let DagNode::Alloc { id, producer } = &self.graph.nodes[nid] else {
                                continue;
                            };
                            if self.result.contains_key(id) {
                                srcs.push(producer.clone());
                            }
                        }
                    }
                    let node = DagNode::Assign {
                        target: name.clone(),
                        source_vars: srcs.clone(),
                    };
                    self.graph.add_producer(node, name);
                }
                Statement::Let {
                    name, expr: None, ..
                } => {
                    self.graph.producers.insert(name.clone(), usize::MAX);
                }
                Statement::Assign(lhs, rhs) => {
                    self.walk_expr(rhs);
                    let rhs_vars = self.collect_var_names(rhs);
                    let lhs_expr: &Expr = lhs;
                    match lhs_expr {
                        Expr::Identifier(name) => {
                            let node = DagNode::Assign {
                                target: name.clone(),
                                source_vars: rhs_vars.clone(),
                            };
                            self.graph.add_producer(node, name);
                        }
                        Expr::Field(_, _) => {
                            // state.field = rhs — rhs variables escape
                            let node = DagNode::StateWrite {
                                var: rhs_vars.first().cloned().unwrap_or_default(),
                            };
                            self.graph.add_consumer(node, rhs_vars);
                        }
                        _ => {}
                    }
                }
                Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Tail(e) => {
                    let vars = self.return_alias_vars(e);
                    self.graph.add_consumer(
                        DagNode::Return {
                            var: vars.first().cloned().unwrap_or_default(),
                        },
                        vars,
                    );
                    self.walk_expr(e);
                }
                Statement::Expression(e) => {
                    self.walk_expr(e);
                }
                Statement::Guarded(cond, body) => {
                    self.walk_expr(cond);
                    self.walk_stmts(body);
                }
                Statement::Block(body) => {
                    self.walk_stmts(body);
                }
                _ => {}
            }
        }
    }

    fn walk_expr(&mut self, expr: &mut Expr) {
        match expr {
            // 2026-09-28 (E3): the `[*]` selector is a leaf — nothing to walk.
            Expr::Wildcard => {}
            Expr::Consume(inner) | Expr::Await(inner) => {
                self.walk_expr(inner);
            }
            Expr::Call(name, args, id) if name == "Alloc#" => {
                let analysis_id = *self.counter;
                *self.counter += 1;
                if self.stamp_ids {
                    *id = Some(analysis_id);
                }
                if let Some(size_expr) = args.first() {
                    self.graph
                        .alloc_sizes
                        .insert(analysis_id, size_expr.clone());
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
                let nid = self.graph.add_producer(
                    DagNode::Alloc {
                        id: analysis_id,
                        producer: producer.clone(),
                    },
                    &producer,
                );
                self.graph.alloc_nodes.push(nid);
                for a in args.iter_mut() {
                    self.walk_expr(a);
                }
            }
            // 2026-11-10: model calls explicitly. Only argument positions the
            // callee can retain (escape summary) force Malloc; a read/store
            // helper no longer turns every pointer argument into an escape.
            Expr::Call(name, args, _) => {
                for a in args.iter_mut() {
                    self.walk_expr(a);
                }
                let mut escaped_vars: Vec<String> = vec![];
                if name != "Alloc#" && !is_borrowed_intrinsic(name) && name != "Free#" {
                    match self.call_escapes.get(name) {
                        Some(summary) => {
                            for (i, a) in args.iter().enumerate() {
                                if summary.get(i).copied().unwrap_or(true) {
                                    escaped_vars.extend(self.collect_var_names(a));
                                }
                            }
                        }
                        None => {
                            for a in args.iter() {
                                escaped_vars.extend(self.collect_var_names(a));
                            }
                        }
                    }
                }
                if !escaped_vars.is_empty() {
                    let vars = escaped_vars.clone();
                    self.graph.add_consumer(
                        DagNode::Call {
                            name: name.clone(),
                            arg_vars: escaped_vars,
                        },
                        vars,
                    );
                }
            }
            Expr::BinaryOp(_, l, r) => {
                self.walk_expr(l);
                self.walk_expr(r);
            }
            Expr::UnaryOp(_, e) => self.walk_expr(e),
            Expr::Field(e, _) => self.walk_expr(e),
            Expr::Index(e, i) => {
                self.walk_expr(e);
                self.walk_expr(i);
            }
            Expr::Cast(e, _) | Expr::IsType(e, _) | Expr::Deref(e) | Expr::AddrOf(e) => {
                self.walk_expr(e)
            }
            Expr::Tuple(elems) | Expr::List(elems) => {
                for e in elems.iter_mut() {
                    self.walk_expr(e);
                }
            }
            Expr::If(cond, then, else_) => {
                self.walk_expr(cond);
                self.walk_expr(then);
                if let Some(e) = else_ {
                    self.walk_expr(e);
                }
            }
            Expr::Match(expr, arms) => {
                self.walk_expr(expr);
                for arm in arms.iter_mut() {
                    self.walk_expr(&mut arm.body);
                }
            }
            Expr::Block(stmts) => {
                self.walk_stmts(stmts);
            }
            Expr::Quoted(_)
            | Expr::TaggedQuotedLiteral(_, _)
            | Expr::Decimal(_)
            | Expr::TaggedLiteral(_, _)
            | Expr::Char(_)
            | Expr::Bool(_)
            | Expr::BeginProgram
            | Expr::Float(_)
            | Expr::Identifier(_)
            | Expr::Lambda(_, _)
            | Expr::Within(_, _)
            | Expr::DerivationBlock(_)
            | Expr::FormattingAnnotation(_)
            | Expr::StructLiteral { .. } => {}
            Expr::Field(recv, _) | Expr::Reflect(recv, _, _) => {
                self.walk_expr(recv);
            }
            Expr::MethodCall(recv, _, args, _) => {
                self.walk_expr(recv);
                for a in args {
                    self.walk_expr(a);
                }
            }
            Expr::PluginIntercept { args, .. } => {
                for a in args {
                    self.walk_expr(a);
                }
            }
            Expr::Exists(_) => {
                unreachable!("fn? only in stage eval")
            }
            Expr::Slice { .. } => {}
            Expr::Slice { .. } => {}
            Expr::Range { .. } => {}
            Expr::Spawn { args, .. } => {}
            Expr::UnitLiteral { .. } => {}
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
    let summaries = summarize_escapes(items);
    let arena_defns = arena_reachable_defns(items);
    let mut counter = 0usize;
    let mut result = HashMap::new();
    let mut builder = DagBuilder::new(&mut counter, &mut result, false, false, &summaries, false);
    builder.arena_context_set = arena_defns;
    builder.analyze(items);

    // Extract alloc_sizes before dropping builder (ends mutable borrow on result)
    let alloc_sizes = std::mem::take(&mut builder.graph.alloc_sizes);
    drop(builder);

    for id in result.keys().copied().collect::<Vec<_>>() {
        let strategy = result.get(&id).cloned().unwrap_or(AllocStrategy::Malloc);
        if strategy == AllocStrategy::Malloc {
            continue;
        }
        let Some(size_expr) = alloc_sizes.get(&id) else {
            continue;
        };
        let (Expr::Decimal(size_val) | Expr::TaggedLiteral(size_val, _)) = size_expr else {
            continue;
        };
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

    let summaries = summarize_escapes(items);
    let arena_defns = arena_reachable_defns(items);
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
        let mut builder = DagBuilder::new(
            &mut counter,
            &mut result_map,
            false,
            false,
            &summaries,
            false,
        );
        builder.arena_context_set = arena_defns.clone();
        // 2026-11-03: this pass only needs `needs_arena`; it must not overwrite
        // the ids stamped by analyze_alloc_strategies (codegen reads those).
        builder.stamp_ids = false;
        match item {
            TopLevel::Transaction(txn) => {
                builder.in_txn = true;
                builder.in_bounded = !matches!(txn.contract.post_condition, Expr::Bool(true));
                builder.walk_stmts(&mut txn.body);
            }
            TopLevel::Definition(defn) => {
                builder.arena_context = arena_defns.contains(&defn.name);
                builder.in_txn = builder.arena_context;
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
                Statement::Let {
                    names: vec![],
                    name: "buf".into(),
                    ty: None,
                    expr: Some(alloc_expr),
                    modifiers: vec![],
                },
                Statement::Term(Some(Expr::Identifier(var.into()))),
            ]
        } else {
            vec![
                Statement::Let {
                    names: vec![],
                    name: "buf".into(),
                    ty: None,
                    expr: Some(alloc_expr),
                    modifiers: vec![],
                },
                Statement::Term(None),
            ]
        };
        TopLevel::Definition(Definition {
            name: "f".into(),
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            type_params: vec![],
            contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body,
            derivation: None,
            metadata: Default::default(),
            modifiers: vec![],
            annotations: vec![],
            variadic_param: None,
            span: None,
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
            if let Statement::Let {
                expr: Some(Expr::Call(_, _, id)),
                ..
            } = &mut d.body[0]
            {
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

    fn parse_program(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().unwrap()
    }

    fn strategies(src: &str) -> Vec<AllocStrategy> {
        let mut items = parse_program(src);
        let mut out: Vec<AllocStrategy> =
            analyze_alloc_strategies(&mut items).into_values().collect();
        out.sort_by_key(|s| format!("{s:?}"));
        out
    }

    #[test]
    fn non_escaping_helper_keeps_txn_alloc_in_arena() {
        let src = r#"
defn read_at(p: Int) -> Int { term Load#(p, 8); };
txn make(n: Int) [n > 0][n > 0] -> Int {
    let p: Int = Alloc#(n) as Int;
    Store#(p, 42, 8);
    term read_at(p);
};
node entry [beginprogram][true] { Print#(make(16)); endprogram; };
"#;
        assert_eq!(strategies(src), vec![AllocStrategy::Arena]);
    }

    #[test]
    fn helper_alloc_called_from_txn_uses_arena() {
        let src = r#"
defn helper(n: Int) -> Int {
    let p: Int = Alloc#(n) as Int;
    Store#(p, 42, 8);
    term Load#(p, 8);
};
txn make(n: Int) [n > 0][n > 0] -> Int { term helper(n); };
node entry [beginprogram][true] { Print#(make(16)); endprogram; };
"#;
        assert_eq!(strategies(src), vec![AllocStrategy::Arena]);
    }

    #[test]
    fn helper_that_returns_pointer_forces_malloc() {
        let src = r#"
defn pass(p: Int) -> Int { term p; };
txn make(n: Int) [n > 0][n > 0] -> Int {
    let p: Int = Alloc#(n) as Int;
    term pass(p);
};
node entry [beginprogram][true] { Print#(make(16)); endprogram; };
"#;
        assert_eq!(strategies(src), vec![AllocStrategy::Malloc]);
    }

    #[test]
    fn helper_that_stores_into_state_forces_malloc() {
        let src = r#"
let saved: Int = 0;
defn save(p: Int) { saved = p; term; };
txn make(n: Int) [n > 0][n > 0] -> Int {
    let p: Int = Alloc#(n) as Int;
    save(p);
    term 0;
};
node entry [beginprogram][true] { Print#(make(16)); endprogram; };
"#;
        assert_eq!(strategies(src), vec![AllocStrategy::Malloc]);
    }

    #[test]
    fn aliased_pointer_return_forces_malloc() {
        let src = r#"
defn pass(p: Int) -> Int { let q: Int = p; term q; };
txn make(n: Int) [n > 0][n > 0] -> Int {
    let p: Int = Alloc#(n) as Int;
    term pass(p);
};
node entry [beginprogram][true] { Print#(make(16)); endprogram; };
"#;
        assert_eq!(strategies(src), vec![AllocStrategy::Malloc]);
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
