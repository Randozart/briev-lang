//! gpu_schedule: the contract-leverage scheduling DAG.
//!
//! Plan `docs/plans/2026-09-14-gpu-schedule-pass.md`, Phase 1. The runner's
//! node dispatch is currently declaration order with the author's `phase`
//! scalars as the ordering edges. This pass builds the REAL node DAG from
//! each node's read/write sets (the accel kernel shape's buffers + the
//! host nodes' scalar reads/writes) and emits a producer-before-consumer
//! topological order, plus independence proofs (no data edge either way —
//! Phase 2's sync-elimination gate).
//!
//! Every decision is a structural proof over the declared reads/writes —
//! the contract-leverage tier: the compiler sees across kernel boundaries.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use crate::analysis::accel::AccelEntry;
use crate::ast::Expr;
use crate::ast::top::{Statement, TopLevel, Transaction};

/// The scheduling DAG computed from the program's node read/write sets.
#[derive(Debug, Clone, Default)]
pub struct GpuSchedule {
    /// Node names in producer-before-consumer topological order.
    pub order: Vec<String>,
    /// Independent pairs — no data edge in either direction (RAW/WAW/WAR
    /// all absent) — so they may launch back-to-back without a sync
    /// (Phase 2).
    pub independent: Vec<(String, String)>,
    /// The edges that DO require ordering, with the kind.
    pub edges: Vec<(String, String, EdgeKind)>,
    /// Per-node read/write sets, for diagnostics and the record.
    pub reads: Vec<(String, Vec<String>)>,
    pub writes: Vec<(String, Vec<String>)>,
}

/// Why an edge exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// Producer writes what the consumer reads (true data dependency).
    Raw,
    /// Both write the same field (write-after-write).
    Waw,
    /// Producer reads what the consumer writes (read-after-write).
    War,
}

/// Collect the identifiers referenced by an expression (state field names).
fn expr_ids(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Identifier(s) => {
            out.insert(s.clone());
        }
        Expr::BinaryOp(_, l, r) => {
            expr_ids(l, out);
            expr_ids(r, out);
        }
        Expr::Range { start, end, .. } => {
            expr_ids(start, out);
            expr_ids(end, out);
        }
        Expr::UnaryOp(_, x)
        | Expr::Field(x, _)
        | Expr::Deref(x)
        | Expr::AddrOf(x)
        | Expr::Consume(x)
        | Expr::Await(x) => expr_ids(x, out),
        Expr::Call(_, args, _) | Expr::List(args) | Expr::Tuple(args) => {
            for a in args {
                expr_ids(a, out);
            }
        }
        Expr::MethodCall(recv, _, args, _) => {
            expr_ids(recv, out);
            for a in args {
                expr_ids(a, out);
            }
        }
        Expr::Index(l, r) => {
            expr_ids(l, out);
            expr_ids(r, out);
        }
        Expr::Slice {
            array,
            start,
            end,
            stride,
        } => {
            expr_ids(array, out);
            if let Some(s) = start {
                expr_ids(s, out);
            }
            if let Some(e) = end {
                expr_ids(e, out);
            }
            if let Some(s) = stride {
                expr_ids(s, out);
            }
        }
        Expr::Reflect(recv, _, _) => expr_ids(recv, out),
        Expr::Cast(recv, _) => expr_ids(recv, out),
        Expr::IsType(recv, _) => expr_ids(recv, out),
        Expr::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                expr_ids(v, out);
            }
        }
        Expr::Spawn { args, .. } => {
            for a in args {
                expr_ids(a, out);
            }
        }
        Expr::PluginIntercept { args, .. } => {
            for a in args {
                expr_ids(a, out);
            }
        }
        Expr::Block(stmts) => {
            for s in stmts {
                if let Statement::Assign(_, r) = s {
                    expr_ids(r, out);
                }
            }
        }
        Expr::If(c, t, e2) => {
            expr_ids(c, out);
            expr_ids(t, out);
            if let Some(e2) = e2 {
                expr_ids(e2, out);
            }
        }
        Expr::Within(c, t) => {
            expr_ids(c, out);
            expr_ids(t, out);
        }
        Expr::Match(scrut, arms) => {
            expr_ids(scrut, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    expr_ids(g, out);
                }
                expr_ids(&arm.body, out);
            }
        }
        Expr::Named { inner, .. } => expr_ids(inner, out),
        _ => {}
    }
}

/// The reads of one transaction.
///
/// Kernel nodes: the accel shape's `read_buffers` (arrays) + `scalar_ins`
/// (read-only scalars/consts) + the index counter's pre read.
/// Host nodes: the identifiers in the pre-condition (the phase/counters
/// that gate this node).
fn txn_reads(
    t: &Transaction,
    accel: &HashMap<String, AccelEntry>,
    out: &mut BTreeSet<String>,
) {
    if let Some(e) = accel.get(&t.name) {
        if e.shape.eligible {
            for f in &e.shape.read_buffers {
                out.insert(f.clone());
            }
            for s in &e.shape.scalar_ins {
                out.insert(s.clone());
            }
        }
    }
    expr_ids(&t.contract.pre_condition, out);
}

/// The writes of one transaction.
///
/// Kernel nodes: the accel shape's `write_buffers` (arrays) + the index
/// counter (the runner fast-forwards it to N). Host nodes: the scalar
/// identifiers assigned in the body.
fn txn_writes(
    t: &Transaction,
    accel: &HashMap<String, AccelEntry>,
    out: &mut BTreeSet<String>,
) {
    if let Some(e) = accel.get(&t.name) {
        if e.shape.eligible {
            for f in &e.shape.write_buffers {
                out.insert(f.clone());
            }
            if !e.shape.index_var.is_empty() {
                out.insert(e.shape.index_var.clone());
            }
        }
    }
    for s in &t.body {
        if let Statement::Assign(lhs, _) = s {
            expr_ids(lhs, out);
        }
    }
}

/// Build the node DAG from the program's read/write sets.
///
/// Edge A→B exists when the ordering matters:
/// - RAW:  writes(A) ∩ reads(B) ≠ ∅   (producer data → consumer)
/// - WAW:  writes(A) ∩ writes(B) ≠ ∅  (shared output)
/// - WAR:  reads(A) ∩ writes(B) ≠ ∅   (consumer reads before producer writes)
/// Nodes with no edge in either direction are INDEPENDENT (Phase 2).
///
/// Topological order falls back to declaration order on a cycle (a
/// well-formed reactive program has none; a cycle is a contract bug the
/// causality pass would already flag).
pub fn build_schedule(
    program: &[TopLevel],
    accel: &HashMap<String, AccelEntry>,
) -> GpuSchedule {
    let mut nodes: Vec<(String, BTreeSet<String>, BTreeSet<String>)> = Vec::new();
    for item in program {
        if let TopLevel::Transaction(t) = item {
            let mut r = BTreeSet::new();
            let mut w = BTreeSet::new();
            txn_reads(t, accel, &mut r);
            txn_writes(t, accel, &mut w);
            nodes.push((t.name.clone(), r, w));
        }
    }

    let mut sched = GpuSchedule::default();
    for (n, r, w) in &nodes {
        sched.reads.push((n.clone(), r.iter().cloned().collect()));
        sched.writes.push((n.clone(), w.iter().cloned().collect()));
    }

    // Edges + topo order (Kahn).
    let names: Vec<String> = nodes.iter().map(|(n, _, _)| n.clone()).collect();
    let idx: HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut indeg = vec![0usize; nodes.len()];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for i in 0..nodes.len() {
        for j in 0..nodes.len() {
            if i == j {
                continue;
            }
            let (ni, ri, wi) = &nodes[i];
            let (nj, rj, wj) = &nodes[j];
            let wi_rj: Vec<&String> = wi.intersection(rj).collect();
            let wi_wj: Vec<&String> = wi.intersection(wj).collect();
            let ri_wj: Vec<&String> = ri.intersection(wj).collect();
            let kind = if !wi_rj.is_empty() {
                Some((EdgeKind::Raw, i, j))
            } else if !wi_wj.is_empty() {
                Some((EdgeKind::Waw, i, j))
            } else if !ri_wj.is_empty() {
                // WAR: `i` READS what `j` WRITES → the writer `j` must fire
                // first (edge j→i), so the reader sees the pre-write value.
                Some((EdgeKind::War, j, i))
            } else {
                None
            };
            if let Some((k, src, dst)) = kind {
                sched
                    .edges
                    .push((names[src].clone(), names[dst].clone(), k));
                adj[src].push(dst);
                indeg[dst] += 1;
            }
        }
    }
    // Independence: no edge either way.
    let mut has_edge = vec![vec![false; nodes.len()]; nodes.len()];
    for (a, b, _) in &sched.edges {
        if let (Some(&ia), Some(&ib)) = (idx.get(a.as_str()), idx.get(b.as_str())) {
            has_edge[ia][ib] = true;
        }
    }
    for i in 0..nodes.len() {
        for j in i + 1..nodes.len() {
            if !has_edge[i][j] && !has_edge[j][i] {
                sched.independent.push((names[i].clone(), names[j].clone()));
            }
        }
    }

    // Kahn topo sort (stable: declaration order among ready nodes).
    let mut q: VecDeque<usize> = VecDeque::new();
    for i in 0..nodes.len() {
        if indeg[i] == 0 {
            q.push_back(i);
        }
    }
    let mut ordered: Vec<usize> = Vec::new();
    while let Some(u) = q.pop_front() {
        ordered.push(u);
        for &v in &adj[u] {
            indeg[v] -= 1;
            if indeg[v] == 0 {
                q.push_back(v);
            }
        }
    }
    sched.order = if ordered.len() == nodes.len() {
        ordered.iter().map(|&i| names[i].clone()).collect()
    } else {
        // Cycle: fall back to declaration order (the reactor still works;
        // the causality pass flags the cycle as a program bug).
        names.clone()
    };
    let _ = BTreeMap::<String, usize>::new();
    sched
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_dag_orders_producer_first() {
        // Two nodes: gemm1 writes `c`/`i`, gemm2 reads `c`/writes `d`/`j`.
        // gemm2 is declared FIRST — the DAG must order gemm1 before gemm2.
        let mut accel = HashMap::new();
        let mk = |name: &str, iv: &str, reads: &[&str], writes: &[&str]| -> AccelEntry {
            AccelEntry {
                mode: crate::analysis::accel::AccelMode::TryAll,
                forced: false,
                shape: crate::analysis::accel::KernelShape {
                    index_var: iv.into(),
                    count_expr: None,
                    kernel_stmts: vec![],
                    host_stmts: vec![],
                    read_buffers: reads.iter().map(|s| s.to_string()).collect(),
                    write_buffers: writes.iter().map(|s| s.to_string()).collect(),
                    scalar_ins: vec![],
                    eligible: true,
                    reasons: vec![],
                    work_cols: None,
                    reduction: None,
                },
                decision: crate::analysis::accel::AccelDecision::Gpu,
            }
        };
        accel.insert("gemm2".into(), mk("gemm2", "j", &["c", "e"], &["d"]));
        accel.insert("gemm1".into(), mk("gemm1", "i", &["a", "b"], &["c"]));
        // Fake transactions so build_schedule sees the nodes in reverse order.
        let txn = |name: &str, pre: &str| -> TopLevel {
            TopLevel::Transaction(Transaction {
                name: name.into(),
                is_reactive: false,
                is_async: true,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: crate::ast::top::Contract {
                    pre_condition: Expr::Identifier(pre.into()),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    span: None,
                    explicit: true,
                    post_authority: false,
                },
                body: vec![],
                metadata: Default::default(),
                derivation: None,
                modifiers: vec![],
                doc: None,
                span: None,
            })
        };
        let program = vec![txn("gemm2", "j"), txn("gemm1", "i")];
        let sched = build_schedule(&program, &accel);
        let gemm1_pos = sched.order.iter().position(|n| n == "gemm1");
        let gemm2_pos = sched.order.iter().position(|n| n == "gemm2");
        assert!(gemm1_pos.is_some() && gemm2_pos.is_some());
        assert!(gemm1_pos.unwrap() < gemm2_pos.unwrap(), "producer must fire first: {:?}", sched.order);
        assert!(!sched.independent.is_empty() == false || sched.independent.is_empty());
    }
}
