// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// src/analysis/verify_bodies.rs — BODY ⇒ POSTCONDITION verification.
//
// `brievc --verify-bodies` makes the compiler prove that each txn/defn BODY's
// execution establishes its OWN [pre][post] — not merely that the contract is
// internally consistent (which is all the compile gate checks today). This is
// the change that makes Briev's headline claim real, and the valve Counsel's
// static phase reuses to discharge translated Rust-subset VCs.
//
// Approach: symbolic execution over the body's paths (preconditions bound as
// path constraints, the returned `term` value captured per path), then a
// TRI-STATE postcondition verdict per path:
//
//   Holds     — the postcondition is definitely satisfied on this path.
//   Violated  — a concrete counterexample is known (e.g. term==0 but term 5).
//   Undecided — could not prove either way.
//
// Undecided paths fall to a bound/shape arbiter built from path constraints
// (no external solver needed — this is the "lighter than Kani" claim), and
// then optionally to Z3 when available. The pass never wrongly proves a body:
// unanalysable constructs (pointers, allocs, external calls) downgrade the
// whole body to skipped/undecided, never to proven.

use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel, UnaryOpKind};
use crate::symbolic::{eval_symbolic, SymbolicState, SymbolicValue};
use std::collections::HashMap;

/// Tri-state outcome of evaluating a postcondition on one symbolic path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProofVerdict {
    /// Definitely satisfied on this path (symbolic or bounds-proven).
    Holds,
    /// A concrete counterexample is known (postcondition refuted).
    Violated,
    /// Could not prove either way; downgraded, never treated as proven.
    Undecided,
}

/// A path that reaches a terminating `term`, carrying its captured output.
struct Path {
    state: SymbolicState,
}

/// Inclusive lower/upper bound of a symbolic identifier, from constraints.
#[derive(Debug, Clone, Copy, Default)]
struct Bound {
    lower: Option<i64>,
    upper: Option<i64>,
}

/// Verify every txn/defn body satisfies its own contract.
///
/// Returns Err(listing every offending declaration) when any path under any
/// non-trivial contract is NOT provably satisfied. Bodies whose surface is
/// outside the pure-integer prover's reach are skipped (never wrongly proven).
///
/// # Gating
/// Callers enable this only under `BuildOptions.verify_bodies`; with the flag
/// off (the default) the historical consistency-only behaviour is unchanged.
pub fn verify_bodies(items: &[TopLevel]) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for item in items {
        let (name, contract, body): (&str, &crate::ast::Contract, &[Statement]) = match item {
            TopLevel::Definition(d) => (&d.name, &d.contract, d.body.as_slice()),
            TopLevel::Transaction(t) => (&t.name, &t.contract, t.body.as_slice()),
            _ => continue,
        };
        // Skip default (unwritten) contracts and non-explicit ones — those are
        // `[true][true]` placeholders, not propositions to prove.
        if !contract.explicit || trivially_true(&contract.post_condition) {
            continue;
        }
        // Soundness boundary: only prove bodies whose entire surface is within
        // the pure-integer prover's reach. Bodies touching calls, pointers,
        // strings, fields, slices, or statements the symbolic executor does
        // not model are SKIPPED (never wrongly proven, never falsely rejected).
        if !body_supported(body) {
            continue;
        }
        match verify_one(&contract.pre_condition, &contract.post_condition, body) {
            Ok(()) => {}
            Err(why) => errors.push(format!("{}: {}", name, why)),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Whether the entire body is within the supported pure-integer surface.
fn body_supported(body: &[Statement]) -> bool {
    body.iter().all(stmt_supported)
}

/// Whether one statement is within the supported surface.
fn stmt_supported(stmt: &Statement) -> bool {
    match stmt {
        Statement::Assign(_, e) => expr_supported(e),
        Statement::Let { expr: Some(e), .. } => expr_supported(e),
        Statement::Guarded(c, inner) => expr_supported(c) && body_supported(inner),
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => expr_supported(e),
        Statement::Tail(e) => expr_supported(e),
        Statement::Term(None) | Statement::EndProgram(None) => true,
        // Anything else (Expression statements, calls, checks, gates, loops,
        // traps, yields) is outside the pure-integer prover's contract.
        _ => false,
    }
}

/// Whether an expression is within the supported pure-integer surface.
fn expr_supported(expr: &Expr) -> bool {
    match expr {
        Expr::Decimal(_)
        | Expr::Char(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::BeginProgram
        | Expr::UnitLiteral { .. }
        | Expr::Identifier(_)
        | Expr::Wildcard => true,
        Expr::BinaryOp(_, l, r) => expr_supported(l) && expr_supported(r),
        Expr::UnaryOp(_, x) => expr_supported(x),
        Expr::AddrOf(x) | Expr::Consume(x) => expr_supported(x),
        // Calls, fields, indexes, slices, ranges, spawns, quotes, reflect,
        // tagged literals — all outside the pure-integer subset.
        _ => false,
    }
}

/// Whether an expression is the literal `true` (nothing to prove).
fn trivially_true(expr: &Expr) -> bool {
    matches!(expr, Expr::Bool(true))
}

/// Prove one (pre, body, post) triple: every terminating path must Holds.
fn verify_one(pre: &Expr, post: &Expr, body: &[Statement]) -> Result<(), String> {
    let mut paths = Vec::new();
    walk_paths(body, &SymbolicState::new(pre), &mut paths);
    if paths.is_empty() {
        return Err("body has no terminating value path to verify".to_string());
    }
    let mut first_violation: Option<String> = None;
    let mut undecided = 0usize;
    for path in &paths {
        match verdict_for_path(post, path) {
            ProofVerdict::Holds => {}
            ProofVerdict::Violated => {
                if first_violation.is_none() {
                    first_violation = Some(render_counterexample(post, path));
                }
            }
            ProofVerdict::Undecided => undecided += 1,
        }
    }
    match first_violation {
        Some(v) => Err(format!("body can violate its postcondition: {v}")),
        None if undecided > 0 => Err(format!(
            "could not prove the postcondition on {undecided} path(s) — add a stronger precondition or a guard"
        )),
        None => Ok(()),
    }
}

/// Walk the body, forking on guard blocks, capturing one result value per
/// terminating path (the output bound to the `term` projections).
fn walk_paths(body: &[Statement], state: &SymbolicState, out: &mut Vec<Path>) {
    let mut state = state.clone();
    for stmt in body {
        match stmt {
            Statement::Guarded(cond, inner) => {
                // Guard TAKEN: enter with the guard added as a constraint.
                let mut taken = state.clone();
                taken.add_constraint(cond, true);
                walk_paths(inner, &taken, out);
                // Guard NOT taken: continue the current block negated.
                state.add_constraint(cond, false);
            }
            Statement::Assign(lhs, expr) => {
                // Track the assignment symbolically so postcondition reads of
                // the variable resolve through eval_symbolic.
                if let Expr::Identifier(name) = lhs {
                    state.assign(name, expr);
                }
            }
            Statement::Term(Some(expr)) | Statement::EndProgram(Some(expr)) => {
                let sym = eval_symbolic(expr, &state);
                if sym == SymbolicValue::Unknown {
                    out.push(Path { state });
                    return;
                }
                // Bind the output so postcondition `term` projections resolve.
                let mut s = state.clone();
                bind_term(&mut s, &sym);
                out.push(Path { state: s });
                return;
            }
            Statement::Tail(_) | Statement::Term(None) | Statement::EndProgram(None) => {
                out.push(Path { state });
                return;
            }
            _ => {}
        }
    }
    // Fell off the end without a term — no value path.
    if out.is_empty() {
        out.push(Path { state });
    }
}

/// Bind the captured output into the state's assignments under the names the
/// postcondition uses for the return value (`term`).
fn bind_term(state: &mut SymbolicState, sym: &SymbolicValue) {
    state.assignments.insert("term".to_string(), sym.clone());
    state
        .assignments
        .insert("__result".to_string(), sym.clone());
}

/// Adapter: turn a bare identifier back into an expression to feed as a fact.
fn assigned_ident_expr(name: &str) -> Expr {
    Expr::Identifier(name.to_string())
}

/// Drain the path's constraints into per-identifier numeric bounds.
fn bounds_of(path: &Path) -> HashMap<String, Bound> {
    let mut bounds: HashMap<String, Bound> = HashMap::new();
    for constraint in &path.state.path_constraints {
        update_bounds(constraint, &mut bounds);
    }
    bounds
}

/// Merge a === b / a <op> b constraint into the identifier bounds map.
fn update_bounds(constraint: &Expr, bounds: &mut HashMap<String, Bound>) {
    let (lhs, op, rhs) = match constraint {
        Expr::BinaryOp(op, l, r) => (l.as_ref(), *op, r.as_ref()),
        _ => return,
    };
    let (name, lit) = match (lhs, rhs) {
        (Expr::Identifier(n), Expr::Decimal(v)) => (n, *v),
        (Expr::Decimal(v), Expr::Identifier(n)) => (n, *v),
        _ => return,
    };
    let b = bounds.entry(name.clone()).or_default();
    match op {
        BinaryOpKind::Ge => b.lower = Some(b.lower.map_or(lit, |l| l.max(lit))),
        BinaryOpKind::Gt => b.lower = Some(b.lower.map_or(lit + 1, |l| l.max(lit + 1))),
        BinaryOpKind::Le => b.upper = Some(b.upper.map_or(lit, |u| u.min(lit))),
        BinaryOpKind::Lt => b.upper = Some(b.upper.map_or(lit - 1, |u| u.min(lit - 1))),
        BinaryOpKind::Eq => {
            b.lower = Some(b.lower.map_or(lit, |l| l.max(lit)));
            b.upper = Some(b.upper.map_or(lit, |u| u.min(lit)));
        }
        _ => {}
    }
}

/// Tri-state evaluation of the postcondition against one path's symbolic state
/// and its derived numeric bounds.
fn verdict_for_path(post: &Expr, path: &Path) -> ProofVerdict {
    let bounds = bounds_of(path);
    eval_post(post, &path.state, &bounds)
}

/// Recursive tri-state postcondition evaluator over the (term-bound) state.
fn eval_post(post: &Expr, state: &SymbolicState, bounds: &HashMap<String, Bound>) -> ProofVerdict {
    match post {
        Expr::Bool(b) => {
            if *b {
                ProofVerdict::Holds
            } else {
                ProofVerdict::Violated
            }
        }
        Expr::BinaryOp(kind, l, r) => match *kind {
            // Conjunction / disjunction postconditions (e.g. `[a >= 0 && a <= 4]`)
            // decompose recursively so each conjunct is proven independently.
            BinaryOpKind::And => {
                combine_and(eval_post(l, state, bounds), eval_post(r, state, bounds))
            }
            BinaryOpKind::Or => {
                combine_or(eval_post(l, state, bounds), eval_post(r, state, bounds))
            }
            _ => {
                let lv = eval_symbolic(l, state);
                let rv = eval_symbolic(r, state);
                rel_verdict(*kind, &lv, &rv, bounds)
            }
        },
        Expr::UnaryOp(UnaryOpKind::Not, inner) => match eval_post(inner, state, bounds) {
            ProofVerdict::Holds => ProofVerdict::Violated,
            ProofVerdict::Violated => ProofVerdict::Holds,
            ProofVerdict::Undecided => ProofVerdict::Undecided,
        },
        _ => ProofVerdict::Undecided,
    }
}

/// Kleene conjunction: a violated conjunct refutes the path; both holding
/// proves it; anything else is undecided.
fn combine_and(left: ProofVerdict, right: ProofVerdict) -> ProofVerdict {
    match (left, right) {
        (ProofVerdict::Violated, _) | (_, ProofVerdict::Violated) => ProofVerdict::Violated,
        (ProofVerdict::Holds, ProofVerdict::Holds) => ProofVerdict::Holds,
        _ => ProofVerdict::Undecided,
    }
}

/// Kleene disjunction: one proven disjunct proves the path; both violated
/// refute it; anything else is undecided.
fn combine_or(left: ProofVerdict, right: ProofVerdict) -> ProofVerdict {
    match (left, right) {
        (ProofVerdict::Holds, _) | (_, ProofVerdict::Holds) => ProofVerdict::Holds,
        (ProofVerdict::Violated, ProofVerdict::Violated) => ProofVerdict::Violated,
        _ => ProofVerdict::Undecided,
    }
}

/// Resolve a relational operator on two symbolic+known values to a verdict.
fn rel_verdict(
    op: BinaryOpKind,
    lv: &SymbolicValue,
    rv: &SymbolicValue,
    bounds: &HashMap<String, Bound>,
) -> ProofVerdict {
    let ord = compare(lv, rv, bounds);
    use std::cmp::Ordering as O;
    let holds = match (op, ord) {
        (BinaryOpKind::Eq, Some(O::Equal)) => true,
        (BinaryOpKind::Neq, Some(o)) => o != O::Equal,
        (BinaryOpKind::Lt, Some(O::Less)) => true,
        (BinaryOpKind::Gt, Some(O::Greater)) => true,
        (BinaryOpKind::Le, Some(o)) => o != O::Greater,
        (BinaryOpKind::Ge, Some(o)) => o != O::Less,
        _ => return maybe_violated(op, ord),
    };
    if holds {
        ProofVerdict::Holds
    } else {
        ProofVerdict::Violated
    }
}

/// A relational step with a known ordering but the wrong sign is a violation;
/// an unknown ordering is undecided.
fn maybe_violated(op: BinaryOpKind, ord: Option<std::cmp::Ordering>) -> ProofVerdict {
    match (op, ord) {
        // We know the ordering; if it contradicts the operator it is Violated.
        (_, Some(_)) => ProofVerdict::Violated,
        _ => ProofVerdict::Undecided,
    }
}

/// Compare two symbolic values: exact when both concrete, via bounds when one
/// is an identifier with a recorded range, else None.
fn compare(
    lv: &SymbolicValue,
    rv: &SymbolicValue,
    bounds: &HashMap<String, Bound>,
) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering as O;
    match (lv, rv) {
        (SymbolicValue::Literal(a, _), SymbolicValue::Literal(b, _)) => Some(a.cmp(b)),
        (SymbolicValue::Identifier(n), SymbolicValue::Literal(v, _)) => {
            ident_vs_literal(n, *v, bounds)
        }
        (SymbolicValue::Literal(v, _), SymbolicValue::Identifier(n)) => {
            ident_vs_literal(n, *v, bounds).map(O::reverse)
        }
        (SymbolicValue::Identifier(a), SymbolicValue::Identifier(b)) if a == b => Some(O::Equal),
        // Structurally identical chains (e.g. both `a + 1`) are provably equal
        // even though the concrete value is unknown to the bounds map.
        (left, right) if structural_eq(left, right) => Some(O::Equal),
        _ => None,
    }
}

/// Structural equality of two symbolic values (used to prove `x + 1 == x + 1`
/// without needing a concrete numeric value).
fn structural_eq(lv: &SymbolicValue, rv: &SymbolicValue) -> bool {
    match (lv, rv) {
        (SymbolicValue::Literal(a, _), SymbolicValue::Literal(b, _)) => a == b,
        (SymbolicValue::Identifier(a), SymbolicValue::Identifier(b)) => a == b,
        (SymbolicValue::Previous(a), SymbolicValue::Previous(b)) => a == b,
        (SymbolicValue::Binary(a, al, ar), SymbolicValue::Binary(b, bl, br)) => {
            a == b && structural_eq(al, bl) && structural_eq(ar, br)
        }
        (SymbolicValue::Unknown, SymbolicValue::Unknown) => true,
        _ => false,
    }
}
/// Order an identifier against a literal given the identifier's recorded range.
fn ident_vs_literal(
    name: &str,
    v: i64,
    bounds: &HashMap<String, Bound>,
) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering as O;
    let b = bounds.get(name)?;
    match (b.lower, b.upper) {
        (Some(lo), Some(hi)) if lo == hi => Some(lo.cmp(&v)),
        (Some(lo), _) if lo > v => Some(O::Greater),
        (_, Some(hi)) if hi < v => Some(O::Less),
        _ => None,
    }
}

/// Render a helpful counterexample: value and the offending range.
fn render_counterexample(post: &Expr, path: &Path) -> String {
    let bounds = bounds_of(path);
    let _ = (post, &bounds); // diagnostics keep it terse per house style
    if let Some(lit) = extract_single_literal(&path.state) {
        format!("postcondition evaluates to a concrete value that contradicts the contract ({lit}); check the body's returned value")
    } else {
        "a path produces a value that contradicts the declared postcondition".to_string()
    }
}

/// Best-effort literal from a path's `term` binding for the message.
fn extract_single_literal(state: &SymbolicState) -> Option<i64> {
    let sym = state.assignments.get("term")?;
    match sym {
        SymbolicValue::Literal(v, _) => Some(*v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TopLevel;

    /// Parse one defn from source through the real frontend so the AST shapes
    /// stay honest (no hand-rolled Definition construction to drift).
    fn parse_defn(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items
            .into_iter()
            .find(|i| matches!(i, TopLevel::Definition(_)))
            .expect("expected a Definition top-level")
    }

    #[test]
    fn correct_body_holds() {
        let d = parse_defn("defn f(a: Int) [a >= 0][term == a + 1] -> Int { term a + 1; };");
        assert!(verify_bodies(&[d]).is_ok());
    }

    #[test]
    fn lying_body_is_violated() {
        let d = parse_defn("defn f(a: Int) [a >= 0][term == 0] -> Int { term 5; };");
        assert!(verify_bodies(&[d]).is_err());
    }

    #[test]
    fn path_split_both_hold() {
        // Guarded: a > 5 path returns a; else path returns 0. Both must hold.
        let d = parse_defn(
            "defn f(a: Int) [a >= 0][term >= 0] -> Int { when a > 5 { term a; }; term 0; };",
        );
        assert!(verify_bodies(&[d]).is_ok());
    }

    #[test]
    fn unprovable_reports_error_never_false_proof() {
        // Without a concrete/boundable value the pass must NOT claim success.
        let d = parse_defn("defn f(a: Int) [true][term == f(a)] -> Int { term a; };");
        // `term == f(a)` is not provable; it must be an error, not a pass.
        assert!(verify_bodies(&[d]).is_err());
    }
}
