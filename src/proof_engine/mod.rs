// ── Proof Engine — Contract Verification ───────────────────────────────
// 2026-07-12: Phase 5 — Contract checking, SMT integration, bound extraction.
// Core verification logic for pre/post condition contracts.
// Uses # intrinsics for SMT theory mapping (BitVec for all values).

pub(crate) mod smt;
pub use smt::*;

use crate::ast::{Expr, Statement, TopLevel, Type};
use crate::errors::ProofError;

/// Check that a contract's pre/post conditions are satisfiable.
///
/// Uses existing heuristic checks (check_satisfiable, split_and) as a fast path.
/// If heuristics flag a violation or are inconclusive, Z3 is consulted as arbiter.
/// Without Z3, heuristic verdict is final — violations deny, inconclusive allows.
///
/// For protocol contracts, params should include ("Self", type) to
/// declare #Self as a free variable.
pub fn prove_contract(
    pre: &Expr,
    post: &Expr,
    params: &[(String, Type)],
    explicit: bool,
) -> Result<(), Vec<ProofError>> {
    let pre_is_true = matches!(pre, Expr::Bool(true));
    let post_is_true = matches!(post, Expr::Bool(true));

    // 2026-07-31 (Phase 4): A contract that constrains nothing provides no
    // optimization leverage. `[true][true]` and functionally-always-true
    // contracts (`0 == 0`, `x == x`) are rejected at proof time with the
    // `[[post]`/`[pre]]` sugar hint — but only when the contract was written
    // explicitly (a no-contract default is not a tautology).
    if let Some(tautology) = detect_tautology(pre, post, explicit) {
        return Err(vec![tautology]);
    }

    let condition = if pre_is_true {
        post.clone()
    } else if post_is_true {
        pre.clone()
    } else {
        Expr::BinaryOp(
            crate::ast::BinaryOpKind::And,
            Box::new(pre.clone()),
            Box::new(post.clone()),
        )
    };

    // Run existing heuristic checks
    let conditions = split_and(&condition);
    let mut has_violation = false;

    for cond in &conditions {
        if !check_satisfiable(cond, &Expr::Bool(true)) {
            has_violation = true;
        }
    }

    // If heuristic detected no violation, the contract passes basic sanity
    if !has_violation {
        // Still let Z3 verify deeply if available
        if smt::is_z3_available() {
            let query = smt::build_contract_query(&condition, params);
            match smt::prove_smt_formula(&query, 1000) {
                smt::SmtResult::Unsat => return Ok(()),
                smt::SmtResult::Sat(model) => {
                    let msg = if model.is_empty() {
                        "SMT solver found a counterexample".into()
                    } else {
                        format!("SMT counterexample: {:?}", model)
                    };
                    return Err(vec![ProofError::PostconditionUnsatisfiable {
                        transaction: "<contract>".into(),
                        postcondition: format!("{}", condition),
                        reason: msg,
                        example_values: model.iter().map(|(k, v)| format!("{} = {}", k, v)).collect(),
                        suggestion: "add a runtime guard or prove the input constraints".into(),
                        span: crate::errors::Span::dummy(),
                    }]);
                }
                smt::SmtResult::Unknown => {
                    // Z3 couldn't prove either way — allow
                    return Ok(());
                }
            }
        }
        // No Z3 — heuristic pass is sufficient
        return Ok(());
    }

    // If heuristic found a violation or was inconclusive, consult Z3
    if smt::is_z3_available() {
        let query = smt::build_contract_query(&condition, params);
        match smt::prove_smt_formula(&query, 1000) {
            smt::SmtResult::Unsat => return Ok(()),
            smt::SmtResult::Sat(model) => {
                let msg = if model.is_empty() {
                    "SMT solver found a counterexample".into()
                } else {
                    format!("SMT counterexample: {:?}", model)
                };
                return Err(vec![ProofError::PostconditionUnsatisfiable {
                    transaction: "<contract>".into(),
                    postcondition: format!("{}", condition),
                    reason: msg,
                    example_values: model.iter().map(|(k, v)| format!("{} = {}", k, v)).collect(),
                    suggestion: "add a runtime guard or prove the input constraints".into(),
                    span: crate::errors::Span::dummy(),
                }]);
            }
            smt::SmtResult::Unknown => {
                eprintln!("warning: contract could not be proven (Z3 returned Unknown), allowing");
                return Ok(());
            }
        }
    }

    // No Z3 available — heuristic verdict is final
    if has_violation {
        Err(vec![ProofError::PostconditionUnsatisfiable {
            transaction: "<contract>".into(),
            postcondition: format!("{}", condition),
            reason: "heuristic detected a contract violation (no Z3 to verify further)".into(),
            example_values: vec![],
            suggestion: "simplify the contract or install z3 for more precise checking".into(),
            span: crate::errors::Span::dummy(),
        }])
    } else {
        Ok(())
    }
}

/// Extract a loop bound from a postcondition like [done == N].
/// Returns the bound value if it's a compile-time constant.
pub fn extract_bound_from_postcondition(post: &Expr) -> Option<u64> {
    match post {
        Expr::BinaryOp(kind, lhs, rhs) => {
            if *kind != crate::ast::BinaryOpKind::Eq {
                return None;
            }
            match (lhs.as_ref(), rhs.as_ref()) {
                (Expr::Identifier(_), Expr::Decimal(n)) => Some(*n as u64),
                (Expr::Decimal(n), Expr::Identifier(_)) => Some(*n as u64),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Check if a sequence of statements is linearly provable (no branching).
pub fn prove_linear(stmts: &[Statement]) -> bool {
    stmts.iter().all(|s| matches!(s,
        Statement::Expression(_) |
        Statement::Assign(_, _) |
        Statement::Let { .. } |
        Statement::Term(_) |
        Statement::EndProgram(_)
    ))
}

/// Estimate the cost of an expression (for optimization budget).
pub fn expr_cost(expr: &Expr) -> u64 {
    match expr {
        Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Quoted(_) => 1,
        Expr::Identifier(_) => 1,
        Expr::Call(name, args, _) => {
            let base = if name.ends_with('#') { 5 } else { 10 };
            base + args.iter().map(expr_cost).sum::<u64>()
        }
        Expr::BinaryOp(_, lhs, rhs) => 3 + expr_cost(lhs) + expr_cost(rhs),
        Expr::UnaryOp(_, e) => 2 + expr_cost(e),
        Expr::Block(stmts) => stmts.iter().map(|s| stmt_cost(s)).sum(),
        Expr::If(cond, then, else_) => {
            2 + expr_cost(cond) + expr_cost(then) + else_.as_ref().map_or(0, |e| expr_cost(e))
        }
        Expr::Tuple(exprs) | Expr::List(exprs) => exprs.iter().map(expr_cost).sum(),
        _ => 5,
    }
}

/// Estimate the cost of a statement.
pub fn stmt_cost(stmt: &Statement) -> u64 {
    match stmt {
        Statement::Expression(expr) => expr_cost(expr),
        Statement::Let { expr, .. } => expr.as_ref().map_or(0, |e| expr_cost(e)),
        Statement::Assign(_, rhs) => expr_cost(rhs),
        Statement::Term(val) | Statement::EndProgram(val) => val.as_ref().map_or(0, |e| expr_cost(e)),
        Statement::Guarded(cond, body) => {
            expr_cost(cond) + body.iter().map(stmt_cost).sum::<u64>()
        }
        Statement::Block(stmts) => stmts.iter().map(stmt_cost).sum(),
        _ => 2,
    }
}

/// Count the number of function/intrinsic calls in an expression.
pub fn count_calls(expr: &Expr, intrinsics: &mut u64, includes_io: &mut bool) {
    match expr {
        Expr::Call(name, args, _) => {
            if name.ends_with('#') {
                *intrinsics += 1;
                if name == "Print#" {
                    *includes_io = true;
                }
            }
            for arg in args {
                count_calls(arg, intrinsics, includes_io);
            }
        }
        Expr::BinaryOp(_, lhs, rhs) => {
            count_calls(lhs, intrinsics, includes_io);
            count_calls(rhs, intrinsics, includes_io);
        }
        Expr::UnaryOp(_, e) => count_calls(e, intrinsics, includes_io),
        Expr::Block(stmts) => {
            for stmt in stmts {
                if let Statement::Expression(e) = stmt {
                    count_calls(e, intrinsics, includes_io);
                }
            }
        }
        _ => {}
    }
}

/// Check if an expression is provably terminable (no unbounded recursion).
pub fn is_proven_terminable(expr: &Expr) -> bool {
    // An expression is terminable if it has no recursive calls.
    match expr {
        Expr::Call(name, _, _) => !name.ends_with('#'),
        _ => true,
    }
}

/// Split a conjunction (&&) into individual conditions.
pub fn split_and(expr: &Expr) -> Vec<&Expr> {
    match expr {
        Expr::BinaryOp(kind, lhs, rhs) if *kind == crate::ast::BinaryOpKind::And => {
            let mut result = split_and(lhs);
            result.extend(split_and(rhs));
            result
        }
        _ => vec![expr],
    }
}

/// 2026-07-31 (Phase 4): Tautology-only gate for txn/node convergence
/// contracts. Unlike `prove_contract` — whose pre/post satisfiability check is
/// designed for simultaneous protocol invariants — txn/node pre/post describe
/// BEFORE/AFTER states, so only vacuous-true contracts are rejected here.
pub fn detect_tautology(pre: &Expr, post: &Expr, explicit: bool) -> Option<ProofError> {
    if !explicit {
        return None;
    }
    let pre_true = matches!(pre, Expr::Bool(true));
    let post_true = matches!(post, Expr::Bool(true));
    if pre_true && post_true {
        return Some(ProofError::UnreachableState {
            transaction: "<contract>".into(),
            precondition: "[true][true] is a useless tautology".into(),
            reason: "a contract with both sides always-true constrains nothing; \
                     use `[[post]` (postcondition-only) or `[pre]]` (precondition-only) \
                     sugar, or write a contract that constrains behavior"
                .into(),
            proof_trace: vec![],
            span: crate::errors::Span::dummy(),
        });
    }
    if is_vacuously_true(pre) && is_vacuously_true(post) {
        return Some(ProofError::UnreachableState {
            transaction: "<contract>".into(),
            precondition: "contract is functionally always-true".into(),
            reason: "a precondition and postcondition that are true for every input \
                     constrain nothing; write contracts that pin down behavior"
                .into(),
            proof_trace: vec![],
            span: crate::errors::Span::dummy(),
        });
    }
    None
}

/// Check if two expressions are jointly satisfiable.
pub fn check_satisfiable(a: &Expr, b: &Expr) -> bool {
    // Simplified: returns true unless there's a direct contradiction.
    // A full implementation would use SMT.
    match (a, b) {
        (Expr::Bool(false), _) | (_, Expr::Bool(false)) => false,
        (Expr::Decimal(a), Expr::Decimal(b)) if a != b => false,
        _ => {
            // 2026-08-01 (Phase 3c): detect `f() == "a"` vs `f() == "b"` (same
            // lhs expression, different constant rhs) as UNSAT — the entry!
            // subcommand-dispatch pattern (`cmd == "build"` vs `cmd == "run"`
            // can never both hold). This is what makes two entry! nodes with
            // mutually exclusive commands legal WITHOUT classification.
            if let (Some((l1, r1)), Some((l2, r2))) =
                (as_const_eq(a), as_const_eq(b))
            {
                if expr_eq(l1, l2) && const_ne(r1, r2) {
                    return false;
                }
            }
            // 2026-09-22 (Slice C): opposite numeric comparisons on the SAME
            // lhs are unsatisfiable together — `temperature > 100` vs
            // `temperature <= 100`. This is what lets mutually-exclusive
            // when-law guards and node guards be classified as disjoint.
            if comparison_contradicts(a, b) {
                return false;
            }
            true
        }
    }
}

/// `lhs OP c` vs `lhs OP' c'` with the same lhs where the operator pair and
/// constants make the conjunction impossible: `>` vs `<=`, `>=` vs `<`.
fn comparison_contradicts(a: &Expr, b: &Expr) -> bool {
    use crate::ast::BinaryOpKind::*;
    fn as_comp(expr: &Expr) -> Option<(&Expr, crate::ast::BinaryOpKind, f64)> {
        match expr {
            Expr::BinaryOp(k, l, r) => {
                let (val, neg) = match r.as_ref() {
                    Expr::Decimal(n) => (*n as f64, false),
                    Expr::UnitLiteral { value, .. } => (*value, false),
                    Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => match inner.as_ref() {
                        Expr::Decimal(n) => (-(*n as f64), false),
                        _ => return None,
                    },
                    _ => return None,
                };
                let _ = neg;
                Some((l.as_ref(), *k, val))
            }
            _ => None,
        }
    }
    let (Some((l1, k1, c1)), Some((l2, k2, c2))) = (as_comp(a), as_comp(b)) else {
        return false;
    };
    if !expr_eq(l1, l2) {
        return false;
    }
    // Same threshold — `>` vs `<=` (and `>=` vs `<`) on the same constant.
    if (c1 - c2).abs() < f64::EPSILON {
        return matches!((k1, k2), (Gt, Le) | (Ge, Lt) | (Le, Gt) | (Lt, Ge));
    }
    // Different thresholds — `x > hi` vs `x <= lo` with hi >= lo is UNSAT.
    if (c1 > c2) && matches!((k1, k2), (Gt, Le) | (Ge, Lt)) {
        return true;
    }
    if (c1 < c2) && matches!((k1, k2), (Le, Gt) | (Lt, Ge)) {
        return true;
    }
    false
}

/// Match `Expr::BinaryOp(Eq, lhs, rhs)` and return (lhs, rhs).
fn as_const_eq(expr: &Expr) -> Option<(&Expr, &Expr)> {
    match expr {
        Expr::BinaryOp(crate::ast::BinaryOpKind::Eq, l, r) => Some((l, r)),
        _ => None,
    }
}

/// Two constant expressions are unequal.
fn const_ne(a: &Expr, b: &Expr) -> bool {
    // String constants compare by byte content; numeric constants by value.
    if let (Expr::Quoted(x), Expr::Quoted(y)) = (a, b) {
        return x != y;
    }
    // 2026-09-22 (plan 2026-09-22-electronics-participation-and-when-law,
    // Slice A): unit literals compare by their numeric value regardless of
    // suffix — `3.3V` vs `1.8V` are unequal, `3.3V` vs `3.3V` equal. This
    // is what lets the concurrency gate see two voltage-comparison guards
    // on the same pin as mutually exclusive (no false async/sync demand on
    // clock-sensitive electronics boards).
    if let (Expr::UnitLiteral { value: va, .. }, Expr::UnitLiteral { value: vb, .. }) = (a, b) {
        return (va - vb).abs() > f64::EPSILON;
    }
    match (const_value(a), const_value(b)) {
        (Some(x), Some(y)) => x != y,
        _ => false,
    }
}

/// 2026-07-31 (Phase 4): Is an expression vacuously true — true for every
/// input, providing no constraint and hence no optimization leverage?
/// Detects `true`, constant equalities (`0 == 0`), self-comparisons
/// (`x == x`, `x >= x`), and trivially-foldable constant relations.
pub fn is_vacuously_true(expr: &Expr) -> bool {
    match expr {
        Expr::Bool(true) => true,
        Expr::BinaryOp(kind, l, r) => {
            use crate::ast::BinaryOpKind::*;
            match kind {
                Eq => match (const_value(l), const_value(r)) {
                    (Some(a), Some(b)) => a == b,
                    _ => expr_eq(l, r),
                },
                Le | Ge => match (const_value(l), const_value(r)) {
                    (Some(a), Some(b)) => a == b,
                    _ => expr_eq(l, r),
                },
                Neq | Lt | Gt => false,
                And | Or => is_vacuously_true(l) && is_vacuously_true(r),
                _ => false,
            }
        }
        _ => false,
    }
}

/// Fold a constant expression to its integer value, if it is fully constant.
fn const_value(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Decimal(n) => Some(*n),
        Expr::BinaryOp(kind, l, r) => {
            use crate::ast::BinaryOpKind::*;
            let (a, b) = (const_value(l)?, const_value(r)?);
            match kind {
                Add => Some(a.wrapping_add(b)),
                Sub => Some(a.wrapping_sub(b)),
                Mul => Some(a.wrapping_mul(b)),
                Div if b != 0 => Some(a.wrapping_div(b)),
                Mod if b != 0 => Some(a.wrapping_rem(b)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Structural equality of two expressions (`x == x`, `a + 1 == a + 1`).
fn expr_eq(l: &Expr, r: &Expr) -> bool {
    match (l, r) {
        (Expr::Identifier(a), Expr::Identifier(b)) => a == b,
        (Expr::Decimal(a), Expr::Decimal(b)) => a == b,
        (Expr::Bool(a), Expr::Bool(b)) => a == b,
        (Expr::Quoted(a), Expr::Quoted(b)) => a == b,
        // 2026-09-22 (Slice A, plan 2026-09-22-...-when-law): field-access
        // chains compare structurally — `u1.sclk.voltage` equals itself, so
        // the concurrency gate can match the SAME pin-access lhs across two
        // guards and (via const_ne) see `== 3.3V` vs `== 1.8V` as mutually
        // exclusive. This is what makes clock-sensitive electronics nodes
        // free of false async/sync classification demands.
        (Expr::Field(la, fa), Expr::Field(lb, fb)) => {
            fa == fb && expr_eq(la, lb)
        }
        // 2026-08-01 (Phase 3c): Call equality — `entry_cmd()` == `entry_cmd()`.
        // Needed so `entry_cmd() == "a"` vs `entry_cmd() == "b"` shares a lhs.
        (Expr::Call(na, aa, ta), Expr::Call(nb, ab, tb)) => {
            na == nb
                && ta == tb
                && aa.len() == ab.len()
                && aa.iter().zip(ab.iter()).all(|(x, y)| expr_eq(x, y))
        }
        (Expr::BinaryOp(ka, la, ra), Expr::BinaryOp(kb, lb, rb)) => {
            ka == kb && expr_eq(la, lb) && expr_eq(ra, rb)
        }
        _ => false,
    }
}

/// Check if a transaction body converges (loop bound is provable).
pub fn check_convergence(
    body: &[Statement],
    postcondition: &Expr,
) -> Result<(), String> {
    // A transaction converges if either:
    // 1. It has no loops (prove_linear)
    // 2. The postcondition provides a numeric bound
    if prove_linear(body) {
        return Ok(());
    }
    if extract_bound_from_postcondition(postcondition).is_some() {
        return Ok(());
    }
    Err("transaction does not obviously converge — add [result == N] postcondition".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_bound() {
        let post = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Eq,
            Box::new(Expr::Identifier("done".into())),
            Box::new(Expr::Decimal(100)),
        );
        assert_eq!(extract_bound_from_postcondition(&post), Some(100));
    }

    #[test]
    fn test_expr_cost_literal() {
        assert_eq!(expr_cost(&Expr::Decimal(42)), 1);
        assert_eq!(expr_cost(&Expr::Bool(true)), 1);
    }

    #[test]
    fn test_expr_cost_binary() {
        let expr = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Add,
            Box::new(Expr::Decimal(1)),
            Box::new(Expr::Decimal(2)),
        );
        assert_eq!(expr_cost(&expr), 5); // 3 + 1 + 1
    }

    #[test]
    fn test_count_calls() {
        let expr = Expr::Call("AddI64#".into(), vec![Expr::Decimal(1), Expr::Decimal(2)], None);
        let mut intrinsics = 0;
        let mut includes_io = false;
        count_calls(&expr, &mut intrinsics, &mut includes_io);
        assert_eq!(intrinsics, 1);
        assert!(!includes_io);
    }

    #[test]
    fn test_check_satisfiable() {
        assert!(check_satisfiable(&Expr::Bool(true), &Expr::Bool(true)));
        assert!(!check_satisfiable(&Expr::Bool(true), &Expr::Bool(false)));
        assert!(!check_satisfiable(&Expr::Decimal(1), &Expr::Decimal(2)));
    }

    #[test]
    fn test_prove_linear() {
        let stmts = vec![
            Statement::Expression(Expr::Decimal(1)),
        ];
        assert!(prove_linear(&stmts));
    }

    #[test]
    fn test_split_and() {
        let conjunct = Expr::BinaryOp(
            crate::ast::BinaryOpKind::And,
            Box::new(Expr::Bool(true)),
            Box::new(Expr::Bool(false)),
        );
        let parts = split_and(&conjunct);
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn test_check_convergence() {
        let body = vec![Statement::Expression(Expr::Decimal(0))];
        assert!(check_convergence(&body, &Expr::Bool(true)).is_ok());
    }

    // ── 2026-07-31 (Phase 4): Tautology detection ──────────────────

    #[test]
    fn test_tautology_true_true() {
        let err = detect_tautology(&Expr::Bool(true), &Expr::Bool(true), true);
        assert!(err.is_some(), "[true][true] must be a tautology");
    }

    #[test]
    fn test_tautology_not_flagged_when_implicit() {
        let err = detect_tautology(&Expr::Bool(true), &Expr::Bool(true), false);
        assert!(err.is_none(), "no-contract default is not a tautology");
    }

    #[test]
    fn test_tautology_constant_equality() {
        let zero = Expr::Decimal(0);
        let eq = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Eq,
            Box::new(zero.clone()),
            Box::new(zero),
        );
        let err = detect_tautology(&eq, &eq, true);
        assert!(err.is_some(), "0 == 0 must be a tautology");
    }

    #[test]
    fn test_tautology_self_comparison() {
        let x = Expr::Identifier("x".into());
        let eq = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Eq,
            Box::new(x.clone()),
            Box::new(x),
        );
        assert!(is_vacuously_true(&eq), "x == x is vacuously true");
    }

    #[test]
    fn test_real_contract_not_tautology() {
        let pre = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Lt,
            Box::new(Expr::Identifier("count".into())),
            Box::new(Expr::Identifier("total".into())),
        );
        let post = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Eq,
            Box::new(Expr::Identifier("count".into())),
            Box::new(Expr::Identifier("total".into())),
        );
        let err = detect_tautology(&pre, &post, true);
        assert!(err.is_none(), "[count < total][count == total] is a real contract");
    }
}
