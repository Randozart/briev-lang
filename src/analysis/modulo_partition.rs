// ── Modulo Partition Detection ──────────────────────────────────────
//
// 2026-07-31: Phase 2 (plan §7.2) — detect "every reactive txn precondition
// is `count % K == N` for a common K" ONCE in the frontend. Replaces the
// backend's `extract_mod_info` / `extract_mod_guard` (ssa.rs:68-117), which
// re-walked precondition expressions for every dispatch.
//
// The backend consumer keeps the structural dispatch choice (rotated loop
// whenever the txn set has a bounded counter precondition — the only form
// that handles a bounded counter, per the comment at ssa.rs:55-57) but the
// partition itself (counter, divisor, residue→txn cases) is computed here.
//
// Semantics are replicated exactly from the old backend helpers:
//   extract_mod_info:  Mod(counter, divisor) | Eq(...) | And(...) recursion
//   extract_mod_guard: And(...) recursion | Eq(Mod(counter, divisor), N)

use crate::ast::{BinaryOpKind, Expr, TopLevel};
use std::collections::HashMap;

/// A modulo partition: every reactive txn precondition is
/// `count % divisor == residue` for a common (counter, divisor).
#[derive(Debug, Clone, PartialEq)]
pub struct ModuloPartition {
    /// The counter register name (e.g. "count").
    pub counter: String,
    /// The common divisor K (e.g. 8).
    pub divisor: i64,
    /// (residue, txn name) — the residue N in `count % K == N` per txn.
    pub cases: Vec<(i64, String)>,
}

/// Detect a modulo partition over the reactive txns in program order.
///
/// 2026-07-31: Mirrors the old `try_modulo_switch_dispatch` gate exactly —
/// at least 2 reactive txns, a valid (counter, divisor) from the FIRST txn's
/// precondition, and every txn's precondition must reduce to the same
/// counter/divisor. The result is None if any txn fails to match.
pub fn detect_modulo_partition(items: &[TopLevel]) -> Option<ModuloPartition> {
    let reactive: Vec<&crate::ast::Transaction> = items
        .iter()
        .filter_map(|item| match item {
            TopLevel::Transaction(t) if t.is_reactive => Some(t),
            _ => None,
        })
        .collect();
    if reactive.len() < 2 {
        return None;
    }
    let first_pre = &reactive[0].contract.pre_condition;
    let (counter, divisor) = extract_mod_info(first_pre)?;
    let mut cases: Vec<(i64, String)> = Vec::new();
    for t in &reactive {
        let residue = extract_mod_guard(&t.contract.pre_condition, &counter, divisor)?;
        cases.push((residue, t.name.clone()));
    }
    if cases.len() != reactive.len() {
        return None;
    }
    Some(ModuloPartition {
        counter,
        divisor,
        cases,
    })
}

/// Extract `(counter, divisor)` from a modulo precondition.
///
/// 2026-07-31: Replicates `extract_mod_info` (ssa.rs:68-93) — accepts
/// `count % K`, `count % K == N` (Eq wraps Mod), and compound `&&` chains.
fn extract_mod_info(expr: &Expr) -> Option<(String, i64)> {
    match expr {
        Expr::BinaryOp(kind, l, r) if *kind == BinaryOpKind::Mod => {
            let counter = match l.as_ref() {
                Expr::Identifier(n) => n.clone(),
                _ => return None,
            };
            let divisor = match r.as_ref() {
                Expr::Decimal(d) => *d,
                _ => return None,
            };
            Some((counter, divisor))
        }
        Expr::BinaryOp(kind, l, r) if *kind == BinaryOpKind::Eq => {
            extract_mod_info(l).or_else(|| extract_mod_info(r))
        }
        Expr::BinaryOp(kind, l, r) if *kind == BinaryOpKind::And => {
            extract_mod_info(l).or_else(|| extract_mod_info(r))
        }
        _ => None,
    }
}

/// Extract the expected modulo residue from a guard expression.
///
/// 2026-07-31: Replicates `extract_mod_guard` (ssa.rs:96-117) — the Eq arm
/// requires the LHS to be exactly `Mod(counter, divisor)` and the RHS a
/// Decimal residue.
fn extract_mod_guard(expr: &Expr, counter: &str, divisor: i64) -> Option<i64> {
    match expr {
        Expr::BinaryOp(kind, l, r) if *kind == BinaryOpKind::And => {
            extract_mod_guard(l, counter, divisor).or_else(|| extract_mod_guard(r, counter, divisor))
        }
        Expr::BinaryOp(kind, l, r) if *kind == BinaryOpKind::Eq => {
            let inner = match l.as_ref() {
                Expr::BinaryOp(k, cl, cr)
                    if *k == BinaryOpKind::Mod
                        && matches!(cl.as_ref(), Expr::Identifier(n) if n == counter)
                        && matches!(cr.as_ref(), Expr::Decimal(d) if *d == divisor) =>
                {
                    l.as_ref()
                }
                _ => return None,
            };
            let _ = inner;
            match r.as_ref() {
                Expr::Decimal(v) => Some(*v),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Map txn name → its reactive Transaction (program order preserved).
///
/// 2026-07-31: Used by the backend consumer to correlate a partition's
/// cases with the transition graph's bounded_pre per txn.
pub fn reactive_txns_by_name(items: &[TopLevel]) -> HashMap<String, &crate::ast::Transaction> {
    let mut out = HashMap::new();
    for item in items {
        if let TopLevel::Transaction(t) = item {
            if t.is_reactive {
                out.insert(t.name.clone(), t);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Contract, Transaction};

    fn mod_expr(counter: &str, divisor: i64) -> Expr {
        Expr::BinaryOp(
            BinaryOpKind::Mod,
            Box::new(Expr::Identifier(counter.to_string())),
            Box::new(Expr::Decimal(divisor)),
        )
    }

    fn eq(l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(BinaryOpKind::Eq, Box::new(l), Box::new(r))
    }

    fn and(l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(BinaryOpKind::And, Box::new(l), Box::new(r))
    }

    fn lt(l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(BinaryOpKind::Lt, Box::new(l), Box::new(r))
    }

    fn txn(name: &str, pre: Expr) -> TopLevel {
        TopLevel::Transaction(Transaction {
            variadic_param: None,
            name: name.to_string(),
            is_reactive: true,
            is_async: false,
            type_params: Vec::new(),
            parameters: Vec::new(),
            output_type: None,
            outputs: Vec::new(),
            contract: Contract {
                pre_condition: pre,
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: Vec::new(),
            metadata: HashMap::new(),
            derivation: None,
            modifiers: Vec::new(),
            span: None,
            doc: None,
        })
    }

    /// Sparse-dispatch style: `count < total && count % 8 == N` for N=0..7.
    #[test]
    fn detects_eight_way_partition() {
        let mut items = Vec::new();
        for n in 0..8i64 {
            let pre = and(
                lt(Expr::Identifier("count".into()), Expr::Identifier("total".into())),
                eq(mod_expr("count", 8), Expr::Decimal(n)),
            );
            items.push(txn(&format!("t{}", n), pre));
        }
        let p = detect_modulo_partition(&items).unwrap();
        assert_eq!(p.counter, "count");
        assert_eq!(p.divisor, 8);
        assert_eq!(p.cases.len(), 8);
        for (i, (residue, name)) in p.cases.iter().enumerate() {
            assert_eq!(*residue, i as i64);
            assert_eq!(name, &format!("t{}", i));
        }
    }

    /// A single reactive txn is not a partition.
    #[test]
    fn single_txn_is_not_partition() {
        let pre = eq(mod_expr("count", 8), Expr::Decimal(0));
        let items = vec![txn("t0", pre)];
        assert!(detect_modulo_partition(&items).is_none());
    }

    /// Mixed residues on the SAME counter/divisor still partition.
    #[test]
    fn detects_two_way_partition() {
        let items = vec![
            txn("even", eq(mod_expr("count", 2), Expr::Decimal(0))),
            txn("odd", eq(mod_expr("count", 2), Expr::Decimal(1))),
        ];
        let p = detect_modulo_partition(&items).unwrap();
        assert_eq!(p.divisor, 2);
        assert_eq!(p.cases, vec![(0, "even".to_string()), (1, "odd".to_string())]);
    }

    /// A txn without a matching residue breaks the partition.
    #[test]
    fn divergent_precondition_breaks_partition() {
        let items = vec![
            txn("t0", eq(mod_expr("count", 8), Expr::Decimal(0))),
            txn("t1", lt(Expr::Identifier("count".into()), Expr::Identifier("total".into()))),
        ];
        assert!(detect_modulo_partition(&items).is_none());
    }

    /// A txn using a DIFFERENT divisor breaks the partition.
    #[test]
    fn different_divisor_breaks_partition() {
        let items = vec![
            txn("t0", eq(mod_expr("count", 8), Expr::Decimal(0))),
            txn("t1", eq(mod_expr("count", 4), Expr::Decimal(1))),
        ];
        assert!(detect_modulo_partition(&items).is_none());
    }
}
