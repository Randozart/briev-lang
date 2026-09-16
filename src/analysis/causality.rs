// ── Causal DAG pass ────────────────────────────────────────────────────
// 2026-09-12 (plan 2026-09-12-dynamics-causal-dag.md): the compile-time
// wiring the June 2026 dirty-flag design specified. `A → B` iff A writes
// what B reads; each edge is PROVEN (`post(A)` entails every `pre(B)`
// conjunct, with A-preserved conjuncts established by `pre(A)`) or WEAK
// (overlap only — reported, never fused). Cyclic components whose members
// declare no completion in their posts are refused: an unproven cycle
// needs a checkable liveness obligation, and a trivial `[true]` post is
// no obligation. This is the v1 contract-trust rule; the Z3 fixpoint
// engine is the future deep verifier (documented gap: one substantive
// post in a multi-node cycle masks an otherwise-oscillating component).
//
// Semantics this encodes (locked with the author): pre = eligibility,
// post = the declaration of completion. The program signals intent; the
// compiler derives the machinery.

use crate::ast::{BinaryOpKind, Expr, TopLevel};
use std::collections::BTreeSet;

/// Soundness level of a derived causal edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeProof {
    /// `post(A)` (+ A-preserved `pre(A)` facts) entails every `pre(B)`
    /// conjunct — B fires as a consequence of A (fusible, future slice).
    Proven,
    /// Write/read overlap only — the dependency is real but the enabling
    /// is not provable v1. Reported; never fused.
    Weak,
}

/// One derived causal edge.
#[derive(Debug, Clone)]
pub struct CausalEdge {
    pub from: String,
    pub to: String,
    pub proof: EdgeProof,
}

/// One reactive node in the causal graph.
#[derive(Debug, Clone)]
pub struct CausalNode {
    pub name: String,
    pub writes: BTreeSet<String>,
    pub reads: BTreeSet<String>,
    /// Field comparisons stated by the pre / post (entailment input).
    pub pre_facts: Vec<FieldFact>,
    pub post_facts: Vec<FieldFact>,
    /// The post declares a completion (not `[true]`/vacuous).
    pub post_declares_completion: bool,
}

/// The derived causal graph.
#[derive(Debug, Default)]
pub struct CausalGraph {
    pub nodes: Vec<CausalNode>,
    pub edges: Vec<CausalEdge>,
    /// Liveness refusals — cyclic components with no declared completion.
    pub refusals: Vec<String>,
}

/// Run the pass over a program. Additive: errors surface via `refusals`;
/// the caller decides whether they are fatal.
pub fn run(items: &[TopLevel]) -> CausalGraph {
    let mut nodes = collect_nodes(items);
    nodes.sort_by(|a, b| a.name.cmp(&b.name));

    let mut edges = Vec::new();
    for a in &nodes {
        for b in &nodes {
            let overlap: BTreeSet<String> =
                a.writes.intersection(&b.reads).cloned().collect();
            if overlap.is_empty() {
                continue;
            }
            let proof = if entails(&a.post_facts, &a.pre_facts, &b.pre_facts, &a.writes) {
                EdgeProof::Proven
            } else {
                EdgeProof::Weak
            };
            edges.push(CausalEdge { from: a.name.clone(), to: b.name.clone(), proof });
        }
    }
    edges.sort_by(|x, y| (&x.from, &x.to).cmp(&(&y.from, &y.to)));

    let mut graph = CausalGraph { nodes, edges, refusals: Vec::new() };
    check_cycles(&mut graph);
    graph
}

// ── Node collection ────────────────────────────────────────────────────

fn collect_nodes(items: &[TopLevel]) -> Vec<CausalNode> {
    let mut out = Vec::new();
    for item in items {
        let txn = match item {
            TopLevel::Transaction(t) if t.is_reactive => t,
            TopLevel::SyncGroup { item: inner, .. } => match inner.as_ref() {
                TopLevel::Transaction(t) if t.is_reactive => t,
                _ => continue,
            },
            _ => continue,
        };
        let writes: BTreeSet<String> =
            crate::backend::collect_assigned_identifiers(&txn.body)
                .into_iter()
                .collect();
        let mut reads: BTreeSet<String> =
            crate::backend::collect_read_identifiers(&txn.body)
                .into_iter()
                .collect();
        expr_identifiers(&txn.contract.pre_condition, &mut reads);
        expr_identifiers(&txn.contract.post_condition, &mut reads);
        let post = &txn.contract.post_condition;
        out.push(CausalNode {
            name: txn.name.clone(),
            writes,
            reads,
            pre_facts: field_facts(&txn.contract.pre_condition),
            post_facts: field_facts(post),
            post_declares_completion: !matches!(post, Expr::Bool(true))
                && !crate::proof_engine::is_vacuously_true(post),
        });
    }
    out
}

// ── Cycle classification + liveness refusal ───────────────────────────

/// Detect cyclic components (SCCs with a back edge; self-loops included).
/// A component where NO member declares completion in its post carries no
/// liveness obligation — refuse. Where at least one member declares
/// completion, v1 trusts the contract.
fn check_cycles(g: &mut CausalGraph) {
    let n = g.nodes.len();
    let pos = |name: &str| g.nodes.iter().position(|x| x.name == name);
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &g.edges {
        if let (Some(f), Some(t)) = (pos(&e.from), pos(&e.to)) {
            adj[f].push(t);
        }
    }

    let sccs = tarjan_scc(n, &adj);

    for comp in sccs {
        let is_cyclic = comp.len() > 1 || adj[comp[0]].contains(&comp[0]);
        if !is_cyclic {
            continue;
        }
        let any_completion = comp.iter().any(|&i| g.nodes[i].post_declares_completion);
        if !any_completion {
            let mut names: Vec<&str> =
                comp.iter().map(|&i| g.nodes[i].name.as_str()).collect();
            names.sort_unstable();
            g.refusals.push(format!(
                "the reactive cycle ({}) has no provable convergence and no liveness \
                 obligation — none of its nodes declares a completion in a \
                 postcondition, so it may never quiesce between events. fix: state \
                 the completion in the nodes' postconditions (e.g. [mode == \
                 Mode::Alarm], [done == true]), or make the exit condition \
                 explicit in a pre.",
                names.join(", ")
            ));
        }
    }
}

/// Tarjan SCC (recursive — node counts are reactive txns per program,
/// dozens at most). Returns components.
fn tarjan_scc(n: usize, adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct State {
        index: Vec<usize>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        counter: usize,
        sccs: Vec<Vec<usize>>,
    }
    const UNSET: usize = usize::MAX;

    fn finalize_component(v: usize, st: &mut State) {
        if st.low[v] != st.index[v] {
            return;
        }
        let mut comp = Vec::new();
        while let Some(w) = st.stack.pop() {
            st.on_stack[w] = false;
            comp.push(w);
            if w == v {
                break;
            }
        }
        st.sccs.push(comp);
    }

    fn relax(v: usize, w: usize, st: &mut State) {
        if st.on_stack[w] {
            st.low[v] = st.low[v].min(st.index[w]);
        }
    }

    fn dfs(v: usize, adj: &[Vec<usize>], st: &mut State) {
        st.index[v] = st.counter;
        st.low[v] = st.counter;
        st.counter += 1;
        st.stack.push(v);
        st.on_stack[v] = true;
        for &w in &adj[v] {
            if st.index[w] == UNSET {
                dfs(w, adj, st);
                st.low[v] = st.low[v].min(st.low[w]);
            } else {
                relax(v, w, st);
            }
        }
        finalize_component(v, st);
    }

    let mut st = State {
        index: vec![UNSET; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        counter: 0,
        sccs: Vec::new(),
    };
    for v in 0..n {
        if st.index[v] == UNSET {
            dfs(v, adj, &mut st);
        }
    }
    st.sccs
}

// ── Entailment (edge proof) ────────────────────────────────────────────

/// One field comparison extracted from a contract expression.
#[derive(Debug, Clone)]
pub struct FieldFact {
    pub field: String,
    pub op: BinaryOpKind,
    pub value: f64,
}

/// Extract `field op literal` conjuncts from an expression (flat &&-split).
pub fn field_facts(expr: &Expr) -> Vec<FieldFact> {
    let mut out = Vec::new();
    for c in crate::proof_engine::split_and(expr) {
        if let Some(f) = as_field_fact(c) {
            out.push(f);
        }
    }
    out
}

fn as_field_fact(e: &Expr) -> Option<FieldFact> {
    let Expr::BinaryOp(op, l, r) = e else { return None };
    let cmp = matches!(
        op,
        BinaryOpKind::Eq
            | BinaryOpKind::Neq
            | BinaryOpKind::Lt
            | BinaryOpKind::Gt
            | BinaryOpKind::Le
            | BinaryOpKind::Ge
    );
    if !cmp {
        return None;
    }
    let (field, lit) = match (l.as_ref(), r.as_ref()) {
        (Expr::Identifier(f), Expr::Float(v)) => (f.clone(), *v),
        (Expr::Identifier(f), Expr::Decimal(v)) => (f.clone(), *v as f64),
        (Expr::Float(v), Expr::Identifier(f)) => (f.clone(), *v),
        (Expr::Decimal(v), Expr::Identifier(f)) => (f.clone(), *v as f64),
        (Expr::Bool(b), Expr::Identifier(f)) | (Expr::Identifier(f), Expr::Bool(b)) => {
            (f.clone(), if *b { 1.0 } else { 0.0 })
        }
        _ => return None,
    };
    Some(FieldFact { field, op: *op, value: lit })
}

/// Conservative structural entailment: every `next` conjunct must be
/// entailed by `curr` facts (A's post — fields A rewrote) or, for fields
/// A does NOT rewrite (A preserves them), by A's own `prior` facts. A
/// rewritten field cannot vouch for itself through the pre — a self-edge
/// is PROVEN only if the post alone re-establishes the pre. Conjuncts on
/// fields with no stated fact anywhere (calls, triggers, enum literals)
/// are not provable v1 → the edge is WEAK.
pub fn entails(
    curr: &[FieldFact],
    prior: &[FieldFact],
    next: &[FieldFact],
    rewritten: &BTreeSet<String>,
) -> bool {
    next.iter().all(|n| {
        let by_curr = curr.iter().any(|c| c.field == n.field && dominates(c, n));
        let by_prior = !rewritten.contains(&n.field)
            && prior.iter().any(|p| p.field == n.field && dominates(p, n));
        by_curr || by_prior
    })
}

/// Does fact `a` entail fact `n` (same field)?
fn dominates(a: &FieldFact, n: &FieldFact) -> bool {
    use BinaryOpKind::*;
    let (v, w) = (a.value, n.value);
    match a.op {
        Eq => match n.op {
            Eq => v == w,
            Neq => v != w,
            Ge => v >= w,
            Le => v <= w,
            Gt => v > w,
            Lt => v < w,
            _ => false,
        },
        Ge => match n.op {
            Ge => v >= w,
            Gt => v > w,
            Neq => v > w,
            _ => false,
        },
        Gt => match n.op {
            Ge | Gt => v >= w,
            Neq => v >= w,
            _ => false,
        },
        Le => match n.op {
            Le => v <= w,
            Lt => v < w,
            Neq => v < w,
            _ => false,
        },
        Lt => match n.op {
            Le | Lt => v <= w,
            Neq => v <= w,
            _ => false,
        },
        Neq => match n.op {
            Neq => v == w,
            _ => false,
        },
        _ => false,
    }
}

// ── Report ─────────────────────────────────────────────────────────────

/// The "what fires into what" report — one line per node, sorted.
pub fn explain(graph: &CausalGraph) -> Vec<String> {
    let mut lines = Vec::new();
    for n in &graph.nodes {
        let mut proven = Vec::new();
        let mut weak = Vec::new();
        for e in &graph.edges {
            if e.from != n.name {
                continue;
            }
            match e.proof {
                EdgeProof::Proven => proven.push(e.to.clone()),
                EdgeProof::Weak => weak.push(e.to.clone()),
            }
        }
        lines.push(format!(
            "node {}: fires into [proven: {}] [weak: {}] — post {} completion",
            n.name,
            join_or(proven),
            join_or(weak),
            if n.post_declares_completion { "declares" } else { "declares no" }
        ));
    }
    lines
}

fn join_or(mut v: Vec<String>) -> String {
    v.sort();
    if v.is_empty() {
        "none".to_string()
    } else {
        v.join(", ")
    }
}

// ── Expr identifier walk (pre/post reads) ──────────────────────────────

fn expr_identifiers(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Identifier(s) => {
            out.insert(s.clone());
        }
        Expr::BinaryOp(_, l, r) | Expr::Within(l, r) => {
            expr_identifiers(l, out);
            expr_identifiers(r, out);
        }
        Expr::UnaryOp(_, x)
        | Expr::Field(x, _)
        | Expr::Deref(x)
        | Expr::AddrOf(x)
        | Expr::Consume(x)
        | Expr::Await(x) => expr_identifiers(x, out),
        Expr::Named { inner, .. } => expr_identifiers(inner, out),
        Expr::Call(_, args, _) => {
            for a in args {
                expr_identifiers(a, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(src: &str) -> CausalGraph {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        let items = p.parse_program().unwrap();
        run(&items)
    }

    #[test]
    fn hysteresis_chain_gets_proven_edges() {
        // idle: pre(mode==Idle, temp>80) post(mode==Alarm)
        // alarm: pre(mode==Alarm, temp<75) post(mode==Idle)
        // idle->alarm: post(idle) proves mode==Alarm; temp<75 is not
        // entailed (temp not written by idle, unconstrained in pre(idle))
        // -> WEAK. The dependency is real but not provably-enabling.
        let src = r#"
            let mode: Int = 0;
            let temp: Float = 0.0;
            node idle
                [mode == 0 && temp > 80.0]
                [mode == 1]
            { mode = 1; }
            node alarm
                [mode == 1 && temp < 75.0]
                [mode == 0]
            { mode = 0; }
        "#;
        let g = graph(src);
        assert!(g.refusals.is_empty(), "hysteresis declares completions: {:?}", g.refusals);
        let e = g.edges.iter().find(|e| e.from == "idle" && e.to == "alarm").expect("idle->alarm edge");
        assert_eq!(e.proof, EdgeProof::Weak, "temp<75 is not entailed");
    }

    #[test]
    fn fully_proven_chain() {
        // shutdown's pre is fully entailed by commit's post (+ preserved facts).
        let src = r#"
            let stage: Int = 0;
            node arm
                [stage == 0]
                [stage == 1]
            { stage = 1; }
            node commit
                [stage == 1]
                [stage == 2]
            { stage = 2; }
            node shutdown
                [stage == 2]
                [stage == 3]
            { stage = 3; }
        "#;
        let g = graph(src);
        assert!(g.refusals.is_empty());
        for (a, b) in [("arm", "commit"), ("commit", "shutdown")] {
            let e = g.edges.iter().find(|e| e.from == a && e.to == b).expect("edge");
            assert_eq!(e.proof, EdgeProof::Proven, "{a}->{b} should be proven");
        }
    }

    #[test]
    fn oscillator_without_completion_is_refused() {
        let src = r#"
            let x: Int = 0;
            node flip
                [x == 0]
                [true]
            { x = 1 - x; }
        "#;
        let g = graph(src);
        assert_eq!(g.refusals.len(), 1, "oscillator must be refused");
        assert!(g.refusals[0].contains("flip"), "names the node: {}", g.refusals[0]);
        assert!(g.refusals[0].contains("liveness"), "states the missing obligation");
        assert!(g.refusals[0].contains("fix:"), "gives the fix");
    }

    #[test]
    fn cycle_with_completion_declaration_passes() {
        let src = r#"
            let x: Int = 0;
            let done: Bool = false;
            node step
                [done == false]
                [x > 0]
            { x = x + 1; done = x > 3; }
        "#;
        let g = graph(src);
        assert!(g.refusals.is_empty(), "completion declared: {:?}", g.refusals);
    }

    #[test]
    fn two_node_cycle_both_trivial_posts_refused() {
        let src = r#"
            let a: Int = 0;
            let b: Int = 0;
            node ping
                [a == 0]
                [true]
            { b = b + 1; }
            node pong
                [b == 0]
                [true]
            { a = a + 1; }
        "#;
        let g = graph(src);
        assert_eq!(g.refusals.len(), 1);
        assert!(g.refusals[0].contains("ping") && g.refusals[0].contains("pong"));
    }

    #[test]
    fn acyclic_program_has_no_refusals() {
        let src = r#"
            let x: Int = 0;
            let y: Int = 0;
            node producer
                [x == 0]
                [y == 1]
            { y = 1; }
            node consumer
                [y == 1]
                [y == 1]
            { }
        "#;
        let g = graph(src);
        assert!(g.refusals.is_empty());
        let e = g.edges.iter().find(|e| e.from == "producer" && e.to == "consumer").expect("edge");
        assert_eq!(e.proof, EdgeProof::Proven, "[y==1] post entails [y==1] pre");
    }

    #[test]
    fn rewritten_field_cannot_vouch_for_own_pre() {
        // 2026-09-16 (electronics fixups): A rewrites `y` (y = 2) but its
        // post does NOT re-establish `y == 1`; its pre (`y == 1`) must not
        // vouch for B's matching pre through the stale pre-facts. The edge is
        // WEAK — A destroys the very state B requires. Before the fix A's pre
        // entailed B's pre and the edge was falsely PROVEN.
        let src = r#"
            let y: Int = 0;
            node a
                [y == 1]
                [y == 2]
            { y = 2; }
            node b
                [y == 1]
                [y == 2]
            { }
        "#;
        let g = graph(src);
        let e = g.edges.iter().find(|e| e.from == "a" && e.to == "b").expect("a->b edge");
        assert_eq!(
            e.proof,
            EdgeProof::Weak,
            "a rewrites y without re-establishing y==1 — the edge must be WEAK, not PROVEN"
        );
    }

    #[test]
    fn report_is_deterministic_and_sorted() {
        let src = r#"
            let stage: Int = 0;
            node arm
                [stage == 0]
                [stage == 1]
            { stage = 1; }
            node commit
                [stage == 1]
                [stage == 2]
            { stage = 2; }
        "#;
        let g = graph(src);
        let r1 = explain(&g);
        let r2 = explain(&g);
        assert_eq!(r1, r2, "report is deterministic");
        assert!(r1[0].starts_with("node arm"), "sorted by node name: {}", r1[0]);
    }

    #[test]
    fn non_reactive_txns_are_ignored() {
        let src = r#"
            let x: Int = 0;
            txn contract_carrier
                [x == 0]
                [x == 0]
            { }
        "#;
        let g = graph(src);
        assert!(g.nodes.is_empty(), "plain txn is a contract carrier, not a node");
        assert!(g.refusals.is_empty());
    }
}
