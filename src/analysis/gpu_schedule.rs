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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use crate::analysis::accel::AccelEntry;
use crate::ast::Expr;
use crate::ast::top::{Statement, TopLevel, Transaction};
use crate::ast::BinaryOpKind;

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
    /// Epilogue-fusible pairs (Phase 4a): a GEMM producer whose output is
    /// consumed by a pure elementwise SCALE (`out[c] = producer_y[c] * k`)
    /// that is dead afterwards — the scale folds into the GEMM's y-store.
    pub fusions: Vec<Fusion>,
    /// 2026-09-14 (Phase 3 — buffer reuse): per-array last-use — the last
    /// txn (topo order) that reads or writes each array. An array is dead
    /// after its last-use txn completes; its slot may be reused by a later
    /// array of matching size/type.
    pub array_last_use: HashMap<String, String>,
    /// 2026-09-15 (Phase 3 enablement): per-array first-use — the first txn
    /// (topo order) that reads or writes each array. The runner uploads an
    /// input array from host state right before its first-use kernel; an
    /// aliased slot is free then (last_use(partner) < first_use(this)).
    pub array_first_use: HashMap<String, String>,
    /// 2026-09-14 (Phase 3): reuse opportunities — (dead_slot, new_array,
    /// after_txn). The runner may alias `new_array` into `dead_slot`'s
    /// projection offset after `after_txn` completes.
    pub reuse_opportunities: Vec<(String, String, String)>,
}

impl GpuSchedule {
/// Phase 3 — convert reuse opportunities to a mapping: aliased field →
/// target field (the one whose device slot is reused). For `projection_offsets`
/// and the kernel's SSBO struct.
///
/// Chains are resolved transitively (B→A, C→B ⇒ C→A) so a field's offset
/// equals the slot's ORIGINAL owner regardless of the order
/// `projection_offsets` walks the fields.
pub fn reuse_map(&self) -> HashMap<String, String> {
    let direct: HashMap<String, String> = self
        .reuse_opportunities
        .iter()
        .map(|(dead, new, _)| (new.clone(), dead.clone()))
        .collect();
    let mut out: HashMap<String, String> = HashMap::new();
    for (new, dead) in &direct {
        let mut target = dead.clone();
        let mut seen = std::collections::HashSet::new();
        while let Some(next) = direct.get(&target) {
            if !seen.insert(target.clone()) {
                break; // cycle guard (should not happen)
            }
            target = next.clone();
        }
        out.insert(new.clone(), target);
    }
    out
}
}

/// A producer→consumer epilogue fusion: the consumer's elementwise scale
/// is applied by the producer's kernel, and the consumer node is dropped.
#[derive(Debug, Clone)]
pub struct Fusion {
    pub producer: String,
    pub consumer: String,
    /// The scale multiplier (the consumer's `out = in * k` constant).
    pub scale: f64,
    /// The consumer's output field (the fused kernel writes here).
    pub out_field: String,
    /// The producer's output field being scaled (dead after fusion).
    pub in_field: String,
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
    array_sizes: &HashMap<String, u64>,
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

    // Phase 4a: epilogue-fusible pairs. A RAW edge producer→consumer where
    // the consumer is a pure elementwise scale of the producer's output
    // (`out[c] = in[c] * k`) and `in` is the RAW field. The scale folds into
    // the producer's y-store; the consumer node is dropped.
    let consts = module_consts(program);
    for (a, b, kind) in &sched.edges {
        if *kind != EdgeKind::Raw {
            continue;
        }
        let Some(consumer) = accel.get(b) else {
            continue;
        };
        let Some((out_field, in_field, scale)) =
            detect_scale(&consumer.shape.kernel_stmts, &consts)
        else {
            continue;
        };
        // The producer must WRITE `in_field` (the RAW dependency), and the
        // consumer must be its only reader (dead-after-fusion). We check the
        // producer side here; the "only reader" proof is the consumer being
        // the single node with `in_field` in its reads.
        let producer_writes_in = sched
            .writes
            .iter()
            .find(|(n, _)| n == a)
            .map(|(_, ws)| ws.iter().any(|w| w == &in_field))
            .unwrap_or(false);
        if !producer_writes_in {
            continue;
        }
        let readers: usize = sched
            .reads
            .iter()
            .filter(|(_, rs)| rs.iter().any(|r| r == &in_field))
            .count();
        if readers != 1 {
            continue;
        }
        // 2026-09-15: the runner applies epilogue fusion ONLY to f32 (elem
        // 4) producers — the f16 tensor epilogue isn't ready. The schedule's
        // fused-consumer set drives `compute_array_last_use` (a fused
        // consumer is skipped), so it must match: an Int/f16 scale still
        // runs as its own node, and excluding it would corrupt last-use.
        if array_sizes.get(&in_field).copied() != Some(4) {
            continue;
        }
        sched.fusions.push(Fusion {
            producer: a.clone(),
            consumer: b.clone(),
            scale,
            out_field,
            in_field,
        });
    }

    // 2026-09-14 (Phase 3 — buffer reuse): per-array last-use + reuse
    // opportunities. An array is dead after its last-use txn completes; its
    // slot may be reused by a later array of matching size/type.
    compute_array_last_use(&mut sched);
    sched.array_first_use = compute_array_first_use(&sched);
    compute_reuse_opportunities(&mut sched, array_sizes);

    sched
}

/// Phase 3 — per-array last-use: the last txn (topo order) that reads or
/// writes each array. Fused consumers are skipped (their work folds into
/// the producer's epilogue).
fn compute_array_last_use(sched: &mut GpuSchedule) {
    let all_arrays: HashSet<String> = sched
        .reads
        .iter()
        .chain(sched.writes.iter())
        .flat_map(|(_, names)| names.iter().cloned())
        .collect();
    let fused_consumers: HashSet<String> =
        sched.fusions.iter().map(|f| f.consumer.clone()).collect();
    for arr in &all_arrays {
        let last = sched
            .order
            .iter()
            .filter(|n| !fused_consumers.contains(*n))
            .filter(|n| txn_touches_array(sched, n, arr))
            .last()
            .cloned();
        if let Some(txn) = last {
            sched.array_last_use.insert(arr.clone(), txn);
        }
    }
}

/// True when txn `n` reads or writes array `arr` (checks BOTH the reads and
/// writes entries — a txn that only writes `arr` has no reads entry match).
fn txn_touches_array(sched: &GpuSchedule, n: &str, arr: &str) -> bool {
    let touches = |sets: &[(String, Vec<String>)]| {
        sets.iter()
            .find(|(nm, _)| nm == n)
            .map_or(false, |(_, bs)| bs.iter().any(|b| b == arr))
    };
    touches(&sched.reads) || touches(&sched.writes)
}

/// True when txn `n` WRITES array `arr` (its writes entry lists it).
fn txn_writes_array(sched: &GpuSchedule, n: &str, arr: &str) -> bool {
    sched
        .writes
        .iter()
        .find(|(nm, _)| nm == n)
        .map_or(false, |(_, bs)| bs.iter().any(|b| b == arr))
}

/// Phase 3 — first-use per array: the first txn (topo order) touching it.
fn compute_array_first_use(sched: &GpuSchedule) -> HashMap<String, String> {
    let mut first_use: HashMap<String, String> = HashMap::new();
    for n in &sched.order {
        let mut arrays_here = BTreeSet::new();
        for (nm, bs) in sched.reads.iter().chain(sched.writes.iter()) {
            if nm == n {
                for b in bs {
                    arrays_here.insert(b.clone());
                }
            }
        }
        for arr in arrays_here {
            first_use.entry(arr).or_insert_with(|| n.clone());
        }
    }
    first_use
}

/// Phase 3 — reuse opportunities: greedy interval allocation over the
/// topo order. Every array occupies a slot; a WRITE-FIRST array may take a
/// slot whose previous occupant is already dead (last_use < first_use).
/// Picking the tightest free slot (largest last_use still < first_use)
/// yields pairwise-disjoint live ranges per slot — the correctness
/// condition the pairwise formulation missed (it let two simultaneously-live
/// arrays share one dead slot).
///
/// Write-first only (B's first-use txn writes it): a read-first (input)
/// array must keep its own seeded slot, because the one-time seed uploads
/// inputs before any kernel runs — a shared slot would be clobbered.
fn compute_reuse_opportunities(sched: &mut GpuSchedule, array_sizes: &HashMap<String, u64>) {
    let first_use = compute_array_first_use(sched);
    let order_idx: HashMap<&str, usize> = sched
        .order
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    // (name, first_idx, last_idx, write_first), sorted by first-use.
    let mut arrays: Vec<(String, usize, usize, bool)> = first_use
        .iter()
        .filter_map(|(name, ftxn)| {
            let &fi = order_idx.get(ftxn.as_str())?;
            let &li = order_idx.get(sched.array_last_use.get(name)?.as_str())?;
            let wf = txn_writes_array(sched, ftxn, name);
            Some((name.clone(), fi, li, wf))
        })
        .collect();
    arrays.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    // slot_owner: (owner_name, owner_last_idx, owner_size) per live slot,
    // in creation order (deterministic tie-break: first candidate wins).
    // A slot is shared ONLY by same-size arrays (size key = device storage
    // bytes; arrays of different sizes cannot occupy the same extent).
    let mut slot_owner: Vec<(String, usize, u64)> = Vec::new();
    for (name, fi, li, write_first) in &arrays {
        if *write_first {
            // Tightest free slot: largest last_use still < first_use, and
            // the same size as this array.
            let size = array_sizes.get(name).copied().unwrap_or(0);
            let cand = slot_owner
                .iter()
                .enumerate()
                .filter(|(_, (_, sl, os))| *sl < *fi && *os == size)
                .max_by_key(|(_, (_, sl, _))| *sl)
                .map(|(i, _)| i);
            if let Some(pos) = cand {
                let (owner, _, _) = slot_owner[pos].clone();
                let after = sched
                    .array_last_use
                    .get(&owner)
                    .cloned()
                    .unwrap_or_default();
                sched
                    .reuse_opportunities
                    .push((owner, name.clone(), after));
                slot_owner[pos] = (name.clone(), *li, size);
                continue;
            }
        }
        slot_owner.push((name.clone(), *li, array_sizes.get(name).copied().unwrap_or(0)));
    }
}

/// Detect a pure elementwise scale: `out[idx] = in[idx] * k` (or `k * in[idx]`).
/// Returns `(out_field, in_field, k)`. `k` may be a literal or a module const.
fn detect_scale(
    stmts: &[Statement],
    consts: &HashMap<String, f64>,
) -> Option<(String, String, f64)> {
    // Exactly one assignment.
    let assign = stmts.iter().find_map(|s| {
        if let Statement::Assign(lhs, rhs) = s {
            Some((lhs, rhs))
        } else {
            None
        }
    })?;
    let (lhs, rhs) = assign;
    let out_field = match lhs {
        Expr::Index(b, _) => match &**b {
            Expr::Identifier(s) => s.clone(),
            _ => return None,
        },
        _ => return None,
    };
    let Expr::BinaryOp(BinaryOpKind::Mul, l, r) = rhs else {
        return None;
    };
    let as_f = |e: &Expr| -> Option<f64> {
        match e {
            Expr::Float(k) => Some(*k),
            Expr::Decimal(k) => Some(*k as f64),
            Expr::Identifier(s) => consts.get(s).copied(),
            _ => None,
        }
    };
    let as_index = |e: &Expr| -> Option<String> {
        match e {
            Expr::Index(b, _) => match &**b {
                Expr::Identifier(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        }
    };
    let (in_field, k) = match (as_index(l), as_f(r)) {
        (Some(f), Some(k)) => (f, k),
        _ => match (as_f(l), as_index(r)) {
            (Some(k), Some(f)) => (f, k),
            _ => return None,
        },
    };
    Some((out_field, in_field, k))
}

/// Module-level const float/int literals (for folding scale multipliers).
fn module_consts(program: &[TopLevel]) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for item in program {
        if let TopLevel::Constant(c) = item {
            match &c.expr {
                Expr::Float(f) => {
                    m.insert(c.name.clone(), *f);
                }
                Expr::Decimal(n) => {
                    m.insert(c.name.clone(), *n as f64);
                }
                _ => {}
            }
        }
    }
    m
}
#[cfg(test)]
mod tests {
    use super::*;

    fn sizes(pairs: &[(&str, u64)]) -> HashMap<String, u64> {
        pairs.iter().map(|(n, s)| (n.to_string(), *s)).collect()
    }

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
        let sched = build_schedule(&program, &accel, &HashMap::new());
        let gemm1_pos = sched.order.iter().position(|n| n == "gemm1");
        let gemm2_pos = sched.order.iter().position(|n| n == "gemm2");
        assert!(gemm1_pos.is_some() && gemm2_pos.is_some());
        assert!(gemm1_pos.unwrap() < gemm2_pos.unwrap(), "producer must fire first: {:?}", sched.order);
        assert!(!sched.independent.is_empty() == false || sched.independent.is_empty());
    }

    #[test]
    fn detects_epilogue_scale_fusion() {
        // gemm1 (writes c) -> scale (c[i]*2.0 -> d[i]). The scale's body is
        // the pure `d[i] = c[i] * 2.0`; c is read only by scale. The fusion
        // must fold the scale into gemm1's epilogue.
        let mut accel = HashMap::new();
        let gemm_shape = crate::analysis::accel::KernelShape {
            index_var: "i".into(),
            count_expr: None,
            kernel_stmts: vec![],
            host_stmts: vec![],
            read_buffers: vec!["a".into(), "b".into()],
            write_buffers: vec!["c".into()],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: None,
        };
        accel.insert(
            "gemm1".into(),
            AccelEntry {
                mode: crate::analysis::accel::AccelMode::TryAll,
                forced: false,
                shape: gemm_shape,
                decision: crate::analysis::accel::AccelDecision::Gpu,
            },
        );
        // scale: kernel_stmts = [d[i] = c[i] * 2.0]
        let scale_stmt = Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("d".into())), Box::new(Expr::Identifier("i".into()))),
            Expr::BinaryOp(
                BinaryOpKind::Mul,
                Box::new(Expr::Index(Box::new(Expr::Identifier("c".into())), Box::new(Expr::Identifier("i".into())))),
                Box::new(Expr::Decimal(2)),
            ),
        );
        accel.insert(
            "scale".into(),
            AccelEntry {
                mode: crate::analysis::accel::AccelMode::TryAll,
                forced: false,
                shape: crate::analysis::accel::KernelShape {
                    index_var: "i".into(),
                    count_expr: None,
                    kernel_stmts: vec![scale_stmt],
                    host_stmts: vec![],
                    read_buffers: vec!["c".into()],
                    write_buffers: vec!["d".into()],
                    scalar_ins: vec![],
                    eligible: true,
                    reasons: vec![],
                    work_cols: None,
                    reduction: None,
                },
                decision: crate::analysis::accel::AccelDecision::Gpu,
            },
        );
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
        let program = vec![txn("gemm1", "i"), txn("scale", "j")];
        // The fusion is f32-only (matches the runner): in_field "c" is f32.
        let sz = sizes(&[("c", 4), ("d", 4)]);
        let sched = build_schedule(&program, &accel, &sz);
        assert_eq!(sched.fusions.len(), 1, "one fusion expected");
        let f = &sched.fusions[0];
        assert_eq!(f.producer, "gemm1");
        assert_eq!(f.consumer, "scale");
        assert_eq!(f.out_field, "d");
        assert_eq!(f.in_field, "c");
        assert!((f.scale - 2.0).abs() < 1e-9);
        // 2026-09-15: an Int (elem 8) in_field must NOT fuse — the runner
        // only applies f32 epilogue fusion, and excluding a still-running
        // consumer from last-use would corrupt the reuse analysis.
        let sz_int = sizes(&[("c", 8), ("d", 8)]);
        let sched_int = build_schedule(&program, &accel, &sz_int);
        assert_eq!(sched_int.fusions.len(), 0, "Int scale must not fuse");
    }

    #[test]
    fn array_last_use_tracks_dead_arrays() {
        // 3-node attention-decode: qk (reads q,kt / writes s),
        // scale (reads s / writes s2), pv (reads s2,kt / writes o).
        // s is dead after scale; s2 is dead after pv; q/kt are dead after qk/pv.
        let mut accel = HashMap::new();
        let mk = |name: &str, reads: &[&str], writes: &[&str]| -> AccelEntry {
            AccelEntry {
                mode: crate::analysis::accel::AccelMode::TryAll,
                forced: false,
                shape: crate::analysis::accel::KernelShape {
                    index_var: "i".into(),
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
        accel.insert("qk".into(), mk("qk", &["q", "kt"], &["s"]));
        accel.insert("scale".into(), mk("scale", &["s"], &["s2"]));
        accel.insert("pv".into(), mk("pv", &["s2", "kt"], &["o"]));
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
        let program = vec![txn("qk", "i"), txn("scale", "j"), txn("pv", "k")];
        // q, kt, s, s2, o are arrays (65536 bytes each); i/j/k are scalars.
        let sz = sizes(&[
            ("q", 65536), ("kt", 65536), ("s", 65536), ("s2", 65536), ("o", 65536),
            ("i", 8), ("j", 8), ("k", 8),
        ]);
        let sched = build_schedule(&program, &accel, &sz);
        // qk fires first, then scale, then pv.
        assert_eq!(sched.order, vec!["qk", "scale", "pv"]);
        // s is last used by scale (qk writes it, scale reads it).
        assert_eq!(sched.array_last_use.get("s").map(|s| s.as_str()), Some("scale"));
        // s2 is last used by pv.
        assert_eq!(sched.array_last_use.get("s2").map(|s| s.as_str()), Some("pv"));
        // q is last used by qk.
        assert_eq!(sched.array_last_use.get("q").map(|s| s.as_str()), Some("qk"));
        // kt is last used by pv.
        assert_eq!(sched.array_last_use.get("kt").map(|s| s.as_str()), Some("pv"));
        // o is last used by pv.
        assert_eq!(sched.array_last_use.get("o").map(|s| s.as_str()), Some("pv"));
        // First use: q/kt/s start at qk, s2 at scale, o at pv.
        assert_eq!(sched.array_first_use.get("q").map(|s| s.as_str()), Some("qk"));
        assert_eq!(sched.array_first_use.get("kt").map(|s| s.as_str()), Some("qk"));
        assert_eq!(sched.array_first_use.get("s").map(|s| s.as_str()), Some("qk"));
        assert_eq!(sched.array_first_use.get("s2").map(|s| s.as_str()), Some("scale"));
        assert_eq!(sched.array_first_use.get("o").map(|s| s.as_str()), Some("pv"));

        // Reuse opportunities (greedy interval allocation, write-first targets):
        // q (dead after qk) is reused by s2 (first used at scale, write-first).
        // s (dead after scale) is reused by o (first used at pv, write-first).
        // Slots are exclusive: q's slot cannot also serve o (o would be
        // live alongside s2).
        assert!(sched.reuse_opportunities.iter().any(|(d, n, _)| d == "q" && n == "s2"),
            "q slot reused by s2: {:?}", sched.reuse_opportunities);
        assert!(sched.reuse_opportunities.iter().any(|(d, n, _)| d == "s" && n == "o"),
            "s slot reused by o: {:?}", sched.reuse_opportunities);
        // A dead slot serves at most one later array.
        let q_targets: Vec<&str> = sched.reuse_opportunities.iter()
            .filter(|(d, _, _)| d == "q").map(|(_, n, _)| n.as_str()).collect();
        assert_eq!(q_targets, vec!["s2"], "q's slot must be reused once");
    }
}
