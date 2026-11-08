// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.5 — coll grow-guard elimination / pre-grow proofs over HIR.
//! Mirror of `analysis::coll_length` (AST, authoritative until parity).

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::coll_length::{CAPACITY_READS, CAPACITY_WRITES};
use crate::analysis::coll_length::COLL_DEFAULT_CAP;
use crate::ast::Type;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
struct Track {
    cap: i64,
    len: i64,
    max: i64,
    is_local: bool,
    known: bool,
    base: String,
}

impl Track {
    fn new(cap: i64, base: &str, is_local: bool) -> Track {
        Track {
            cap,
            len: 0,
            max: 0,
            known: true,
            base: base.to_string(),
            is_local,
        }
    }
    fn push(&mut self) {
        self.len += 1;
        self.max = self.max.max(self.len);
    }
    fn pop(&mut self) {
        self.len = (self.len - 1).max(0);
    }
    fn unknown_cap(&mut self) {
        self.cap = -1;
    }
    fn fail(&mut self) {
        self.known = false;
    }
}

pub fn analyze(program: &HirProgram) -> HashSet<(String, String)> {
    let coll_obj: HashSet<String> = program
        .type_defs
        .iter()
        .filter(|t| t.is_coll)
        .map(|t| t.name.clone())
        .collect();
    if coll_obj.is_empty() {
        return HashSet::new();
    }
    let state_inits = collect_state_inits(program, &coll_obj);
    let shared = shared_writers(program, &state_inits);
    collect_safe_pairs(program, &state_inits, &shared, &coll_obj)
}

pub fn analyze_pregrow(program: &HirProgram) -> HashMap<(String, String), i64> {
    let coll_obj: HashSet<String> = program
        .type_defs
        .iter()
        .filter(|t| t.is_coll)
        .map(|t| t.name.clone())
        .collect();
    if coll_obj.is_empty() {
        return HashMap::new();
    }
    let declared_grow = declared_grow_bases(program);
    let state_inits = collect_state_inits(program, &coll_obj);
    let shared = shared_writers(program, &state_inits);
    let mut pre_grow = HashMap::new();
    let ctx = PregrowCtx {
        state_inits: &state_inits,
        shared: &shared,
        coll_obj: &coll_obj,
        declared_grow: &declared_grow,
    };
    for c in txns(program) {
        collect_txn_pregrow(c, &ctx, &mut pre_grow);
    }
    pre_grow
}

fn txns(program: &HirProgram) -> impl Iterator<Item = &super::program::HirCallable> {
    program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Transaction) && c.owner.is_none() && !c.exported
    })
}

fn declared_grow_bases(program: &HirProgram) -> HashSet<String> {
    program
        .type_defs
        .iter()
        .filter(|t| t.is_coll && t.op_bindings.iter().any(|b| b == "Grow"))
        .map(|t| t.name.clone())
        .collect()
}

struct PregrowCtx<'a> {
    state_inits: &'a HashMap<String, (String, i64)>,
    shared: &'a HashSet<String>,
    coll_obj: &'a HashSet<String>,
    declared_grow: &'a HashSet<String>,
}

fn collect_txn_pregrow(
    t: &super::program::HirCallable,
    ctx: &PregrowCtx<'_>,
    out: &mut HashMap<(String, String), i64>,
) {
    let mut tracks = seed_tracks(ctx.state_inits, ctx.shared);
    walk_body(&t.body.statements, &mut tracks, ctx.coll_obj);
    for (name, tr) in &tracks {
        if pregrow_qualifies(tr, ctx) {
            out.insert((t.name.clone(), name.clone()), tr.max);
        }
    }
}

fn pregrow_qualifies(tr: &Track, ctx: &PregrowCtx<'_>) -> bool {
    tr.is_local
        && tr.known
        && tr.cap >= 0
        && tr.max > COLL_DEFAULT_CAP
        && !ctx.declared_grow.contains(&tr.base)
}

fn collect_state_inits(
    program: &HirProgram,
    coll_obj: &HashSet<String>,
) -> HashMap<String, (String, i64)> {
    let mut state_inits = HashMap::new();
    for g in &program.globals {
        if g.is_state_decl {
            if let Some((base, _)) = g.ty.as_ref().and_then(|t| coll_base(t, coll_obj)) {
                state_inits.insert(g.name.clone(), (base, -1));
            }
            continue;
        }
        if let Some((base, _)) = g.ty.as_ref().and_then(|t| coll_base(t, coll_obj)) {
            let init_len = match g.expr.as_ref().map(|e| &e.kind) {
                Some(HirExprKind::List(elems)) => elems.len() as i64,
                _ => -1,
            };
            state_inits.insert(g.name.clone(), (base, init_len));
        }
    }
    state_inits
}

fn record_writers(
    t: &HirCallable,
    state_inits: &HashMap<String, (String, i64)>,
    writers: &mut HashMap<String, HashSet<String>>,
) {
    for name in colls_written_by_txn(&t.body.statements, state_inits) {
        writers.entry(name).or_default().insert(t.name.clone());
    }
}

fn insert_proven(
    txn_name: &str,
    proven: Vec<(String, String)>,
    safe: &mut HashSet<(String, String)>,
) {
    for (_coll, base) in proven {
        safe.insert((txn_name.to_string(), base));
    }
}

fn shared_writers(
    program: &HirProgram,
    state_inits: &HashMap<String, (String, i64)>,
) -> HashSet<String> {
    let mut writers: HashMap<String, HashSet<String>> = HashMap::new();
    for t in txns(program) {
        record_writers(t, state_inits, &mut writers);
    }
    writers
        .iter()
        .filter(|(_, txns)| txns.len() > 1)
        .map(|(n, _)| n.clone())
        .collect()
}

fn collect_safe_pairs(
    program: &HirProgram,
    state_inits: &HashMap<String, (String, i64)>,
    shared: &HashSet<String>,
    coll_obj: &HashSet<String>,
) -> HashSet<(String, String)> {
    let mut safe = HashSet::new();
    for t in txns(program) {
        let proven = prove_txn(&t.body.statements, state_inits, shared, coll_obj);
        insert_proven(t.name.as_str(), proven, &mut safe);
    }
    safe
}

fn coll_base(ty: &Type, coll_obj: &HashSet<String>) -> Option<(String, Vec<Type>)> {
    match ty {
        Type::Custom(n) => {
            let base = n.split('<').next().unwrap_or(n).to_string();
            coll_obj.contains(&base).then_some((base, Vec::new()))
        }
        Type::Applied(n, args) => {
            coll_obj.contains(n).then(|| (n.clone(), args.clone()))
        }
        _ => None,
    }
}

fn colls_written_by_txn(
    body: &[HirStmt],
    state_inits: &HashMap<String, (String, i64)>,
) -> HashSet<String> {
    let mut out = HashSet::new();
    collect_writes(body, state_inits, &mut out);
    out
}

fn collect_writes(
    body: &[HirStmt],
    state_inits: &HashMap<String, (String, i64)>,
    out: &mut HashSet<String>,
) {
    for stmt in body {
        match &stmt.kind {
            HirStmtKind::ArrowAssign { target, value, .. } => {
                if let Some(t) = target {
                    record_coll(t, state_inits, out);
                }
                record_coll(value, state_inits, out);
            }
            HirStmtKind::Expression(e) => collect_expr_coll_writes(e, state_inits, out),
            HirStmtKind::Assign(lhs, _) => record_coll(lhs, state_inits, out),
            HirStmtKind::Guarded(_, body)
            | HirStmtKind::Block(body)
            | HirStmtKind::Foreach { body, .. }
            | HirStmtKind::Defer(body)
            | HirStmtKind::Mutex(body)
            | HirStmtKind::SyncBlock(body) => collect_writes(body, state_inits, out),
            _ => {}
        }
    }
}

fn record_coll(
    e: &HirExpr,
    state_inits: &HashMap<String, (String, i64)>,
    out: &mut HashSet<String>,
) {
    if let Some(n) = coll_name_of(e) {
        if is_coll_field(n, state_inits) {
            out.insert(n.to_string());
        }
    }
}

fn collect_expr_coll_writes(
    e: &HirExpr,
    state_inits: &HashMap<String, (String, i64)>,
    out: &mut HashSet<String>,
) {
    match &e.kind {
        HirExprKind::MethodCall { receiver, name, .. } if name == "push" || name == "pop" => {
            insert_if_field(receiver, state_inits, out);
        }
        HirExprKind::Call { callee, args, .. } if CAPACITY_WRITES.contains(&callee.as_str()) => {
            if let Some(arg) = args.first() {
                insert_if_field(arg, state_inits, out);
            }
        }
        _ => {}
    }
}

fn insert_if_field(
    e: &HirExpr,
    state_inits: &HashMap<String, (String, i64)>,
    out: &mut HashSet<String>,
) {
    if let Some(n) = coll_name_of(e) {
        if is_coll_field(n, state_inits) {
            out.insert(n.to_string());
        }
    }
}

fn is_coll_field(name: &str, state_inits: &HashMap<String, (String, i64)>) -> bool {
    state_inits.contains_key(name)
}

fn coll_name_of(e: &HirExpr) -> Option<&str> {
    match &e.kind {
        HirExprKind::Identifier(n) => Some(n),
        HirExprKind::Index(base, _) => coll_name_of(base),
        HirExprKind::Field { receiver: base, .. } => coll_name_of(base),
        HirExprKind::MethodCall { receiver, .. } => coll_name_of(receiver),
        _ => None,
    }
}

fn prove_txn(
    body: &[HirStmt],
    state_inits: &HashMap<String, (String, i64)>,
    shared: &HashSet<String>,
    coll_obj: &HashSet<String>,
) -> Vec<(String, String)> {
    let mut tracks = seed_tracks(state_inits, shared);
    let entry: HashMap<String, i64> = tracks.iter().map(|(k, t)| (k.clone(), t.len)).collect();
    walk_body(body, &mut tracks, coll_obj);

    let mut out = Vec::new();
    for (name, t) in &tracks {
        if t.known && provably_safe(t, name, body, &entry) {
            out.push((name.clone(), t.base.clone()));
        }
    }
    out
}

fn seed_tracks(
    state_inits: &HashMap<String, (String, i64)>,
    shared: &HashSet<String>,
) -> HashMap<String, Track> {
    let mut tracks = HashMap::new();
    for (name, (base, init_len)) in state_inits {
        if shared.contains(name) || *init_len < 0 {
            continue;
        }
        let mut tr = Track::new(COLL_DEFAULT_CAP, base, false);
        tr.len = *init_len;
        tr.max = *init_len;
        tracks.insert(name.clone(), tr);
    }
    tracks
}

fn provably_safe(t: &Track, name: &str, body: &[HirStmt], entry: &HashMap<String, i64>) -> bool {
    if !body_contains_push(body, name) {
        return false;
    }
    if !t.is_local {
        let exit = t.len;
        let entry_len = entry.get(name).copied().unwrap_or(0);
        if exit > entry_len {
            return false;
        }
    }
    t.cap >= 0 && t.max < t.cap
}

fn body_contains_push(body: &[HirStmt], coll: &str) -> bool {
    body.iter().any(|s| stmt_contains_push(s, coll))
}

fn stmt_contains_push(stmt: &HirStmt, coll: &str) -> bool {
    match &stmt.kind {
        HirStmtKind::ArrowAssign { target, .. } => {
            target.as_ref().and_then(|t| coll_name_of(t)) == Some(coll)
        }
        HirStmtKind::Expression(e) => expr_contains_push(e, coll),
        HirStmtKind::Guarded(_, b) => b.iter().any(|s| stmt_contains_push(s, coll)),
        HirStmtKind::Block(b) => b.iter().any(|s| stmt_contains_push(s, coll)),
        HirStmtKind::Foreach { body, .. } => body.iter().any(|s| stmt_contains_push(s, coll)),
        HirStmtKind::Defer(b) | HirStmtKind::Mutex(b) | HirStmtKind::SyncBlock(b) => {
            b.iter().any(|s| stmt_contains_push(s, coll))
        }
        _ => false,
    }
}

fn expr_contains_push(e: &HirExpr, coll: &str) -> bool {
    match &e.kind {
        HirExprKind::MethodCall { receiver, name, .. } => {
            name == "push" && coll_name_of(receiver) == Some(coll)
        }
        _ => false,
    }
}

fn walk_body(body: &[HirStmt], tracks: &mut HashMap<String, Track>, coll_obj: &HashSet<String>) {
    for stmt in body {
        walk_stmt(stmt, tracks, coll_obj);
        if matches!(
            stmt.kind,
            HirStmtKind::Term(_) | HirStmtKind::EndProgram(_) | HirStmtKind::Tail(_)
        ) {
            break;
        }
    }
}

fn walk_stmt(stmt: &HirStmt, tracks: &mut HashMap<String, Track>, coll_obj: &HashSet<String>) {
    match &stmt.kind {
        HirStmtKind::Let { name, ty, expr, .. } => {
            if tracks.contains_key(name) {
                if let Some(t) = tracks.get_mut(name) {
                    t.fail();
                }
            }
            if let Some((base, _)) = ty.as_ref().and_then(|t| coll_base(t, coll_obj)) {
                let init_len = match expr.as_ref().map(|e| &e.kind) {
                    Some(HirExprKind::List(elems)) => elems.len() as i64,
                    _ => -1,
                };
                if init_len >= 0 {
                    let mut tr = Track::new(COLL_DEFAULT_CAP, &base, true);
                    tr.len = init_len;
                    tr.max = init_len;
                    tracks.insert(name.clone(), tr);
                }
            }
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            let lhs_coll = target
                .as_ref()
                .and_then(|t| coll_name_of(t))
                .filter(|n| tracks.contains_key(*n))
                .map(|s| s.to_string());
            let rhs_coll = coll_name_of(value)
                .filter(|n| tracks.contains_key(*n))
                .map(|s| s.to_string());
            match (lhs_coll, rhs_coll) {
                (Some(t), None) => {
                    if let Some(tr) = tracks.get_mut(&t) {
                        tr.push();
                    }
                }
                (None, Some(v)) => {
                    if let Some(tr) = tracks.get_mut(&v) {
                        tr.pop();
                    }
                }
                (Some(_), Some(_)) => {
                    for t in tracks.values_mut() {
                        t.fail();
                    }
                }
                (None, None) => {}
            }
        }
        HirStmtKind::Assign(lhs, _) => {
            if let Some(n) = coll_name_of(lhs) {
                if let Some(t) = tracks.get_mut(n) {
                    t.fail();
                }
            }
        }
        HirStmtKind::Expression(e) => walk_expr(e, tracks),
        HirStmtKind::Guarded(_, body) => {
            let before = tracks.clone();
            let mut fired = before.clone();
            walk_body(body, &mut fired, coll_obj);
            join_max(tracks, &fired, &before);
        }
        HirStmtKind::Block(b)
        | HirStmtKind::Defer(b)
        | HirStmtKind::Mutex(b)
        | HirStmtKind::SyncBlock(b) => walk_body(b, tracks, coll_obj),
        HirStmtKind::Foreach { list, body, .. } => walk_foreach(list, body, tracks, coll_obj),
        _ => {}
    }
}

fn walk_foreach(
    list: &HirExpr,
    body: &[HirStmt],
    tracks: &mut HashMap<String, Track>,
    coll_obj: &HashSet<String>,
) {
    let n = range_len(list);
    let before = tracks.clone();
    let mut iter = before.clone();
    walk_body(body, &mut iter, coll_obj);
    match n {
        Some(iters) if iters > 0 => {
            if foreach_body_is_conditional(body) {
                fail_changed(&mut iter, &before);
                *tracks = iter;
                return;
            }
            apply_foreach_transform(tracks, &before, &iter, iters);
        }
        _ => {
            for (name, t) in tracks.iter_mut() {
                if body_contains_push(body, name) {
                    t.fail();
                }
            }
            *tracks = iter;
        }
    }
}

fn foreach_body_is_conditional(body: &[HirStmt]) -> bool {
    body.iter()
        .any(|s| matches!(s.kind, HirStmtKind::Guarded(..)) && stmt_has_coll_op(s))
}

fn fail_changed(tracks: &mut HashMap<String, Track>, before: &HashMap<String, Track>) {
    let changed: Vec<String> = tracks
        .iter()
        .filter(|(name, t)| {
            let b_len = before.get(*name).map(|b| b.len).unwrap_or(0);
            t.len != b_len
        })
        .map(|(name, _)| name.clone())
        .collect();
    for name in changed {
        if let Some(t) = tracks.get_mut(&name) {
            t.fail();
        }
    }
}

fn apply_foreach_transform(
    tracks: &mut HashMap<String, Track>,
    before: &HashMap<String, Track>,
    iter: &HashMap<String, Track>,
    iters: i64,
) {
    for (name, t) in tracks.iter_mut() {
        let b_len = before.get(name).map(|b| b.len).unwrap_or(0);
        let i_len = iter.get(name).map(|i| i.len).unwrap_or(0);
        let i_max = iter.get(name).map(|i| i.max).unwrap_or(0);
        let delta = i_len - b_len;
        let peak = i_max - b_len;
        if delta > 0 {
            t.len = b_len + delta * iters;
            t.max = t.max.max(b_len + peak + delta * (iters - 1));
        } else {
            t.len = b_len + delta * iters;
            t.max = t.max.max(i_max);
        }
    }
}

fn stmt_has_coll_op(s: &HirStmt) -> bool {
    match &s.kind {
        HirStmtKind::ArrowAssign { .. } => true,
        HirStmtKind::Expression(HirExpr {
            kind: HirExprKind::MethodCall { name, .. },
            ..
        }) => name == "push" || name == "pop",
        _ => false,
    }
}

fn range_len(e: &HirExpr) -> Option<i64> {
    match &e.kind {
        HirExprKind::Range { start, end, inclusive } => {
            let s = match &start.kind {
                HirExprKind::Literal(super::expr::HirLiteral::Int(v)) => *v,
                _ => return None,
            };
            let en = match &end.kind {
                HirExprKind::Literal(super::expr::HirLiteral::Int(v)) => *v,
                _ => return None,
            };
            let count = if *inclusive { en - s + 1 } else { en - s };
            (count >= 0).then_some(count)
        }
        _ => None,
    }
}

fn join_max(
    tracks: &mut HashMap<String, Track>,
    a: &HashMap<String, Track>,
    b: &HashMap<String, Track>,
) {
    for (name, ta) in a {
        if let Some(t) = tracks.get_mut(name) {
            merge_path(t, ta, b.get(name));
        }
    }
}

fn merge_path(t: &mut Track, ta: &Track, tb: Option<&Track>) {
    let tb = tb.unwrap_or(ta);
    t.len = ta.len.max(tb.len);
    t.max = t.max.max(ta.max).max(tb.max);
    if !ta.known || !tb.known {
        t.known = false;
    }
    if ta.cap < 0 || tb.cap < 0 {
        t.cap = -1;
    }
}

fn walk_expr(e: &HirExpr, tracks: &mut HashMap<String, Track>) {
    match &e.kind {
        HirExprKind::Call { callee, args, .. } => walk_call(callee, args, tracks),
        HirExprKind::MethodCall { receiver, name, .. } => {
            if let Some(n) = coll_name_of(receiver) {
                if let Some(t) = tracks.get_mut(n) {
                    match name.as_str() {
                        "push" => t.push(),
                        "pop" => t.pop(),
                        _ => {}
                    }
                }
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            walk_expr(left, tracks);
            walk_expr(right, tracks);
        }
        HirExprKind::Unary { inner, .. } => walk_expr(inner, tracks),
        HirExprKind::Index(base, _) => walk_expr(base, tracks),
        HirExprKind::Field { receiver: base, .. } => walk_expr(base, tracks),
        HirExprKind::Reflect { receiver: base, .. } => walk_expr(base, tracks),
        HirExprKind::List(elems) => {
            for el in elems {
                walk_expr(el, tracks);
            }
        }
        _ => {}
    }
}

fn walk_call(name: &str, args: &[HirExpr], tracks: &mut HashMap<String, Track>) {
    if CAPACITY_WRITES.contains(&name) {
        if let Some(arg) = args.first() {
            if let Some(n) = coll_name_of(arg) {
                if let Some(t) = tracks.get_mut(n) {
                    t.unknown_cap();
                }
            }
        }
        return;
    }
    if CAPACITY_READS.contains(&name) {
        return;
    }
    for arg in args {
        fail_if_coll(arg, tracks);
    }
}

fn fail_if_coll(e: &HirExpr, tracks: &mut HashMap<String, Track>) {
    if let Some(n) = coll_name_of(e) {
        if let Some(t) = tracks.get_mut(n) {
            t.fail();
        }
    }
}
