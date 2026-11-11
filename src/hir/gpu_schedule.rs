// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-11 (H5 port): contract-leverage scheduling DAG over typed HIR.
//!
//! Mirrors `analysis::gpu_schedule`. Every decision is a structural proof
//! over the declared read/write sets: producer-before-consumer topological
//! order, independence, epilogue/chain fusion topology, and buffer-slot
//! reuse. The public `GpuSchedule` (and `Fusion`/`ChainFusion`/`ArrayMeta`)
//! result types are AST-free, so the consumer reads the HIR result unchanged.
//!
//! HIR-native inputs: callables (transactions), contract preconditions,
//! bodies, module consts, and the HIR accel result. Kernel statements for
//! the scale detector come from the shared HIR accel partition.
//!
//! Complexity exception (house limits): the node DAG is built pairwise over
//! the callable set and per-node read/write scans nest over the same declared
//! surfaces, so these traversals are inherently O(n²) in the node count — the
//! same shape as the AST oracle. The work is proportional to the number of
//! reactive transactions and their direct fields, not program size.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use crate::analysis::accel::AccelEntry;
use crate::analysis::gpu_schedule::{
    fusion_applies, ArrayMeta, ChainFusion, EdgeKind, Fusion, GpuSchedule,
};
use crate::ast::BinaryOpKind;
use crate::hir::expr::{HirExpr, HirExprKind, HirLiteral};
use crate::hir::program::{HirCallable, HirCallableKind, HirProgram};
use crate::hir::stmt::{HirStmt, HirStmtKind};

/// The transaction callables the AST schedule sees: bare, non-exported,
/// non-sync-wrapped `TopLevel::Transaction` items (sync groups and exports
/// are separate top-level forms the AST walker does not visit).
fn schedule_callables(program: &HirProgram) -> impl Iterator<Item = &HirCallable> {
    program.callables.iter().filter(|c| {
        c.kind == HirCallableKind::Transaction && !c.exported && c.sync_domains.is_empty()
    })
}

/// Collect the identifiers referenced by an expression (state field names).
fn expr_ids(e: &HirExpr, out: &mut BTreeSet<String>) {
    match &e.kind {
        HirExprKind::Identifier(s) => {
            out.insert(s.clone());
        }
        HirExprKind::Binary { left, right, .. } => {
            expr_ids(left, out);
            expr_ids(right, out);
        }
        HirExprKind::Range { start, end, .. } => {
            expr_ids(start, out);
            expr_ids(end, out);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Field {
            receiver: inner, ..
        }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => expr_ids(inner, out),
        HirExprKind::Call { args, .. } | HirExprKind::List(args) | HirExprKind::Tuple(args) => {
            for a in args {
                expr_ids(a, out);
            }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            expr_ids(receiver, out);
            for a in args {
                expr_ids(a, out);
            }
        }
        HirExprKind::Index(l, r) => {
            expr_ids(l, out);
            expr_ids(r, out);
        }
        HirExprKind::Slice {
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
        HirExprKind::Reflect { receiver, .. } => expr_ids(receiver, out),
        HirExprKind::Cast { inner, .. } => expr_ids(inner, out),
        HirExprKind::IsType { inner, .. } => expr_ids(inner, out),
        HirExprKind::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                expr_ids(v, out);
            }
        }
        HirExprKind::Spawn { args, .. } => {
            for a in args {
                expr_ids(a, out);
            }
        }
        HirExprKind::PluginIntercept { args, .. } => {
            for a in args {
                expr_ids(a, out);
            }
        }
        HirExprKind::Block(stmts) => {
            for s in stmts {
                if let HirStmtKind::Assign(_, r) = &s.kind {
                    expr_ids(r, out);
                }
            }
        }
        HirExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_ids(condition, out);
            expr_ids(then_branch, out);
            if let Some(e2) = else_branch {
                expr_ids(e2, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => expr_ids_match(scrutinee, arms, out),
        _ => {}
    }
}

fn expr_ids_match(
    scrutinee: &HirExpr,
    arms: &[crate::hir::expr::HirMatchArm],
    out: &mut BTreeSet<String>,
) {
    expr_ids(scrutinee, out);
    for arm in arms {
        if let Some(g) = &arm.guard {
            expr_ids(g, out);
        }
        expr_ids(&arm.body, out);
    }
}

/// The reads of one transaction (kernel buffers/scalars + the precondition).
fn txn_reads(t: &HirCallable, accel: &HashMap<String, AccelEntry>, out: &mut BTreeSet<String>) {
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
    if let Some(contract) = &t.contract {
        expr_ids(&contract.pre, out);
    }
}

/// The writes of one transaction (kernel buffers + index counter + body).
fn txn_writes(t: &HirCallable, accel: &HashMap<String, AccelEntry>, out: &mut BTreeSet<String>) {
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
    for s in &t.body.statements {
        if let HirStmtKind::Assign(lhs, _) = &s.kind {
            expr_ids(lhs, out);
        }
    }
}

pub fn build_schedule(
    program: &HirProgram,
    accel: &HashMap<String, AccelEntry>,
    array_meta: &HashMap<String, ArrayMeta>,
    f16_acc: bool,
) -> GpuSchedule {
    let mut nodes: Vec<(String, BTreeSet<String>, BTreeSet<String>)> = Vec::new();
    for t in schedule_callables(program) {
        let mut r = BTreeSet::new();
        let mut w = BTreeSet::new();
        txn_reads(t, accel, &mut r);
        txn_writes(t, accel, &mut w);
        nodes.push((t.name.clone(), r, w));
    }
    let mut sched = GpuSchedule::default();
    for (n, r, w) in &nodes {
        sched.reads.push((n.clone(), r.iter().cloned().collect()));
        sched.writes.push((n.clone(), w.iter().cloned().collect()));
    }
    build_edges(&nodes, &mut sched);
    sched.order = topo_order(&nodes, &sched);
    detect_fusions(program, accel, array_meta, f16_acc, &mut sched);
    sched.chain_fusion = detect_chain_fusion(&sched, &sched.fusions, array_meta);
    compute_array_last_use(&mut sched);
    sched.array_first_use = compute_array_first_use(&sched);
    compute_reuse_opportunities(&mut sched, array_meta);
    sched
}

// O(n²) by construction: every ordered node pair is one DAG query.
fn build_edges(nodes: &[(String, BTreeSet<String>, BTreeSet<String>)], sched: &mut GpuSchedule) {
    let names: Vec<String> = nodes.iter().map(|(n, _, _)| n.clone()).collect();
    for i in 0..nodes.len() {
        edges_from(i, nodes, &names, sched);
    }
    compute_independence(&names, sched);
}

fn edges_from(
    i: usize,
    nodes: &[(String, BTreeSet<String>, BTreeSet<String>)],
    names: &[String],
    sched: &mut GpuSchedule,
) {
    for j in 0..nodes.len() {
        if i == j {
            continue;
        }
        let (_, ri, wi) = &nodes[i];
        let (_, rj, wj) = &nodes[j];
        let Some((kind, reverse)) = edge_for(wi, ri, rj, wj) else {
            continue;
        };
        let (src, dst) = if reverse { (j, i) } else { (i, j) };
        sched
            .edges
            .push((names[src].clone(), names[dst].clone(), kind));
    }
}

/// The ordering edge between independent node pairs, if any. WAR reverses
/// direction: the writer must fire first so the reader sees the pre-write
/// value.
fn edge_for(
    wi: &BTreeSet<String>,
    ri: &BTreeSet<String>,
    rj: &BTreeSet<String>,
    wj: &BTreeSet<String>,
) -> Option<(EdgeKind, bool)> {
    if !wi.is_disjoint(rj) {
        return Some((EdgeKind::Raw, false));
    }
    if !wi.is_disjoint(wj) {
        return Some((EdgeKind::Waw, false));
    }
    if !ri.is_disjoint(wj) {
        return Some((EdgeKind::War, true));
    }
    None
}

fn compute_independence(names: &[String], sched: &mut GpuSchedule) {
    let idx: HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut has_edge = vec![vec![false; names.len()]; names.len()];
    for (a, b, _) in &sched.edges {
        if let (Some(&ia), Some(&ib)) = (idx.get(a.as_str()), idx.get(b.as_str())) {
            has_edge[ia][ib] = true;
        }
    }
    for i in 0..names.len() {
        push_independent(i, names, &has_edge, &mut sched.independent);
    }
}

fn push_independent(
    i: usize,
    names: &[String],
    has_edge: &[Vec<bool>],
    independent: &mut Vec<(String, String)>,
) {
    for j in i + 1..names.len() {
        if !has_edge[i][j] && !has_edge[j][i] {
            independent.push((names[i].clone(), names[j].clone()));
        }
    }
}

fn topo_order(
    nodes: &[(String, BTreeSet<String>, BTreeSet<String>)],
    sched: &GpuSchedule,
) -> Vec<String> {
    let names: Vec<String> = nodes.iter().map(|(n, _, _)| n.clone()).collect();
    let idx: HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut indeg = vec![0usize; nodes.len()];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (a, b, _) in &sched.edges {
        if let (Some(&ia), Some(&ib)) = (idx.get(a.as_str()), idx.get(b.as_str())) {
            adj[ia].push(ib);
            indeg[ib] += 1;
        }
    }
    let mut q: VecDeque<usize> = VecDeque::new();
    for i in 0..nodes.len() {
        if indeg[i] == 0 {
            q.push_back(i);
        }
    }
    let mut ordered: Vec<usize> = Vec::new();
    while let Some(u) = q.pop_front() {
        ordered.push(u);
        relax_neighbors(u, &adj, &mut indeg, &mut q);
    }
    if ordered.len() == nodes.len() {
        ordered.iter().map(|&i| names[i].clone()).collect()
    } else {
        names
    }
}

fn relax_neighbors(u: usize, adj: &[Vec<usize>], indeg: &mut [usize], q: &mut VecDeque<usize>) {
    for &v in &adj[u] {
        indeg[v] -= 1;
        if indeg[v] == 0 {
            q.push_back(v);
        }
    }
}

fn detect_fusions(
    program: &HirProgram,
    accel: &HashMap<String, AccelEntry>,
    array_meta: &HashMap<String, ArrayMeta>,
    f16_acc: bool,
    sched: &mut GpuSchedule,
) {
    let consts = module_consts(program);
    for (a, b, kind) in &sched.edges {
        if *kind != EdgeKind::Raw {
            continue;
        }
        let Some(consumer) = accel.get(b) else {
            continue;
        };
        let Some(callable) = transaction(program, b) else {
            continue;
        };
        let kernel = super::accel::kernel_statements(program, callable, &consumer.shape.index_var);
        let Some((out_field, in_field, scale)) = detect_scale(&kernel, &consts) else {
            continue;
        };
        if !producer_writes(sched, a, &in_field) || readers_of(sched, &in_field) != 1 {
            continue;
        }
        let elem = array_meta.get(&in_field).map(|m| m.elem).unwrap_or(0);
        if !fusion_applies(elem, f16_acc) {
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
}

fn transaction<'a>(program: &'a HirProgram, name: &str) -> Option<&'a HirCallable> {
    schedule_callables(program).find(|c| c.name == name)
}

fn producer_writes(sched: &GpuSchedule, producer: &str, field: &str) -> bool {
    sched
        .writes
        .iter()
        .find(|(n, _)| n == producer)
        .map_or(false, |(_, ws)| ws.iter().any(|w| w == field))
}

fn readers_of(sched: &GpuSchedule, field: &str) -> usize {
    sched
        .reads
        .iter()
        .filter(|(_, rs)| rs.iter().any(|r| r == field))
        .count()
}

/// Detect a pure elementwise scale: `out[idx] = in[idx] * k` (or `k * in[idx]`).
fn detect_scale(stmts: &[HirStmt], consts: &HashMap<String, f64>) -> Option<(String, String, f64)> {
    let assign = stmts.iter().find_map(|s| match &s.kind {
        HirStmtKind::Assign(lhs, rhs) => Some((lhs, rhs)),
        _ => None,
    })?;
    let (lhs, rhs) = assign;
    let out_field = index_identifier(lhs)?;
    let HirExprKind::Binary {
        op: BinaryOpKind::Mul,
        left,
        right,
    } = &rhs.kind
    else {
        return None;
    };
    let (in_field, k) = match (index_identifier(left), as_f64(right, consts)) {
        (Some(f), Some(k)) => (f, k),
        _ => (index_identifier(right)?, as_f64(left, consts)?),
    };
    Some((out_field, in_field, k))
}

fn index_identifier(e: &HirExpr) -> Option<String> {
    let HirExprKind::Index(base, _) = &e.kind else {
        return None;
    };
    match &base.kind {
        HirExprKind::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

fn as_f64(e: &HirExpr, consts: &HashMap<String, f64>) -> Option<f64> {
    match &e.kind {
        HirExprKind::Literal(HirLiteral::Float(k)) => Some(*k),
        HirExprKind::Literal(HirLiteral::Int(k)) => Some(*k as f64),
        HirExprKind::Identifier(s) => consts.get(s).copied(),
        _ => None,
    }
}

/// Module-level const float/int literals (for folding scale multipliers).
fn module_consts(program: &HirProgram) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for (name, expr) in &program.const_exprs {
        match &expr.kind {
            HirExprKind::Literal(HirLiteral::Float(f)) => {
                m.insert(name.clone(), *f);
            }
            HirExprKind::Literal(HirLiteral::Int(n)) => {
                m.insert(name.clone(), *n as f64);
            }
            _ => {}
        }
    }
    m
}

fn detect_chain_fusion(
    sched: &GpuSchedule,
    fusions: &[Fusion],
    array_meta: &HashMap<String, ArrayMeta>,
) -> Option<ChainFusion> {
    for f in fusions {
        let mid_out = &f.out_field;
        let mid_out_readers: Vec<&String> = sched
            .reads
            .iter()
            .filter(|(_, rs)| rs.iter().any(|r| r == mid_out))
            .map(|(n, _)| n)
            .collect();
        if mid_out_readers.len() != 1 {
            continue;
        }
        let consumer = mid_out_readers[0].clone();
        if consumer == f.consumer {
            continue;
        }
        let ws = sched
            .writes
            .iter()
            .find(|(n, _)| n == &consumer)
            .map(|(_, ws)| ws.clone())
            .unwrap_or_default();
        let terminal = ws.iter().find(|w| {
            let wstr = w.as_str();
            array_meta.contains_key(wstr)
                && !sched
                    .reads
                    .iter()
                    .any(|(rn, rrs)| rn != &consumer && rrs.iter().any(|r| r.as_str() == wstr))
        });
        let Some(o) = terminal else { continue };
        if ws.iter().any(|w| w == &f.in_field) {
            continue;
        }
        return Some(ChainFusion {
            producer: f.producer.clone(),
            middle: f.consumer.clone(),
            consumer,
            mid_in: f.in_field.clone(),
            mid_out: mid_out.clone(),
            scale: f.scale,
        });
    }
    None
}

fn compute_array_last_use(sched: &mut GpuSchedule) {
    let all_arrays: BTreeSet<String> = sched
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

fn txn_touches_array(sched: &GpuSchedule, n: &str, arr: &str) -> bool {
    let touches = |sets: &[(String, Vec<String>)]| {
        sets.iter()
            .find(|(nm, _)| nm == n)
            .map_or(false, |(_, bs)| bs.iter().any(|b| b == arr))
    };
    touches(&sched.reads) || touches(&sched.writes)
}

fn txn_writes_array(sched: &GpuSchedule, n: &str, arr: &str) -> bool {
    sched
        .writes
        .iter()
        .find(|(nm, _)| nm == n)
        .map_or(false, |(_, bs)| bs.iter().any(|b| b == arr))
}

fn arrays_touched(sched: &GpuSchedule, txn: &str) -> BTreeSet<String> {
    sched
        .reads
        .iter()
        .chain(sched.writes.iter())
        .filter(|(nm, _)| nm == txn)
        .flat_map(|(_, bs)| bs.iter().cloned())
        .collect()
}

fn compute_array_first_use(sched: &GpuSchedule) -> HashMap<String, String> {
    let mut first_use: HashMap<String, String> = HashMap::new();
    for n in &sched.order {
        record_first_use(sched, n, &mut first_use);
    }
    first_use
}

fn record_first_use(sched: &GpuSchedule, txn: &str, first_use: &mut HashMap<String, String>) {
    for arr in arrays_touched(sched, txn) {
        first_use.entry(arr).or_insert_with(|| txn.to_string());
    }
}

/// Tightest free slot: largest last-use still before `first_use`, same size.
fn tightest_free_slot(
    slot_owner: &[(String, usize, u64)],
    first_use: usize,
    size: u64,
) -> Option<usize> {
    slot_owner
        .iter()
        .enumerate()
        .filter(|(_, (_, last, bytes))| *last < first_use && *bytes == size)
        .max_by_key(|(_, (_, last, _))| *last)
        .map(|(i, _)| i)
}

fn compute_reuse_opportunities(sched: &mut GpuSchedule, array_meta: &HashMap<String, ArrayMeta>) {
    let first_use = compute_array_first_use(sched);
    let order_idx: HashMap<&str, usize> = sched
        .order
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
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
    let mut slot_owner: Vec<(String, usize, u64)> = Vec::new();
    for (name, fi, li, write_first) in &arrays {
        if *write_first {
            let size = array_meta.get(name).map(|m| m.bytes).unwrap_or(0);
            if let Some(pos) = tightest_free_slot(&slot_owner, *fi, size) {
                let (owner, _, _) = slot_owner[pos].clone();
                let after = sched
                    .array_last_use
                    .get(&owner)
                    .cloned()
                    .unwrap_or_default();
                sched.reuse_opportunities.push((owner, name.clone(), after));
                slot_owner[pos] = (name.clone(), *li, size);
                continue;
            }
        }
        slot_owner.push((
            name.clone(),
            *li,
            array_meta.get(name).map(|m| m.bytes).unwrap_or(0),
        ));
    }
}

/// Test-only HIR dump: build, then canonicalise the whole schedule.
#[cfg(test)]
pub(crate) fn schedule_for_test(
    program: &HirProgram,
    accel: &HashMap<String, AccelEntry>,
    array_meta: &HashMap<String, ArrayMeta>,
    f16_acc: bool,
) -> std::collections::BTreeMap<String, String> {
    crate::analysis::gpu_schedule::canonical_schedule(&build_schedule(
        program, accel, array_meta, f16_acc,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::accel::AccelMode;
    use crate::ast::TopLevel;
    use crate::type_universe::TypeUniverse;

    fn parse_lower(src: &str) -> (Vec<TopLevel>, HirProgram) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let program = crate::hir::lower_program(&items);
        (items, program)
    }

    fn sizes(pairs: &[(&str, u64, u64)]) -> HashMap<String, ArrayMeta> {
        pairs
            .iter()
            .map(|(n, b, e)| {
                (
                    n.to_string(),
                    ArrayMeta {
                        bytes: *b,
                        elem: *e,
                    },
                )
            })
            .collect()
    }

    fn parity(src: &str, meta: &HashMap<String, ArrayMeta>, f16_acc: bool) -> GpuSchedule {
        let (items, program) = parse_lower(src);
        let universe = TypeUniverse::new();
        let ast_accel =
            crate::analysis::accel::analyze(&items, Some(&universe), Some(AccelMode::TryAll));
        let hir_accel =
            crate::hir::accel::analyze(&program, Some(&universe), Some(AccelMode::TryAll));
        let ast = crate::analysis::gpu_schedule::build_schedule(&items, &ast_accel, meta, f16_acc);
        assert_eq!(
            schedule_for_test(&program, &hir_accel, meta, f16_acc),
            crate::analysis::gpu_schedule::canonical_schedule(&ast),
            "HIR/AST gpu-schedule drift:\n{src}"
        );
        build_schedule(&program, &hir_accel, meta, f16_acc)
    }

    const CHAIN_SRC: &str = r#"
const N: Int = 64;
let q: Float[64];
let kt: Float[64];
let s: Float[64];
let s2: Float[64];
let v: Float[64];
let o: Float[64];
let i: Int = 0;
let j: Int = 0;
let k: Int = 0;
accel node pv [k < N][k == N] {
    o[k] = s2[k] + v[k];
    k = k + 1;
    term;
};
accel node scale [j < N][j == N] {
    s2[j] = s[j] * 2.0;
    j = j + 1;
    term;
};
accel node qk [i < N][i == N] {
    s[i] = q[i] + kt[i];
    i = i + 1;
    term;
};
"#;

    fn chain_meta() -> HashMap<String, ArrayMeta> {
        sizes(&[
            ("q", 256, 4),
            ("kt", 256, 4),
            ("s", 256, 4),
            ("s2", 256, 4),
            ("v", 256, 4),
            ("o", 256, 4),
        ])
    }

    #[test]
    fn chain_dag_orders_producer_first() {
        let sched = parity(CHAIN_SRC, &chain_meta(), false);
        let pos = |n: &str| sched.order.iter().position(|x| x == n);
        assert!(pos("qk") < pos("scale"), "order: {:?}", sched.order);
        assert!(pos("scale") < pos("pv"), "order: {:?}", sched.order);
    }

    #[test]
    fn detects_epilogue_scale_fusion_and_f16_gate() {
        let src = r#"
const N: Int = 64;
let q: Float[64];
let kt: Float[64];
let s: Float[64];
let s2: Float[64];
let i: Int = 0;
let j: Int = 0;
accel node gemm1 [i < N][i == N] {
    s[i] = q[i] + kt[i];
    i = i + 1;
    term;
};
accel node scale [j < N][j == N] {
    s2[j] = s[j] * 2.0;
    j = j + 1;
    term;
};
"#;
        let f32_meta = sizes(&[("q", 4, 4), ("kt", 4, 4), ("s", 4, 4), ("s2", 4, 4)]);
        let sched = parity(src, &f32_meta, false);
        assert_eq!(sched.fusions.len(), 1, "one fusion expected");
        let f = &sched.fusions[0];
        assert_eq!(f.producer, "gemm1");
        assert_eq!(f.consumer, "scale");
        assert_eq!(f.out_field, "s2");
        assert_eq!(f.in_field, "s");
        assert!((f.scale - 2.0).abs() < 1e-9);
        let int_meta = sizes(&[("q", 8, 8), ("kt", 8, 8), ("s", 8, 8), ("s2", 8, 8)]);
        assert!(parity(src, &int_meta, false).fusions.is_empty());
        let f16_meta = sizes(&[("q", 2, 2), ("kt", 2, 2), ("s", 2, 2), ("s2", 2, 2)]);
        assert!(parity(src, &f16_meta, false).fusions.is_empty());
        assert_eq!(parity(src, &f16_meta, true).fusions.len(), 1);
    }

    #[test]
    fn array_last_use_and_slot_reuse() {
        // f16 metadata without the f16-acc tier: the epilogue gate keeps the
        // scale a separate consumer, so last-use/slot-reuse see every node.
        let meta = sizes(&[
            ("q", 128, 2),
            ("kt", 128, 2),
            ("s", 128, 2),
            ("s2", 128, 2),
            ("v", 128, 2),
            ("o", 128, 2),
        ]);
        let sched = parity(CHAIN_SRC, &meta, false);
        assert!(sched.fusions.is_empty(), "f16 without acc must not fuse");
        assert_eq!(
            sched.array_last_use.get("s").map(|s| s.as_str()),
            Some("scale")
        );
        assert_eq!(
            sched.array_last_use.get("s2").map(|s| s.as_str()),
            Some("pv")
        );
        assert_eq!(
            sched.array_last_use.get("q").map(|s| s.as_str()),
            Some("qk")
        );
        assert_eq!(
            sched.array_first_use.get("s").map(|s| s.as_str()),
            Some("qk")
        );
        assert_eq!(
            sched.array_first_use.get("s2").map(|s| s.as_str()),
            Some("scale")
        );
        assert_eq!(
            sched.array_first_use.get("o").map(|s| s.as_str()),
            Some("pv")
        );
        assert!(
            sched
                .reuse_opportunities
                .iter()
                .any(|(d, n, _)| d == "q" && n == "s2"),
            "q slot reused by s2: {:?}",
            sched.reuse_opportunities
        );
        assert!(
            sched
                .reuse_opportunities
                .iter()
                .any(|(d, n, _)| d == "s" && n == "o"),
            "s slot reused by o: {:?}",
            sched.reuse_opportunities
        );
    }

    #[test]
    fn detects_chain_fusion() {
        let sched = parity(CHAIN_SRC, &chain_meta(), true);
        let cf = sched.chain_fusion.expect("chain fusion detected");
        assert_eq!(cf.producer, "qk");
        assert_eq!(cf.middle, "scale");
        assert_eq!(cf.consumer, "pv");
        assert_eq!(cf.mid_in, "s");
        assert_eq!(cf.mid_out, "s2");
        assert!((cf.scale - 2.0).abs() < 1e-9);
    }

    #[test]
    fn independent_kernels_have_no_edge() {
        let src = r#"
let a: Float[64];
let b: Float[64];
let c: Float[64];
let d: Float[64];
let i: Int = 0;
let j: Int = 0;
accel node first [i < 64][i == 64] {
    b[i] = a[i];
    i = i + 1;
    term;
};
accel node second [j < 64][j == 64] {
    d[j] = c[j];
    j = j + 1;
    term;
};
"#;
        let meta = sizes(&[("a", 256, 4), ("b", 256, 4), ("c", 256, 4), ("d", 256, 4)]);
        let sched = parity(src, &meta, false);
        assert!(sched
            .independent
            .contains(&("first".into(), "second".into())));
        assert!(sched.edges.is_empty(), "edges: {:?}", sched.edges);
    }

    #[test]
    fn empty_program_is_clean() {
        let sched = parity("", &HashMap::new(), false);
        assert!(sched.order.is_empty());
        assert!(sched.edges.is_empty());
        assert!(sched.fusions.is_empty());
        assert!(sched.chain_fusion.is_none());
    }
}
