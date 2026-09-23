//! 2026-08-15 (coll grow-on-full, plan 2026-08-15-coll-loop-guard-elimination):
//! prove which `(txn, coll)` pairs can NEVER overflow — the coll's length stays
//! below its initial capacity across the txn's firing sequence — so the
//! grow-on-full guard is dead and the backend can strip it from the inlined
//! push. Frontend-driven dispatch: the compiler knows the loop, so it does not
//! pay for a guard it can prove dead (queue_drain_idio 0.58x → 4.00x was the
//! opaque resize call blocking LLVM's if-conversion).
//!
//! SOUNDNESS: every tracked quantity is an upper bound (the max over all
//! paths). A gate is provable only when the bound is below the cap; anything
//! unknown or unbounded simply fails to prove and the guard stays. The contract
//! never weakens. The post-store-check alternative is unsound (Resize#(h, len)
//! sets cap == len; the next push would store OOB) and is rejected here.

use crate::ast::{Expr, Statement, TopLevel, Transaction, Type};
use std::collections::{HashMap, HashSet};

/// The scaffold's fresh-coll capacity (`coll_scaffold` synth_init / synth_init_empty).
/// Shared so the proof bound and the emitted default never drift.
pub(crate) const COLL_DEFAULT_CAP: i64 = 16;

/// Capacity-write intrinsics: calling any of these on a coll makes its capacity
/// unknown → the coll can no longer be proven non-overflowing.
const CAPACITY_WRITES: [&str; 3] = ["Resize#", "EnsureCap#", "TrimCap#"];

/// Known capacity/read intrinsics on a coll — reads only, harmless to the proof.
const CAPACITY_READS: [&str; 2] = ["Capacity#", "Size#"];

/// Per-coll tracked state: `len` is the MAX over all paths of the current
/// length (an upper bound); `max` is the peak; `cap` is the known capacity
/// (fresh-coll default, or unknown once a capacity write is seen).
#[derive(Debug, Clone)]
struct Track {
    cap: i64,
    len: i64,
    max: i64,
    known: bool,
    /// The coll obj type base name (for the safe-set output).
    base: String,
    /// A LOCAL coll (created in the txn body) is fresh per firing — the
    /// per-firing non-growth gate does not apply to it.
    is_local: bool,
}

impl Track {
    fn new(cap: i64, base: &str, is_local: bool) -> Track {
        Track { cap, len: 0, max: 0, known: true, base: base.to_string(), is_local }
    }
    fn push(&mut self) {
        if !self.known {
            return;
        }
        self.len += 1;
        self.max = self.max.max(self.len);
    }
    fn pop(&mut self) {
        if !self.known {
            return;
        }
        self.len = (self.len - 1).max(0);
    }
    fn unknown_cap(&mut self) {
        self.cap = -1;
    }
    fn fail(&mut self) {
        self.known = false;
    }
}

/// Compute the set of `(txn, coll_obj_type)` pairs whose grow-on-full guard is
/// provably dead. `items` is the full AST (TypeDefs, state fields, txns).
pub fn analyze(items: &[TopLevel]) -> HashSet<(String, String)> {
    let coll_obj: HashSet<String> = items
        .iter()
        .filter_map(|it| match it {
            TopLevel::TypeDef(t) if t.coll => Some(t.name.clone()),
            _ => None,
        })
        .collect();
    if coll_obj.is_empty() {
        return HashSet::new();
    }
    let state_inits = collect_state_inits(items, &coll_obj);
    let shared = shared_writers(items, &state_inits);
    collect_safe_pairs(items, &state_inits, &shared, &coll_obj)
}

/// Compute the pre-grow facts: `(txn, coll_name) -> peak` for LOCAL colls
/// (fresh per firing) whose intra-firing peak exceeds the default cap. The
/// backend emits one `EnsureCap#(q, peak)` at the coll's construction and the
/// per-push grow guard becomes dead (cap == peak >= len on every path). The
/// existing `analyze` gate is strict (`max < cap`) so a coll that genuinely
/// exceeds the cap never proves; pre-grow is the complementary route: raise
/// the cap ONCE up front instead of growing on each push.
///
/// SOUNDNESS (2026-08-16, plan three-track Phase 2): only LOCAL colls qualify
/// — a state coll grows across firings unboundedly, pre-grow to the intra-
/// firing peak would NOT cover a later firing. A declared `op Grow` binding
/// (`coll obj G { data: Ptr<Int>; op Grow: triple(#Lh); }`) is a user contract
/// and must not be bypassed — pre-grow is skipped for such bases. The walker's
/// `max` is an upper bound over all paths (rule: anything unknown fails the
/// track), so `cap = max >= peak` on every path.
pub fn analyze_pregrow(items: &[TopLevel]) -> HashMap<(String, String), i64> {
    let coll_obj: HashSet<String> = items
        .iter()
        .filter_map(|it| match it {
            TopLevel::TypeDef(t) if t.coll => Some(t.name.clone()),
            _ => None,
        })
        .collect();
    if coll_obj.is_empty() {
        return HashMap::new();
    }
    let declared_grow = declared_grow_bases(items);
    let state_inits = collect_state_inits(items, &coll_obj);
    let shared = shared_writers(items, &state_inits);
    let mut pre_grow = HashMap::new();
    let ctx = PregrowCtx {
        state_inits: &state_inits,
        shared: &shared,
        coll_obj: &coll_obj,
        declared_grow: &declared_grow,
    };
    for item in items {
        if let TopLevel::Transaction(t) = item {
            collect_txn_pregrow(t, &ctx, &mut pre_grow);
        }
    }
    pre_grow
}

/// Coll bases declaring a custom `op Grow` — pre-grow must not bypass the
/// declared growth strategy.
fn declared_grow_bases(items: &[TopLevel]) -> HashSet<String> {
    items
        .iter()
        .filter_map(|it| match it {
            TopLevel::TypeDef(t)
                if t.coll && t.body.op_bindings.iter().any(|b| b.name == "Grow") =>
            {
                Some(t.name.clone())
            }
            _ => None,
        })
        .collect()
}

/// Context shared by the pre-grow walk across all txns.
struct PregrowCtx<'a> {
    state_inits: &'a HashMap<String, (String, i64)>,
    shared: &'a HashSet<String>,
    coll_obj: &'a HashSet<String>,
    declared_grow: &'a HashSet<String>,
}

/// Emit `(txn, coll_name) -> peak` for every local coll in ONE txn whose
/// intra-firing peak exceeds the default cap.
fn collect_txn_pregrow(
    t: &Transaction,
    ctx: &PregrowCtx<'_>,
    out: &mut HashMap<(String, String), i64>,
) {
    let mut tracks = seed_tracks(ctx.state_inits, ctx.shared);
    walk_body(&t.body, &mut tracks, ctx.coll_obj);
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

/// State-field coll inits: name → (base coll type, initial list length).
/// An unknown initializer length (−1) or a legacy `StateDecl` cannot prove.
fn collect_state_inits(
    items: &[TopLevel],
    coll_obj: &HashSet<String>,
) -> HashMap<String, (String, i64)> {
    let mut state_inits = HashMap::new();
    for item in items {
        match item {
            TopLevel::StateDecl(s) => {
                if let Some((base, _)) = coll_base(&s.ty, coll_obj) {
                    state_inits.insert(s.name.clone(), (base, -1));
                }
            }
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, ty, expr, .. } = stmt.as_ref() {
                    if let Some((base, _)) = ty.as_ref().and_then(|t| coll_base(t, coll_obj)) {
                        let init_len = match expr.as_ref() {
                            Some(Expr::List(elems)) => elems.len() as i64,
                            _ => -1,
                        };
                        state_inits.insert(name.clone(), (base, init_len));
                    }
                }
            }
            _ => {}
        }
    }
    state_inits
}

/// Colls written (pushed/popped/resized) by MORE than one txn have an unknown
/// entry length for any txn → cannot prove.
fn shared_writers(
    items: &[TopLevel],
    state_inits: &HashMap<String, (String, i64)>,
) -> HashSet<String> {
    let mut writers: HashMap<String, HashSet<String>> = HashMap::new();
    for item in items {
        if let TopLevel::Transaction(t) = item {
            for name in colls_written_by_txn(&t.body, state_inits) {
                writers.entry(name.clone()).or_default().insert(t.name.clone());
            }
        }
    }
    writers
        .iter()
        .filter(|(_, txns)| txns.len() > 1)
        .map(|(n, _)| n.clone())
        .collect()
}

/// Prove each txn's state-field colls; collect the `(txn, base)` pairs.
fn collect_safe_pairs(
    items: &[TopLevel],
    state_inits: &HashMap<String, (String, i64)>,
    shared: &HashSet<String>,
    coll_obj: &HashSet<String>,
) -> HashSet<(String, String)> {
    let mut safe = HashSet::new();
    for item in items {
        if let TopLevel::Transaction(t) = item {
            let proven = prove_txn(&t.body, state_inits, shared, coll_obj);
            for (coll, base) in proven {
                safe.insert((t.name.clone(), base));
            }
        }
    }
    safe
}

/// The base coll-obj type name for a type annotation, if it is one.
fn coll_base(ty: &Type, coll_obj: &HashSet<String>) -> Option<(String, Vec<Type>)> {
    match ty {
        Type::Custom(n) => {
            let base = n.split('<').next().unwrap_or(n).to_string();
            if coll_obj.contains(&base) {
                Some((base, Vec::new()))
            } else {
                None
            }
        }
        Type::Applied(n, args) => {
            if coll_obj.contains(n) {
                Some((n.clone(), args.clone()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Names of colls that a txn body writes (push/pop/capacity-write). Used for
/// the multiple-writer gate.
fn colls_written_by_txn(
    body: &[Statement],
    state_inits: &HashMap<String, (String, i64)>,
) -> HashSet<String> {
    let mut out = HashSet::new();
    collect_writes(body, state_inits, &mut out);
    out
}

fn collect_writes(
    body: &[Statement],
    state_inits: &HashMap<String, (String, i64)>,
    out: &mut HashSet<String>,
) {
    for stmt in body {
        match stmt {
            Statement::ArrowAssign { target, value, .. } => {
                if let Some(t) = target.as_deref() {
                    record_coll(t, state_inits, out);
                }
                record_coll(value, state_inits, out);
            }
            Statement::Expression(e) => collect_expr_coll_writes(e, state_inits, out),
            Statement::Assign(lhs, _) => record_coll(lhs, state_inits, out),
            Statement::Guarded(_, body) => collect_writes(body, state_inits, out),
            Statement::Block(body) => collect_writes(body, state_inits, out),
            Statement::Foreach { body, .. } => collect_writes(body, state_inits, out),
            Statement::Defer(b) | Statement::Mutex(b) | Statement::SyncBlock(b) => {
                collect_writes(b, state_inits, out)
            }
            _ => {}
        }
    }
}

/// Record `e` as a written coll if it names a tracked state-field coll.
fn record_coll(e: &Expr, state_inits: &HashMap<String, (String, i64)>, out: &mut HashSet<String>) {
    if let Some(n) = coll_name_of(e) {
        if is_coll_field(n, state_inits) {
            out.insert(n.to_string());
        }
    }
}

fn collect_expr_coll_writes(
    e: &Expr,
    state_inits: &HashMap<String, (String, i64)>,
    out: &mut HashSet<String>,
) {
    match e {
        Expr::MethodCall(recv, name, _, _, _) => {
            if name == "push" || name == "pop" {
                insert_if_field(recv, state_inits, out);
            }
        }
        Expr::Call(name, args, _) => {
            if CAPACITY_WRITES.contains(&name.as_str()) {
                if let Some(arg) = args.first() {
                    insert_if_field(arg, state_inits, out);
                }
            }
        }
        _ => {}
    }
}

/// Insert the expr's coll name into `out` if it names a tracked state field.
fn insert_if_field(
    e: &Expr,
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

fn coll_name_of(e: &Expr) -> Option<&str> {
    match e {
        Expr::Identifier(n) => Some(n),
        Expr::Index(base, _) => coll_name_of(base),
        Expr::Field(base, _) => coll_name_of(base),
        Expr::MethodCall(recv, _, _, _, _) => coll_name_of(recv),
        _ => None,
    }
}

/// Prove which state-field colls in a txn body never overflow. Returns
/// `(coll_name, base_type)` pairs.
fn prove_txn(
    body: &[Statement],
    state_inits: &HashMap<String, (String, i64)>,
    shared: &HashSet<String>,
    coll_obj: &HashSet<String>,
) -> Vec<(String, String)> {
    let mut tracks = seed_tracks(state_inits, shared);
    // The entry length for a state field is its initializer's length; the
    // per-firing net delta must be ≤ 0 (non-growing across firings) for the
    // repetition to be safe. Locals are fresh per firing — no delta gate.
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

/// Seed a track per state-field coll: the fresh-coll capacity and the
/// initializer's length. A shared writer or an unknown initializer cannot
/// prove (the track is simply not seeded).
fn seed_tracks(
    state_inits: &HashMap<String, (String, i64)>,
    shared: &HashSet<String>,
) -> HashMap<String, Track> {
    let mut tracks = HashMap::new();
    for (name, (base, init_len)) in state_inits {
        if shared.contains(name) || *init_len < 0 {
            continue;
        }
        tracks.insert(name.clone(), Track::new(COLL_DEFAULT_CAP, base, false));
        if let Some(t) = tracks.get_mut(name) {
            t.len = *init_len;
            t.max = *init_len;
        }
    }
    tracks
}

/// The four gates: the track is known, the txn actually pushes (the strip is
/// per-push-site), the per-firing net delta is non-growing (state fields
/// only — locals are fresh per firing), and the peak stays below the known
/// capacity.
fn provably_safe(t: &Track, name: &str, body: &[Statement], entry: &HashMap<String, i64>) -> bool {
    let pushed = body_contains_push(body, name);
    if !pushed {
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

fn body_contains_push(body: &[Statement], coll: &str) -> bool {
    body.iter().any(|s| stmt_contains_push(s, coll))
}

fn stmt_contains_push(stmt: &Statement, coll: &str) -> bool {
    match stmt {
        Statement::ArrowAssign { target, .. } => {
            if let Some(t) = target.as_deref() {
                coll_name_of(t) == Some(coll)
            } else {
                false
            }
        }
        Statement::Expression(e) => expr_contains_push(e, coll),
        Statement::Guarded(_, b) => b.iter().any(|s| stmt_contains_push(s, coll)),
        Statement::Block(b) => b.iter().any(|s| stmt_contains_push(s, coll)),
        Statement::Foreach { body, .. } => body.iter().any(|s| stmt_contains_push(s, coll)),
        Statement::Defer(b) | Statement::Mutex(b) | Statement::SyncBlock(b) => {
            b.iter().any(|s| stmt_contains_push(s, coll))
        }
        _ => false,
    }
}

fn expr_contains_push(e: &Expr, coll: &str) -> bool {
    match e {
        Expr::MethodCall(recv, name, _, _, _) => name == "push" && coll_name_of(recv) == Some(coll),
        _ => false,
    }
}

fn walk_body(body: &[Statement], tracks: &mut HashMap<String, Track>, coll_obj: &HashSet<String>) {
    for stmt in body {
        walk_stmt(stmt, tracks, coll_obj);
        if matches!(stmt, Statement::Term(_) | Statement::EndProgram(_)) {
            break;
        }
    }
}

fn walk_stmt(stmt: &Statement, tracks: &mut HashMap<String, Track>, coll_obj: &HashSet<String>) {
    match stmt {
        Statement::Let { name, ty, expr, .. } => {
            // A local that SHADOWS a tracked state field makes the field's
            // track stale — the identity and length are unknown for the field.
            if tracks.contains_key(name) {
                if let Some(t) = tracks.get_mut(name) {
                    t.fail();
                }
            }
            // 2026-08-16 (Direction 2): a coll obj LOCAL created in the txn
            // body (`let q: MyQueue = [..]`) is fresh per firing — seed a
            // track with the fresh-coll capacity; the intra-body peak is the
            // only bound (no per-firing repetition). The grow guard then
            // strips when the peak stays below the default cap.
            if let Some((base, _)) = ty.as_ref().and_then(|t| coll_base(t, coll_obj)) {
                let init_len = match expr.as_ref() {
                    Some(Expr::List(elems)) => elems.len() as i64,
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
        Statement::ArrowAssign { target, value, .. } => {
            // Only identifiers that are TRACKED colls count as collection
            // operands — `count` in `queue <- count` is a plain value.
            let lhs_coll = target
                .as_deref()
                .and_then(coll_name_of)
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
                    // `<- q` — a discard/extract on q.
                    if let Some(tr) = tracks.get_mut(&v) {
                        tr.pop();
                    }
                }
                (Some(_), Some(_)) => {
                    // coll-to-coll copy (`a <- b`) — unknown length growth.
                    for t in tracks.values_mut() {
                        t.fail();
                    }
                }
                (None, None) => {}
            }
        }
        Statement::Assign(lhs, _) => {
            // Reassigning a coll handle itself — the identity/len is unknown.
            if let Some(n) = coll_name_of(lhs) {
                if let Some(t) = tracks.get_mut(n) {
                    t.fail();
                }
            }
        }
        Statement::Expression(e) => walk_expr(e, tracks),
        Statement::Guarded(_, body) => {
            let before = tracks.clone();
            let mut fired = before.clone();
            walk_body(body, &mut fired, coll_obj);
            join_max(tracks, &fired, &before);
        }
        Statement::Block(b) | Statement::Defer(b) | Statement::Mutex(b) | Statement::SyncBlock(b) => {
            walk_body(b, tracks, coll_obj);
        }
        Statement::Foreach { list, body, .. } => {
            walk_foreach(list, body, tracks, coll_obj);
        }
        _ => {}
    }
}

fn walk_foreach(list: &Expr, body: &[Statement], tracks: &mut HashMap<String, Track>, coll_obj: &HashSet<String>) {
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
            // Unknown bound: any push on a coll in the body is unbounded.
            for (name, t) in tracks.iter_mut() {
                if body_contains_push(body, name) {
                    t.fail();
                }
            }
            *tracks = iter;
        }
    }
}

/// A conditional (`when`) that touches a coll makes the per-iteration
/// delta len-dependent — not a constant transform.
fn foreach_body_is_conditional(body: &[Statement]) -> bool {
    body.iter().any(|s| {
        matches!(s, Statement::Guarded(..)) && stmt_has_coll_op(s)
    })
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

/// Apply the body's constant transform `iters` times from the entry lengths.
/// `delta` is the per-iteration length change; `peak` is the intra-iteration
/// peak above entry. For a positive delta the peak over the loop is reached on
/// the last iteration; otherwise the first iteration's peak is the max.
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

fn stmt_has_coll_op(s: &Statement) -> bool {
    match s {
        Statement::ArrowAssign { .. } => true,
        Statement::Expression(Expr::MethodCall(_, name, _, _, _)) => name == "push" || name == "pop",
        _ => false,
    }
}

/// Statically-known `N` for a `0..N` / `0..=N` range literal; None otherwise.
fn range_len(e: &Expr) -> Option<i64> {
    match e {
        Expr::Range { start, end, inclusive } => {
            let s = match start.as_ref() {
                Expr::Decimal(v) => *v,
                _ => return None,
            };
            let en = match end.as_ref() {
                Expr::Decimal(v) => *v,
                _ => return None,
            };
            let count = if *inclusive { en - s + 1 } else { en - s };
            if count >= 0 {
                Some(count)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Merge two per-path track states: keep the max len, max peak, and the
/// conservative cap/known flags.
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

/// Merge one path state (`ta`, with optional other-path `tb`) into `t`.
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

fn walk_expr(e: &Expr, tracks: &mut HashMap<String, Track>) {
    match e {
        Expr::Call(name, args, _) => walk_call(name, args, tracks),
        Expr::MethodCall(recv, name, _, _, _) => {
            if let Some(n) = coll_name_of(recv) {
                if let Some(t) = tracks.get_mut(n) {
                    match name.as_str() {
                        "push" => t.push(),
                        "pop" => t.pop(),
                        _ => {} // reads (Count#, At#, .^Length, ...) — no change
                    }
                }
            }
        }
        Expr::BinaryOp(_, l, r) => {
            walk_expr(l, tracks);
            walk_expr(r, tracks);
        }
        Expr::UnaryOp(_, inner) => walk_expr(inner, tracks),
        Expr::Index(base, _) => walk_expr(base, tracks),
        Expr::Field(base, _) => walk_expr(base, tracks),
        Expr::Reflect(base, _, _) => walk_expr(base, tracks),
        Expr::List(elems) => {
            for el in elems {
                walk_expr(el, tracks);
            }
        }
        _ => {}
    }
}

fn walk_call(name: &str, args: &[Expr], tracks: &mut HashMap<String, Track>) {
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
    // Any other call passing a coll could mutate it through a defn.
    // Conservative: fail the coll.
    for arg in args {
        fail_if_coll(arg, tracks);
    }
}

fn fail_if_coll(e: &Expr, tracks: &mut HashMap<String, Track>) {
    if let Some(n) = coll_name_of(e) {
        if let Some(t) = tracks.get_mut(n) {
            t.fail();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().expect("parse")
    }

    fn pair(t: &str, c: &str) -> (String, String) {
        (t.to_string(), c.to_string())
    }

    /// A balanced drain (pop then push keeps len ≤ initial < cap) is provable —
    /// the queue_drain_idio shape.
    #[test]
    fn drain_balance_proves() {
        let items = parse(
            "coll obj Q { data: Ptr<Int>; };\n\
             let q: Q = [0];\n\
             let count: Int = 0;\n\
             node work [count < N][count == N] {\n\
               <- q;\n\
               q <- count;\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let safe = analyze(&items);
        assert!(
            safe.contains(&pair("work", "Q")),
            "balanced drain must prove, got {safe:?}"
        );
    }

    /// A monotone push loop (no pop) grows the coll across firings — never
    /// provable (the reactor repeats the body unboundedly).
    #[test]
    fn monotone_push_does_not_prove() {
        let items = parse(
            "coll obj Q { data: Ptr<Int>; };\n\
             let q: Q = [];\n\
             let count: Int = 0;\n\
             node work [count < N][count == N] {\n\
               q <- count;\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let safe = analyze(&items);
        assert!(
            !safe.contains(&pair("work", "Q")),
            "monotone push must NOT prove, got {safe:?}"
        );
    }

    /// A capacity write (Resize#) makes the cap unknown — not provable.
    #[test]
    fn capacity_write_does_not_prove() {
        let items = parse(
            "coll obj Q { data: Ptr<Int>; };\n\
             let q: Q = [0];\n\
             let count: Int = 0;\n\
             node work [count < N][count == N] {\n\
               <- q;\n\
               q <- count;\n\
               count = count + 1;\n\
               when count % 100 == 0 { Resize#(q, 64); };\n\
               term;\n\
             };\n",
        );
        let safe = analyze(&items);
        assert!(
            !safe.contains(&pair("work", "Q")),
            "a capacity write must NOT prove, got {safe:?}"
        );
    }

    /// A coll written by two txns has an unknown entry length — not provable.
    #[test]
    fn shared_writer_does_not_prove() {
        let items = parse(
            "coll obj Q { data: Ptr<Int>; };\n\
             let q: Q = [0];\n\
             let count: Int = 0;\n\
             let done: Int = 0;\n\
             node work [count < N][count == N] {\n\
               <- q;\n\
               q <- count;\n\
               count = count + 1;\n\
               term;\n\
             };\n\
             node other [done == 0][done == 1] {\n\
               q <- count;\n\
               done = 1;\n\
               term;\n\
             };\n",
        );
        let safe = analyze(&items);
        assert!(
            !safe.contains(&pair("work", "Q")),
            "a two-writer coll must NOT prove, got {safe:?}"
        );
    }

    /// A coll that exceeds the default cap within the firing (foreach with
    /// 21 pushes) must NOT prove — it genuinely grows (cap 16 → 32).
    #[test]
    fn foreach_beyond_cap_does_not_prove() {
        let items = parse(
            "coll obj Q { data: Ptr<Int>; };\n\
             let q: Q = [];\n\
             let done: Int = 0;\n\
             node work [done == 0][done == 1] {\n\
               foreach x in 0..21 { q <- x; };\n\
               done = 1;\n\
               term;\n\
             };\n",
        );
        let safe = analyze(&items);
        assert!(
            !safe.contains(&pair("work", "Q")),
            "21 pushes exceed cap 16 — must NOT prove, got {safe:?}"
        );
    }

    /// A coll obj LOCAL created in the txn body (`let q: Q = [5,6,7]` plus two
    /// pushes = 5 < cap 16) is fresh per firing — the intra-body peak proves
    /// and the grow guard strips.
    #[test]
    fn local_coll_within_cap_proves() {
        let items = parse(
            "coll obj Q { data: Ptr<Int>; };\n\
             let done: Int = 0;\n\
             node work [done == 0][done == 1] {\n\
               let q: Q = [5, 6, 7];\n\
               q <- 8;\n\
               q <- 9;\n\
               done = 1;\n\
               term;\n\
             };\n",
        );
        let safe = analyze(&items);
        assert!(
            safe.contains(&pair("work", "Q")),
            "a local coll with peak 5 < cap 16 must prove, got {safe:?}"
        );
    }

    /// A local coll grown past the default cap (foreach 21) must NOT prove —
    /// it genuinely grows.
    #[test]
    fn local_coll_beyond_cap_does_not_prove() {
        let items = parse(
            "coll obj Q { data: Ptr<Int>; };\n\
             let done: Int = 0;\n\
             node work [done == 0][done == 1] {\n\
               let q: Q = [];\n\
               foreach x in 0..21 { q <- x; };\n\
               done = 1;\n\
               term;\n\
             };\n",
        );
        let safe = analyze(&items);
        assert!(
            !safe.contains(&pair("work", "Q")),
            "21 pushes on a local exceed cap 16 — must NOT prove, got {safe:?}"
        );
    }
}
