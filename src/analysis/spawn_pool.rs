//! 2026-08-07 (object instance pools): predictably-inexhaustible pools.
//!
//! Briev has no runtime errors: a spawn pool must be PROVABLY inexhaustible.
//! This analysis computes, per obj base, the TOTAL lifetime spawn count
//! (each bounded firing context's spawns, summed across every node that
//! spawns the base — the monotonic `__spawn_next_<base>` counter is shared,
//! so its max is the sum), OR marks the pool DEPENDENT when the bound is a
//! runtime value.
//!
//! - A STATIC countdown (`[ticks < N]` with a compile-time N) sizes the
//!   member columns to the total spawn count — no runtime exhaustion path.
//! - A DEPENDENT countdown (`[ticks < N]` with N a runtime field/const name)
//!   still bounds the pool: the capacity is N at runtime, so the backend
//!   allocates the member columns as a runtime-sized heap buffer (proven ≥
//!   the bound; SPEC §16.6 dependent bounds). The analysis returns the bound
//!   EXPRESSION per base so the backend can size the malloc.
//! - A spawn whose multiplicity is genuinely unbounded (a `[true]` node, a
//!   non-countdown loop) is a COMPILE ERROR.
//!
//! 2026-08-08 (pool lifecycle): `free`/`keep` do NOT shrink the pool (the
//! allocator is monotonic; no reclamation until the free-list phase), and the
//! per-base capacity is a SUM across nodes (one shared counter), not a max.

use crate::ast::{Expr, Statement, TopLevel};
use std::collections::HashMap;

/// A reactive node's firing multiplicity.
#[derive(Clone, PartialEq)]
enum Firing {
    /// `[count < N]` with a compile-time constant N — the columns are sized
    /// statically to the proven maximum.
    Static(i64),
    /// `[count < N]` with a runtime N (a state field or a named const) — the
    /// pool is sized at runtime from the bound (dependent capacity). Carries
    /// the bound EXPRESSION so the backend can size the heap buffer.
    Dependent(Expr),
    /// Not a countdown (a `[true]` precondition, a non-bounded guard) — an
    /// unprovable spawn here is an error.
    Unprovable,
}

/// One runtime-bound spawn context for a base: a compile-time multiplier
/// (the enclosing const foreach products) times the runtime bound expression
/// (a countdown bound or a runtime-range foreach bound).
#[derive(Clone, Debug)]
pub struct DependentTerm {
    pub multiplier: i64,
    pub bound: Expr,
}

/// The result: `base` → the proven TOTAL lifetime spawn count for STATIC
/// pools (the monotonic `__spawn_next_<base>` counter is shared by every node
/// that spawns the base, so the capacity is the SUM of all firing contexts —
/// the counter never exceeds it; row 0 is the static instance); `base` → the
/// runtime-bound spawn terms for DEPENDENT pools (the backend sizes the heap
/// buffer to the sum of the terms + 1); and the unprovable-spawn errors.
///
/// 2026-08-08 (pool lifecycle, Bug 1+2): capacity is a TOTAL across nodes,
/// not a max. `__spawn_next_<base>` only ever increments (no row reclamation
/// until the free-list phase), so two nodes each spawning the same base write
/// rows 1..(a+b) on ONE counter — `max(a,b)` columns would overflow. And
/// `free`/`keep` do NOT shrink the pool: without reclamation a free never
/// returns a row, so decrementing here would under-allocate.
pub fn analyze(items: &[TopLevel]) -> (
    HashMap<String, usize>,
    HashMap<String, Vec<DependentTerm>>,
    Vec<String>,
    HashMap<String, crate::ast::SpawnStorage>,
) {
    let mut capacities: HashMap<String, usize> = HashMap::new();
    let mut dependent: HashMap<String, Vec<DependentTerm>> = HashMap::new();
    let mut errors: Vec<String> = Vec::new();
    // 2026-08-09 (init kind, Phase 4): an init with a declared bound set sizes
    // its pool to the max of the set — the value is provably ≤ that max, so
    // the pool is statically sized (provably inexhaustible) instead of going
    // through the dependent-heap runtime-malloc path.
    let init_maxes: HashMap<String, i64> = items
        .iter()
        .filter_map(|item| match item {
            TopLevel::Init(i) => i
                .bound
                .as_ref()
                .and_then(bound_set_max)
                .map(|m| (i.name.clone(), m)),
            _ => None,
        })
        .collect();
    for item in items {
        match item {
            TopLevel::Transaction(t) => {
                let firing = node_firing(t, &init_maxes);
                let mut live: HashMap<String, i64> = HashMap::new();
                let mut terms: HashMap<String, Vec<DependentTerm>> = HashMap::new();
                let mut ctx = WalkCtx { firing: &firing, bound_terms: &[] };
                walk_stmts(&t.body, 1, &mut ctx, &mut live, &mut terms, &mut errors);
                merge_total(&mut capacities, &live);
                for (base, ts) in terms {
                    dependent.entry(base).or_default().extend(ts);
                }
            }
            TopLevel::Definition(d) => {
                let mut live: HashMap<String, i64> = HashMap::new();
                let mut terms: HashMap<String, Vec<DependentTerm>> = HashMap::new();
                let mut ctx = WalkCtx { firing: &Firing::Static(1), bound_terms: &[] };
                walk_stmts(&d.body, 1, &mut ctx, &mut live, &mut terms, &mut errors);
                merge_total(&mut capacities, &live);
                for (base, ts) in terms {
                    dependent.entry(base).or_default().extend(ts);
                }
            }
            TopLevel::Statement(stmt) => {
                let mut live: HashMap<String, i64> = HashMap::new();
                let mut terms: HashMap<String, Vec<DependentTerm>> = HashMap::new();
                let mut ctx = WalkCtx { firing: &Firing::Static(1), bound_terms: &[] };
                walk_stmt(stmt, 1, &mut ctx, &mut live, &mut terms, &mut errors);
                merge_total(&mut capacities, &live);
                for (base, ts) in terms {
                    dependent.entry(base).or_default().extend(ts);
                }
            }
            _ => {}
        }
    }
    // 2026-08-09 (Phase 5): collect the storage classes of non-pooled spawns —
    // `box` (per-instance-heap) and `spill` (growable). The backend skips the
    // static `[capacity x T]` column for these bases.
    let storage_classes = collect_storage_classes(items);
    (capacities, dependent, errors, storage_classes)
}

/// Scan every expression for `box`/`spill` spawns and record the base →
/// storage-class mapping. A base with any non-pooled spawn is never given a
/// static pool column.
fn collect_storage_classes(items: &[TopLevel]) -> HashMap<String, crate::ast::SpawnStorage> {
    let mut out = HashMap::new();
    for stmt in items.iter().flat_map(top_level_bodies) {
        walk_stmt_for_storage(stmt, &mut out);
    }
    out
}

/// The statement list a top-level item carries (transaction body, definition
/// body, or a single top-level statement).
fn top_level_bodies(item: &TopLevel) -> Vec<&crate::ast::Statement> {
    match item {
        TopLevel::Transaction(t) => t.body.iter().collect(),
        TopLevel::Definition(d) => d.body.iter().collect(),
        TopLevel::Statement(stmt) => vec![stmt],
        _ => vec![],
    }
}

/// Record any non-pooled `box`/`spill` spawn in a statement (recursively).
fn walk_stmt_for_storage(
    s: &crate::ast::Statement,
    out: &mut HashMap<String, crate::ast::SpawnStorage>,
) {
    match s {
        crate::ast::Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                walk_expr_for_storage(e, out);
            }
        }
        crate::ast::Statement::Assign(l, r) => {
            walk_expr_for_storage(l, out);
            walk_expr_for_storage(r, out);
        }
        crate::ast::Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                walk_expr_for_storage(t, out);
            }
            walk_expr_for_storage(value, out);
        }
        crate::ast::Statement::Term(Some(e)) | crate::ast::Statement::EndProgram(Some(e)) => {
            walk_expr_for_storage(e, out);
        }
        crate::ast::Statement::Expression(e) | crate::ast::Statement::Gate(e) => {
            walk_expr_for_storage(e, out);
        }
        crate::ast::Statement::Guarded(_, body) => {
            for s in body {
                walk_stmt_for_storage(s, out);
            }
        }
        crate::ast::Statement::Block(b) | crate::ast::Statement::SyncBlock(b) => {
            for s in b {
                walk_stmt_for_storage(s, out);
            }
        }
        _ => {}
    }
}

/// Record any non-pooled `box`/`spill` spawn in an expression (recursively).
fn walk_expr_for_storage(e: &Expr, out: &mut HashMap<String, crate::ast::SpawnStorage>) {
    match e {
        Expr::Spawn { type_name, storage, .. } if *storage != crate::ast::SpawnStorage::Pooled => {
            out.entry(type_name.clone()).or_insert(*storage);
        }
        Expr::Spawn { args, .. }
        | Expr::Call(_, args, _)
        | Expr::List(args)
        | Expr::Tuple(args) => {
            for a in args {
                walk_expr_for_storage(a, out);
            }
        }
        Expr::BinaryOp(_, l, r) => {
            walk_expr_for_storage(l, out);
            walk_expr_for_storage(r, out);
        }
        Expr::UnaryOp(_, i) | Expr::Deref(i) | Expr::AddrOf(i) | Expr::Cast(i, _)
        | Expr::Consume(i) | Expr::IsType(i, _) => walk_expr_for_storage(i, out),
        Expr::Field(o, _) | Expr::Index(o, _) | Expr::Reflect(o, _, _) => {
            walk_expr_for_storage(o, out)
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            walk_expr_for_storage(recv, out);
            for a in args {
                walk_expr_for_storage(a, out);
            }
        }
        Expr::If(c, t, e) => {
            walk_expr_for_storage(c, out);
            walk_expr_for_storage(t, out);
            if let Some(e) = e {
                walk_expr_for_storage(e, out);
            }
        }
        Expr::Match(s, arms) => {
            walk_expr_for_storage(s, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr_for_storage(g, out);
                }
                walk_expr_for_storage(&arm.body, out);
            }
        }
        Expr::Slice { array, start, end, stride, .. } => {
            walk_expr_for_storage(array, out);
            for b in [start, end, stride].into_iter().flatten() {
                walk_expr_for_storage(b, out);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, f) in fields {
                walk_expr_for_storage(f, out);
            }
        }
        Expr::Range { start, end, .. } => {
            walk_expr_for_storage(start, out);
            walk_expr_for_storage(end, out);
        }
        Expr::Block(stmts) => {
            for s in stmts {
                walk_stmt_for_storage(s, out);
            }
        }
        _ => {}
    }
}

/// The walk context: the node's firing multiplicity, plus the runtime bound
/// expressions of enclosing runtime-bound foreachs (each spawn's capacity is
/// the firing count times the product of those bounds).
struct WalkCtx<'a> {
    firing: &'a Firing,
    bound_terms: &'a [Expr],
}

/// Classify a reactive node's firing: a countdown `[count < N][count == N]`
/// with a compile-time N is Static; with a runtime N (a field or a named
/// const) it is Dependent (the capacity is N at runtime); a bounded `init`
/// (declared value set) sizes the pool to the max of the set (Static, provably
/// inexhaustible); anything else is Unprovable.
///
/// 2026-08-09 (init kind, Phase 4): `init_maxes` maps init names to the max of
/// their declared bound set. A countdown bound that names a bounded init folds
/// to that static max — the pool is provably sized, no runtime malloc.
fn node_firing(t: &crate::ast::Transaction, init_maxes: &HashMap<String, i64>) -> Firing {
    if !t.is_reactive {
        return Firing::Static(1);
    }
    match &t.contract.pre_condition {
        Expr::Bool(true) => Firing::Unprovable,
        Expr::BinaryOp(crate::ast::BinaryOpKind::Lt, _, r) | Expr::BinaryOp(crate::ast::BinaryOpKind::Le, _, r) => {
            match r.as_ref() {
                Expr::Decimal(n) if *n >= 0 => {
                    let inclusive = matches!(t.contract.pre_condition, Expr::BinaryOp(crate::ast::BinaryOpKind::Le, _, _));
                    Firing::Static(if inclusive { n + 1 } else { *n })
                }
                Expr::Identifier(name) => match init_maxes.get(name) {
                    // A bounded init: the seeded value is one of the set, so
                    // the max is a provable static capacity.
                    Some(&max) => Firing::Static(max),
                    None => Firing::Dependent((**r).clone()),
                },
                _ => Firing::Unprovable,
            }
        }
        _ => Firing::Unprovable,
    }
}

/// The maximum of an init bound set, when it is statically resolvable:
/// `[16 | 32 | 64]` → 64, `[64 | lo..hi]` → hi (when hi is a literal). A set
/// containing a name reference (another runtime value) is not statically
/// bounded → None (the pool falls back to the dependent path).
/// 2026-08-09 (init kind, Phase 4).
fn bound_set_max(bound: &crate::ast::top::BoundSpec) -> Option<i64> {
    match bound {
        crate::ast::top::BoundSpec::Single(term) => bound_term_value(term),
        crate::ast::top::BoundSpec::Range(_, hi) => bound_term_value(hi),
        crate::ast::top::BoundSpec::Choice(parts) => parts
            .iter()
            .map(bound_set_max)
            .try_fold(i64::MIN, |acc, v| v.map(|v| acc.max(v))),
    }
}

fn bound_term_value(term: &crate::ast::top::BoundTerm) -> Option<i64> {
    match term {
        crate::ast::top::BoundTerm::Lit(n) => Some(*n),
        crate::ast::top::BoundTerm::Ref(_) => None,
    }
}

/// The runtime bound of a foreach list expression, when it is a runtime
/// value (a `0..M` range with a runtime end). A const range and a list
/// literal are statically known; anything else cannot bound a spawn.
fn foreach_bound(list: &Expr) -> Option<Expr> {
    match list {
        Expr::Range { start, end, .. } => match (start.as_ref(), end.as_ref()) {
            (Expr::Decimal(_), Expr::Decimal(_)) => None,
            (_, Expr::Identifier(_)) | (_, _) if matches!(end.as_ref(), Expr::Identifier(_) | Expr::Field(_, _)) => {
                Some((**end).clone())
            }
            _ => None,
        },
        _ => None,
    }
}

fn walk_stmts(
    stmts: &[Statement],
    multiplier: i64,
    ctx: &mut WalkCtx,
    live: &mut HashMap<String, i64>,
    terms: &mut HashMap<String, Vec<DependentTerm>>,
    errors: &mut Vec<String>,
) {
    for s in stmts {
        walk_stmt(s, multiplier, ctx, live, terms, errors);
    }
}

fn walk_stmt(
    stmt: &Statement,
    multiplier: i64,
    ctx: &mut WalkCtx,
    live: &mut HashMap<String, i64>,
    terms: &mut HashMap<String, Vec<DependentTerm>>,
    errors: &mut Vec<String>,
) {
    match stmt {
        Statement::Foreach { list, body, .. } => {
            let count = match list.as_ref() {
                Expr::Range { start, end, inclusive } => match (start.as_ref(), end.as_ref()) {
                    (Expr::Decimal(s), Expr::Decimal(e)) if *s <= *e => {
                        Some((if *inclusive { e - s + 1 } else { e - s }).max(0))
                    }
                    _ => None,
                },
                Expr::List(elems) => Some(elems.len() as i64),
                _ => None,
            };
            match count {
                Some(c) => walk_stmts(body, multiplier * c, ctx, live, terms, errors),
                None => {
                    // A runtime-bound loop — a spawn inside it yields a
                    // DEPENDENT pool (the capacity is the loop's runtime
                    // bound, §16.6), not an error. Push the bound expression
                    // so inner spawns accumulate it.
                    if let Some(bound) = foreach_bound(list.as_ref()) {
                        let nested = ctx.bound_terms;
                        let extended: Vec<Expr> = nested.iter().cloned().chain(std::iter::once(bound)).collect();
                        let mut inner_ctx = WalkCtx { firing: ctx.firing, bound_terms: &extended };
                        walk_stmts(body, multiplier, &mut inner_ctx, live, terms, errors);
                    } else {
                        // A foreach we cannot bound (a value-carrying list).
                        let mut inner_live: HashMap<String, i64> = HashMap::new();
                        let mut inner_terms: HashMap<String, Vec<DependentTerm>> = HashMap::new();
                        let mut inner_errors = Vec::new();
                        let nested = ctx.bound_terms;
                        let extended: Vec<Expr> = nested.to_vec();
                        let mut inner_ctx = WalkCtx { firing: ctx.firing, bound_terms: &extended };
                        walk_stmts(body, 0, &mut inner_ctx, &mut inner_live, &mut inner_terms, &mut inner_errors);
                        // An unprovable firing context (not a countdown) inside
                        // such a loop is still an error.
                        for e in inner_errors {
                            errors.push(e);
                        }
                        for (base, ts) in inner_terms {
                            terms.entry(base).or_default().extend(ts);
                        }
                    }
                }
            }
        }
        Statement::Guarded(_, body) => walk_stmts(body, multiplier, ctx, live, terms, errors),
        Statement::Block(body) | Statement::SyncBlock(body) => {
            walk_stmts(body, multiplier, ctx, live, terms, errors);
        }
        Statement::Assign(_, rhs) => walk_expr(rhs, multiplier, ctx, live, terms, errors),
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => {
            walk_expr(e, multiplier, ctx, live, terms, errors);
        }
        Statement::Expression(e) | Statement::Gate(e) => walk_expr(e, multiplier, ctx, live, terms, errors),
        // 2026-08-08 (pool lifecycle, Bug 1): `free h;` / `keep h;` do NOT
        // shrink the pool. The `__spawn_next_<base>` allocator is monotonic —
        // without row reclamation (the free-list phase) a free never returns a
        // row to the pool, so a capacity decrement here would UNDER-allocate
        // and corrupt rows 1..total. `free`/`keep` are consumption/ownership
        // directives: the typechecker marks the handle dead; the pool keeps
        // its full lifetime capacity.
        Statement::FreeHint(_) | Statement::KeepHint(_) => {}
        Statement::Let { expr: Some(e), .. } => walk_expr(e, multiplier, ctx, live, terms, errors),
        _ => {}
    }
}

fn walk_expr(
    expr: &Expr,
    multiplier: i64,
    ctx: &mut WalkCtx,
    live: &mut HashMap<String, i64>,
    terms: &mut HashMap<String, Vec<DependentTerm>>,
    errors: &mut Vec<String>,
) {
    match expr {
        Expr::Spawn { type_name, args, storage } => {
            // 2026-08-09 (Phase 5): `box`/`spill` spawns are NOT pool rows —
            // box is per-instance-heap (each instance its own allocation),
            // spill is a growable buffer. Neither consumes static/dependent
            // pool capacity, and neither triggers the unprovable-spawn error
            // (the user opted out of the pooled column explicitly).
            if *storage != crate::ast::SpawnStorage::Pooled {
                for a in args {
                    walk_expr(a, multiplier, ctx, live, terms, errors);
                }
                return;
            }
            match ctx.firing {
                Firing::Static(n) if ctx.bound_terms.is_empty() => {
                    let entry = live.entry(type_name.clone()).or_insert(0);
                    *entry += multiplier * (*n).max(1);
                }
                Firing::Static(n) => {
                    // Enclosing runtime-bound foreachs: the capacity is the
                    // const firing count times the runtime loop bounds.
                    let bound = product(ctx.bound_terms);
                    let entry = terms.entry(type_name.clone()).or_default();
                    entry.push(DependentTerm { multiplier: multiplier * (*n).max(1), bound });
                }
                                Firing::Dependent(countdown_bound) => {
                    // The countdown bound N is the runtime firing count; each
                    // enclosing runtime-bound foreach multiplies it.
                    let mut bounds: Vec<Expr> = Vec::with_capacity(ctx.bound_terms.len() + 1);
                    bounds.push(countdown_bound.clone());
                    bounds.extend(ctx.bound_terms.iter().cloned());
                    let bound = product(&bounds);
                    let entry = terms.entry(type_name.clone()).or_default();
                    entry.push(DependentTerm { multiplier, bound });
                }
                Firing::Unprovable => {
                    errors.push(format!(
                        "spawn of '{}' is not statically bounded — the pool must be predictably \
                         inexhaustible; spawn inside a countdown node with a compile-time constant \
                         or runtime-field bound",
                        type_name
                    ));
                }
            }
            for a in args {
                walk_expr(a, multiplier, ctx, live, terms, errors);
            }
        }
        Expr::BinaryOp(_, l, r) => {
            walk_expr(l, multiplier, ctx, live, terms, errors);
            walk_expr(r, multiplier, ctx, live, terms, errors);
        }
        Expr::Call(_, args, _) | Expr::List(args) | Expr::Tuple(args) => {
            for a in args {
                walk_expr(a, multiplier, ctx, live, terms, errors);
            }
        }
        Expr::Cast(i, _) | Expr::IsType(i, _) | Expr::Consume(i) | Expr::Deref(i) | Expr::AddrOf(i) => {
            walk_expr(i, multiplier, ctx, live, terms, errors);
        }
        Expr::Field(o, _) | Expr::Index(o, _) | Expr::Reflect(o, _, _) => {
            walk_expr(o, multiplier, ctx, live, terms, errors);
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            walk_expr(recv, multiplier, ctx, live, terms, errors);
            for a in args {
                walk_expr(a, multiplier, ctx, live, terms, errors);
            }
        }
        Expr::If(c, t, e) => {
            walk_expr(c, multiplier, ctx, live, terms, errors);
            walk_expr(t, multiplier, ctx, live, terms, errors);
            if let Some(e) = e {
                walk_expr(e, multiplier, ctx, live, terms, errors);
            }
        }
        Expr::Match(s, arms) => {
            walk_expr(s, multiplier, ctx, live, terms, errors);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr(g, multiplier, ctx, live, terms, errors);
                }
                walk_expr(&arm.body, multiplier, ctx, live, terms, errors);
            }
        }
        Expr::Slice { array, start, end, stride, .. } => {
            walk_expr(array, multiplier, ctx, live, terms, errors);
            for b in [start, end, stride].into_iter().flatten() {
                walk_expr(b, multiplier, ctx, live, terms, errors);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, f) in fields {
                walk_expr(f, multiplier, ctx, live, terms, errors);
            }
        }
        Expr::Range { start, end, .. } => {
            walk_expr(start, multiplier, ctx, live, terms, errors);
            walk_expr(end, multiplier, ctx, live, terms, errors);
        }
        _ => {}
    }
}

/// Product of the runtime foreach bounds — `a * b * c` as a nested expr.
fn product(bounds: &[Expr]) -> Expr {
    if bounds.is_empty() {
        return Expr::Decimal(1);
    }
    let mut acc = bounds[0].clone();
    for b in &bounds[1..] {
        acc = Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, Box::new(acc), Box::new(b.clone()));
    }
    acc
}

/// Sum a node's live counts into the per-base TOTAL. 2026-08-08 (Bug 2): the
/// monotonic `__spawn_next_<base>` counter is shared across every node that
/// spawns the base — its max value is the SUM of all firing contexts, so the
/// pool must be sized to the total, never the max (max would overflow rows
/// 1..(a+b) on one counter).
fn merge_total(out: &mut HashMap<String, usize>, live: &HashMap<String, i64>) {
    for (base, n) in live {
        let entry = out.entry(base.clone()).or_insert(0);
        *entry = *entry + (*n as usize).max(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Contract, Expr, Statement, TopLevel, Transaction};

    fn txn(name: &str, pre: Expr, post: Expr, body: Vec<Statement>) -> TopLevel {
        TopLevel::Transaction(Transaction {
            name: name.to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: pre,
                post_condition: post,
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body,
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        })
    }

    fn spawn(base: &str) -> Statement {
        Statement::Let {
            name: "h".to_string(),
            names: vec![],
            ty: None,
            expr: Some(Expr::Spawn { type_name: base.to_string(), args: vec![], storage: crate::ast::SpawnStorage::Pooled }),
            modifiers: vec![],
        }
    }

    fn countdown(bound: Expr) -> TopLevel {
        txn(
            "work",
            Expr::BinaryOp(crate::ast::BinaryOpKind::Lt,
                Box::new(Expr::Identifier("ticks".into())),
                Box::new(bound)),
            Expr::BinaryOp(crate::ast::BinaryOpKind::Eq,
                Box::new(Expr::Identifier("ticks".into())),
                Box::new(Expr::Decimal(0))),
            vec![spawn("Counter")],
        )
    }

    #[test]
    fn const_countdown_spawn_is_bounded() {
        let program = vec![countdown(Expr::Decimal(2))];
        let (caps, dependent, errors, _) = analyze(&program);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
        assert!(dependent.is_empty(), "a const countdown is not dependent");
        assert_eq!(caps.get("Counter"), Some(&2));
    }

    #[test]
    fn runtime_countdown_spawn_is_dependent_with_bound() {
        // `[ticks < N]` with a runtime N — the pool is dependent (sized from
        // N at runtime, SPEC §16.6), NOT an error, and the bound expression
        // is threaded for the backend malloc.
        let program = vec![countdown(Expr::Identifier("N".into()))];
        let (caps, dependent, errors, _) = analyze(&program);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
        assert!(!caps.contains_key("Counter"));
        let terms = dependent.get("Counter").expect("a runtime-bound spawn must be dependent");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].multiplier, 1);
        assert!(matches!(terms[0].bound, Expr::Identifier(ref n) if n == "N"));
    }

    #[test]
    fn runtime_foreach_multiplies_dependent_bound() {
        // A runtime-bound foreach `foreach i in 0..M` inside a runtime-bound
        // countdown multiplies the countdown bound by the loop bound.
        let foreach = Statement::Foreach {
            item: "i".to_string(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Identifier("M".into())),
                inclusive: false,
            }),
            body: vec![spawn("Counter")],
        };
        let program = vec![countdown_with_body(Expr::Identifier("N".into()), vec![foreach])];
        let (_, dependent, errors, _) = analyze(&program);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
        let terms = dependent.get("Counter").expect("dependent pool expected");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].multiplier, 1);
        assert!(matches!(&terms[0].bound,
            Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, _, _)));
    }

    fn countdown_with_body(bound: Expr, body: Vec<Statement>) -> TopLevel {
        txn(
            "work",
            Expr::BinaryOp(crate::ast::BinaryOpKind::Lt,
                Box::new(Expr::Identifier("ticks".into())),
                Box::new(bound)),
            Expr::BinaryOp(crate::ast::BinaryOpKind::Eq,
                Box::new(Expr::Identifier("ticks".into())),
                Box::new(Expr::Decimal(0))),
            body,
        )
    }

    #[test]
    fn unprovable_spawn_is_rejected() {
        let program = vec![txn(
            "work",
            Expr::Bool(true),
            Expr::Bool(false),
            vec![spawn("Counter")],
        )];
        let (_, _, errors, _) = analyze(&program);
        assert!(!errors.is_empty(), "an unbounded spawn must be rejected");
        assert!(errors[0].contains("not statically bounded"));
    }

    // 2026-08-08 (pool lifecycle, Bug 1): `free h;` must NOT shrink the pool —
    // the __spawn_next_<base> allocator is monotonic (no reclamation yet), so
    // the capacity is the TOTAL spawn count, and free/keep are ownership
    // directives that leave the pool size unchanged.
    #[test]
    fn free_does_not_shrink_pool() {
        let mut body = vec![spawn("Counter")];
        body.push(Statement::FreeHint("h".to_string()));
        let program = vec![countdown_with_body(Expr::Decimal(4), body)];
        let (caps, dependent, errors, _) = analyze(&program);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
        assert!(dependent.is_empty(), "a const countdown is not dependent");
        assert_eq!(caps.get("Counter"), Some(&4),
            "free must not reduce the pool: capacity = total spawn count (4 firings), not max concurrent (1)");
    }

    #[test]
    fn keep_does_not_shrink_pool() {
        let mut body = vec![spawn("Counter")];
        body.push(Statement::KeepHint("h".to_string()));
        let program = vec![countdown_with_body(Expr::Decimal(3), body)];
        let (caps, _, errors, _) = analyze(&program);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
        assert_eq!(caps.get("Counter"), Some(&3),
            "keep must not reduce the pool: capacity = total spawn count (3 firings)");
    }

    // 2026-08-08 (pool lifecycle, Bug 2): capacity is a SUM across nodes —
    // __spawn_next_<base> is ONE monotonic counter shared by every node that
    // spawns the base. Two nodes spawning the same base need rows 1..(a+b);
    // a max (max(a,b)) column would overflow the shared counter.
    #[test]
    fn cross_node_spawns_sum_capacity() {
        let node_a = countdown_with_body(Expr::Decimal(3), vec![spawn("Counter")]);
        let node_b = countdown_with_body(Expr::Decimal(5), vec![spawn("Counter")]);
        let program = vec![node_a, node_b];
        let (caps, _, errors, _) = analyze(&program);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
        assert_eq!(caps.get("Counter"), Some(&8),
            "two nodes spawning Counter need the SUM (3 + 5 = 8) on the shared counter");
    }

    // ── 2026-08-09 (init kind, Phase 4): bounded-init pool capacity ────

    fn bounded_init(name: &str, set: crate::ast::top::BoundSpec) -> TopLevel {
        TopLevel::Init(crate::ast::top::InitDecl {
            name: name.to_string(),
            bound: Some(set),
            ty: crate::ast::Type::int(),
            value: Some(Expr::Decimal(0)),
            body: vec![],
            span: None,
            doc: None,
        })
    }

    /// A countdown whose bound is a bounded init `[16 | 32 | 64]` — the pool
    /// is sized to the max of the set (64), provably inexhaustible, instead of
    /// the dependent-heap runtime-malloc path.
    #[test]
    fn bounded_init_countdown_sizes_pool_to_set_max() {
        let init = bounded_init(
            "N",
            crate::ast::top::BoundSpec::Choice(vec![
                crate::ast::top::BoundSpec::Single(crate::ast::top::BoundTerm::Lit(16)),
                crate::ast::top::BoundSpec::Single(crate::ast::top::BoundTerm::Lit(32)),
                crate::ast::top::BoundSpec::Single(crate::ast::top::BoundTerm::Lit(64)),
            ]),
        );
        let program = vec![init, countdown(Expr::Identifier("N".into()))];
        let (caps, dependent, errors, _) = analyze(&program);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
        assert!(dependent.is_empty(),
            "a bounded init must not use the dependent-heap path, got {dependent:?}");
        assert_eq!(caps.get("Counter"), Some(&64),
            "pool must be sized to the max of the bound set (64)");
    }

    /// A bounded init with a literal range `[64 | lo..hi]` — the max is the
    /// range's hi literal.
    #[test]
    fn bounded_init_range_uses_hi_as_max() {
        let init = bounded_init(
            "N",
            crate::ast::top::BoundSpec::Choice(vec![
                crate::ast::top::BoundSpec::Single(crate::ast::top::BoundTerm::Lit(64)),
                crate::ast::top::BoundSpec::Range(
                    crate::ast::top::BoundTerm::Lit(10),
                    crate::ast::top::BoundTerm::Lit(54),
                ),
            ]),
        );
        let program = vec![init, countdown(Expr::Identifier("N".into()))];
        let (caps, dependent, errors, _) = analyze(&program);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
        assert!(dependent.is_empty());
        assert_eq!(caps.get("Counter"), Some(&64),
            "range max (hi=54) and the single 64 both contribute — max is 64");
    }

    /// An init bound set containing a NAME reference is not statically
    /// bounded — the pool falls back to the dependent path (runtime-sized).
    #[test]
    fn unbounded_init_set_with_ref_stays_dependent() {
        let init = bounded_init(
            "N",
            crate::ast::top::BoundSpec::Choice(vec![
                crate::ast::top::BoundSpec::Single(crate::ast::top::BoundTerm::Lit(16)),
                crate::ast::top::BoundSpec::Range(
                    crate::ast::top::BoundTerm::Lit(10),
                    crate::ast::top::BoundTerm::Ref("M".into()),
                ),
            ]),
        );
        let program = vec![init, countdown(Expr::Identifier("N".into()))];
        let (caps, dependent, errors, _) = analyze(&program);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
        assert!(!caps.contains_key("Counter"),
            "a ref-containing bound set is not statically bounded");
        let terms = dependent.get("Counter").expect("must use the dependent path");
        assert!(matches!(&terms[0].bound, Expr::Identifier(n) if n == "N"));
    }

    // ── 2026-08-09 (Phase 5): box/spill storage classification ────────

    fn spawn_with_storage(storage: crate::ast::SpawnStorage) -> Statement {
        Statement::Let {
            name: "h".to_string(),
            names: vec![],
            ty: None,
            expr: Some(Expr::Spawn { type_name: "Counter".to_string(), args: vec![], storage }),
            modifiers: vec![],
        }
    }

    /// A `box` spawn is NOT pooled — no capacity, no dependent term, no
    /// unprovable error (the user opted out of the pool column explicitly),
    /// and the analysis records the Box storage class.
    #[test]
    fn box_spawn_is_non_pooled_and_classified() {
        let program = vec![txn(
            "work",
            Expr::Bool(true),
            Expr::Bool(false),
            vec![spawn_with_storage(crate::ast::SpawnStorage::Box)],
        )];
        let (caps, dependent, errors, storage) = analyze(&program);
        // The firing is unprovable ([true]) but box legalizes it — no error.
        assert!(errors.is_empty(), "box spawn must not error, got {errors:?}");
        assert!(caps.is_empty(), "a box spawn consumes no pool capacity");
        assert!(dependent.is_empty(), "a box spawn is not a dependent pool");
        assert_eq!(storage.get("Counter"), Some(&crate::ast::SpawnStorage::Box));
    }

    /// A `spill` spawn is also non-pooled and classified Spill.
    #[test]
    fn spill_spawn_is_non_pooled_and_classified() {
        let program = vec![txn(
            "work",
            Expr::Bool(true),
            Expr::Bool(false),
            vec![spawn_with_storage(crate::ast::SpawnStorage::Spill)],
        )];
        let (caps, dependent, errors, storage) = analyze(&program);
        assert!(errors.is_empty(), "spill spawn must not error, got {errors:?}");
        assert!(caps.is_empty(), "a spill spawn consumes no pool capacity");
        assert_eq!(storage.get("Counter"), Some(&crate::ast::SpawnStorage::Spill));
    }

    /// A plain (pooled) spawn in an unprovable context is STILL an error —
    /// box/spill are the explicit escape, not a silent relaxation.
    #[test]
    fn pooled_spawn_in_unprovable_context_is_still_rejected() {
        let program = vec![txn(
            "work",
            Expr::Bool(true),
            Expr::Bool(false),
            vec![spawn_with_storage(crate::ast::SpawnStorage::Pooled)],
        )];
        let (_, _, errors, storage) = analyze(&program);
        assert!(!errors.is_empty(), "a pooled spawn in an unprovable node must error");
        assert!(errors[0].contains("not statically bounded"));
        assert!(storage.is_empty(), "a rejected pooled spawn has no storage class");
    }
}

