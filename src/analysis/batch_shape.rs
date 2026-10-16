// ── Batch Shape Detection ───────────────────────────────────────────
//
// 2026-07-31: Plan §5 (Fix 2) — detect a reactive txn whose single runtime
// guard is `count % N == 0` (a periodic io boundary) and derive the batch
// structure: an inner PURE-compute loop running to the next boundary plus a
// cold outer guard. This is the principled form of the batch-loop removed in
// Phase 6 (81eea6aa) — the boundary is the io precondition's interval
// (docs/plans/2026-07-30-flat-node-decomposition.md §4.1), NOT an
// extract_batch_size heuristic.
//
// The count=0 peel (§4.4): when the guard precedes the counter increment
// (knucleotide/mandelbrot), the guard fires at count=0 — the batch loop would
// miss it. `peel_count_zero` records this; the emission runs one compute
// iteration at entry and fires the guard body.

use crate::analysis::node_decompose::{split_into_segments, PredicateClass, Segment};
use crate::ast::{BinaryOpKind, Expr, Statement};
use std::collections::HashMap;

/// A batch-loop decomposition of a reactive txn.
#[derive(Debug, Clone)]
pub struct BatchShape {
    /// The loop counter variable name (e.g. "count").
    pub counter: String,
    /// The io boundary interval N from `count % N == 0`.
    pub batch_size: usize,
    /// The full `when count % N == 0 { io }` guard, re-emitted at each boundary.
    pub guard: Statement,
    /// The guard's INNER statements (the io), emitted directly when the
    /// boundary is known to have fired (the countdown's cold block) so no
    /// conditional branch structure is introduced.
    pub guard_body: Vec<Statement>,
    /// All compute statements (pre + post segments, guard removed) — the inner
    /// pure-compute loop body.
    pub inner_body: Vec<Statement>,
}

/// Detect a batch shape for the program's foldable reactive txn.
///
/// 2026-07-31: Runs on the swan-song-STRIPPED bodies (what the backend emits).
/// Returns None when no txn qualifies or multiple qualify (ambiguous — fall
/// back to version-DAG / PerFieldPhi).
///
/// SCOPE: only POST-increment guards are batched (the counter is incremented
/// BEFORE the guard, e.g. kalman/float_math). For these the batch structure is
/// EXACT — the io fires at the boundary after the same number of computes as
/// the composite's boundary iteration. PRE-increment guards (knucleotide/
/// mandelbrot, increment AFTER the guard) are rejected: the batch fires the
/// guard after `batch_size` computes, but the composite's boundary iteration
/// fires it after `batch_size + 1`, an off-by-one the count=0 peel does NOT
/// fix (it only adds the count=0 print). See
/// docs/plans/2026-07-31-regain-kalman-float-math-parity.md §5 (scope note).
pub fn detect_batch_shape(
    swan_songs: &HashMap<String, (Vec<Statement>, Vec<Vec<Statement>>)>,
) -> Option<BatchShape> {
    let mut result: Option<BatchShape> = None;
    for (_name, (stripped, _hoisted)) in swan_songs {
        if let Some(bs) = detect_for_body(stripped) {
            if result.is_some() {
                return None; // ambiguous — more than one batch-qualifying txn
            }
            result = Some(bs);
        }
    }
    result
}

/// Detect a batch shape in a single txn body.
///
/// 2026-07-31: Requires exactly one `Runtime` guard with a `count % N == 0`
/// condition, a counter incremented in the compute body, and POST-increment
/// guard placement (the increment precedes the guard). The guard may sit
/// mid-body; the inner loop runs ALL compute (pre+post) and the guard fires at
/// the boundary.
fn detect_for_body(body: &[Statement]) -> Option<BatchShape> {
    let segments = split_into_segments(body);
    let mut guard: Option<Statement> = None;
    let mut guard_body: Vec<Statement> = Vec::new();
    let mut counter: Option<String> = None;
    let mut batch_size: Option<usize> = None;
    let mut pre: Vec<Statement> = Vec::new();
    let mut post: Vec<Statement> = Vec::new();
    let mut seen_guard = false;

    for seg in &segments {
        match seg {
            Segment::Compute(stmts) => {
                if seen_guard {
                    post.extend(stmts.clone());
                } else {
                    pre.extend(stmts.clone());
                }
            }
            Segment::Guard { condition, body: gbody, classification, .. } => {
                if seen_guard {
                    return None; // multiple guards — not a batch shape
                }
                seen_guard = true;
                if *classification != PredicateClass::Runtime {
                    return None;
                }
                let (c, n) = extract_batch_condition(condition)?;
                counter = Some(c);
                batch_size = Some(n);
                guard = Some(Statement::Guarded(condition.clone(), gbody.clone()));
                guard_body = gbody.clone();
            }
        }
    }
    let guard = guard?;
    let counter = counter?;
    let batch_size = batch_size?;

    let mut inner_body = pre.clone();
    inner_body.extend(post.clone());
    // The counter must advance each iteration — otherwise the batch structure
    // cannot advance toward the next boundary.
    if !inner_body.iter().any(|s| is_counter_increment(s, &counter)) {
        return None;
    }
    // POST-increment only: the counter must be incremented BEFORE the guard.
    // A pre-increment guard (increment after the guard) is off-by-one at every
    // boundary (see the module doc) and is rejected.
    if !pre.iter().any(|s| is_counter_increment(s, &counter)) {
        return None;
    }

    Some(BatchShape {
        counter,
        batch_size,
        guard,
        guard_body,
        inner_body,
    })
}

/// Extract `(counter, N)` from `counter % N == 0` (either operand order).
fn extract_batch_condition(cond: &Expr) -> Option<(String, usize)> {
    match cond {
        Expr::BinaryOp(BinaryOpKind::Eq, l, r) => {
            extract_mod_zero(l, r).or_else(|| extract_mod_zero(r, l))
        }
        _ => None,
    }
}

/// Match `Mod(counter, N) == 0` with `a` as the Mod and `b` as the zero.
fn extract_mod_zero(a: &Expr, b: &Expr) -> Option<(String, usize)> {
    match a {
        Expr::BinaryOp(BinaryOpKind::Mod, l, r) => {
            let counter = match l.as_ref() {
                Expr::Identifier(n) => n.clone(),
                _ => return None,
            };
            let n = match r.as_ref() {
                Expr::Decimal(d) if *d > 1 => *d as usize,
                _ => return None,
            };
            if matches!(b, Expr::Decimal(0)) {
                Some((counter, n))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Is this statement `counter = counter + 1`?
fn is_counter_increment(stmt: &Statement, counter: &str) -> bool {
    match stmt {
        Statement::Assign(lhs, rhs) => {
            if lhs.as_var_name() != Some(counter) {
                return false;
            }
            matches!(rhs, Expr::BinaryOp(BinaryOpKind::Add, l, r)
                if matches!(l.as_ref(), Expr::Identifier(n) if n == counter)
                    && matches!(r.as_ref(), Expr::Decimal(1)))
                || matches!(rhs, Expr::BinaryOp(BinaryOpKind::Add, l, r)
                    if matches!(l.as_ref(), Expr::Decimal(1))
                        && matches!(r.as_ref(), Expr::Identifier(n) if n == counter))
        }
        _ => false,
    }
}

/// Count the arithmetic operations (add/sub/mul/div) in a body.
///
/// 2026-07-31: The batch dispatch's COST MODEL. Only DENSE bodies (≥ 40 ops)
/// batch: for dense matrix-style bodies (kalman ~140 ops) the guard-removal
/// dominates AND LLVM does not reassociate the multiply chains into a different
/// float result; for sparse/reduction bodies (float_math ~30 ops) the batch
/// lets LLVM vectorize/reassociate the accumulation, changing the benchmark
/// output (violates symmetric-output), and for tiny bodies the outer/inner
/// overhead exceeds the guard-removal benefit. See
/// docs/plans/2026-07-31-regain-kalman-float-math-parity.md §5.
pub fn arithmetic_op_count(body: &[Statement]) -> usize {
    body.iter().map(|s| stmt_arith_ops(s)).sum()
}

fn stmt_arith_ops(s: &Statement) -> usize {
    match s {
        Statement::Assign(_, e) | Statement::Let { expr: Some(e), .. } => expr_arith_ops(e),
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Tail(e) | Statement::Expression(e) => {
            expr_arith_ops(e)
        }
        Statement::Guarded(cond, body) => {
            expr_arith_ops(cond) + body.iter().map(stmt_arith_ops).sum::<usize>()
        }
        _ => 0,
    }
}

fn expr_arith_ops(e: &Expr) -> usize {
    match e {
        Expr::BinaryOp(kind, l, r) => {
            let base = match kind {
                BinaryOpKind::Add | BinaryOpKind::Sub
                | BinaryOpKind::Mul | BinaryOpKind::Div | BinaryOpKind::Mod => 1,
                _ => 0,
            };
            base + expr_arith_ops(l) + expr_arith_ops(r)
        }
        Expr::UnaryOp(_, inner) | Expr::Cast(inner, _) | Expr::Deref(inner) => expr_arith_ops(inner),
        Expr::Call(_, args, _) => args.iter().map(expr_arith_ops).sum(),
        Expr::Tuple(ts) | Expr::List(ts) => ts.iter().map(expr_arith_ops).sum(),
        Expr::Index(a, i) => expr_arith_ops(a) + expr_arith_ops(i),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txn(body: Vec<Statement>) -> HashMap<String, (Vec<Statement>, Vec<Vec<Statement>>)> {
        let mut m = HashMap::new();
        m.insert("t".to_string(), (body, Vec::new()));
        m
    }

    fn assign(name: &str, e: Expr) -> Statement {
        Statement::Assign(Expr::Identifier(name.to_string()), e)
    }

    fn id(name: &str) -> Expr {
        Expr::Identifier(name.to_string())
    }

    fn add(l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(BinaryOpKind::Add, Box::new(l), Box::new(r))
    }

    fn mod_(l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(BinaryOpKind::Mod, Box::new(l), Box::new(r))
    }

    fn eq(l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(BinaryOpKind::Eq, Box::new(l), Box::new(r))
    }

    fn guard(cond: Expr, body: Vec<Statement>) -> Statement {
        Statement::Guarded(cond, body)
    }

    fn inc(name: &str) -> Statement {
        assign(name, add(id(name), Expr::Decimal(1)))
    }

    /// kalman-style: guard AFTER the increment (post-increment) — batched.
    #[test]
    fn post_increment_guard_batches() {
        let body = vec![
            assign("x0", add(id("a00"), id("x0"))),
            inc("count"),
            guard(eq(mod_(id("count"), Expr::Decimal(5000000)), Expr::Decimal(0)),
                vec![Statement::Expression(Expr::Call("PrintLn#".into(), vec![id("x0")], None))]),
        ];
        let bs = detect_batch_shape(&txn(body)).unwrap();
        assert_eq!(bs.counter, "count");
        assert_eq!(bs.batch_size, 5000000);
        // inner body = pre + post (increment included), guard removed
        assert_eq!(bs.inner_body.len(), 2);
        assert!(bs.inner_body.iter().any(|s| is_counter_increment(s, "count")));
    }

    /// knucleotide-style: guard BEFORE the increment (pre-increment) — REJECTED
    /// (off-by-one at every boundary, see the module doc).
    #[test]
    fn pre_increment_guard_rejected() {
        let body = vec![
            assign("chksum", add(id("chksum"), id("n"))),
            guard(eq(mod_(id("count"), Expr::Decimal(5000000)), Expr::Decimal(0)),
                vec![Statement::Expression(Expr::Call("PrintLn#".into(), vec![id("nchksum")], None))]),
            inc("count"),
        ];
        assert!(detect_batch_shape(&txn(body)).is_none());
    }

    /// A non-periodic guard (`count == N`) is NOT a batch shape.
    #[test]
    fn non_periodic_guard_rejected() {
        let body = vec![
            inc("count"),
            guard(eq(id("count"), id("total")), vec![]),
        ];
        assert!(detect_batch_shape(&txn(body)).is_none());
    }

    /// Multiple guards are not a batch shape.
    #[test]
    fn multiple_guards_rejected() {
        let body = vec![
            inc("count"),
            guard(eq(mod_(id("count"), Expr::Decimal(5)), Expr::Decimal(0)), vec![]),
            guard(eq(mod_(id("count"), Expr::Decimal(7)), Expr::Decimal(0)), vec![]),
        ];
        assert!(detect_batch_shape(&txn(body)).is_none());
    }

    /// A counter that never increments cannot be batched.
    #[test]
    fn no_increment_rejected() {
        let body = vec![
            guard(eq(mod_(id("count"), Expr::Decimal(5)), Expr::Decimal(0)), vec![]),
        ];
        assert!(detect_batch_shape(&txn(body)).is_none());
    }

    /// Zero on either side of the equality (`0 == count % N`) still matches.
    #[test]
    fn zero_on_left_matches() {
        let body = vec![
            inc("count"),
            guard(eq(Expr::Decimal(0), mod_(id("count"), Expr::Decimal(5000000))), vec![]),
        ];
        let bs = detect_batch_shape(&txn(body)).unwrap();
        assert_eq!(bs.batch_size, 5000000);
    }
}
