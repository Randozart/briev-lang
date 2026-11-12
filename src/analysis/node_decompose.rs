// ── Composite-Node Decomposition ────────────────────────────────
//
// 2026-07-31: Decompose a reactive transaction body containing `when`
// guards into a version DAG (§11 of
// docs/plans/2026-07-30-flat-node-decomposition.md).
//
// A `when` guard has no else chain — the body is a sequence of segments
// separated by guards. Each guard splits the body into [pre], [guard],
// [post]. The guard predicate is evaluated AT THE SPLIT POINT, which
// captures whether the guard observes the counter pre- or post-increment
// naturally (no position scanning, no counter-name matching).
//
// Static predicate simplification: a provably always-true guard body is
// inlined (or kept apart for LLVM); a provably always-false guard body is
// dropped; a runtime-dependent guard produces two versions.

use crate::ast::{BinaryOpKind, Expr, Statement};

/// A contiguous run of non-guard statements, or a single guard.
#[derive(Debug, Clone)]
pub enum Segment {
    /// Contiguous compute statements (no `when` guards at top level).
    Compute(Vec<Statement>),
    /// A top-level `when` guard.
    Guard {
        /// The guard condition — evaluated at the split point.
        condition: Expr,
        /// The guard body (the statements inside `{ ... }`).
        body: Vec<Statement>,
        /// Static classification of the condition.
        classification: PredicateClass,
        /// Nested decomposition of the guard body (recursion).
        nested: Vec<Segment>,
    },
}

/// Static classification of a guard predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateClass {
    /// Provably always true — inline the guard body (or keep apart for LLVM).
    AlwaysTrue,
    /// Provably always false — drop the guard body.
    AlwaysFalse,
    /// Runtime-dependent — produce guard-present / guard-absent versions.
    Runtime,
}

/// Partition a transaction body into segments at top-level `when` guards.
/// Guard bodies are recursively decomposed into nested segments.
///
/// 2026-07-31: statement-level `match` never reaches this pass — it exists
/// only inside `$defn` macro bodies (interpreter invariant, eval.rs) and is
/// expanded at macro time. The former match→when normalizer was removed
/// 2026-08-22 (Phase 4a): it had zero callers.
pub fn split_into_segments(body: &[Statement]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut compute: Vec<Statement> = Vec::new();

    for stmt in body {
        match stmt {
            Statement::Guarded(cond, guard_body) => {
                if !compute.is_empty() {
                    segments.push(Segment::Compute(std::mem::take(&mut compute)));
                }
                // 2026-07-31: Recurse into the guard body — nested whens
                // become sub-segments (Phase 5).
                let nested = split_into_segments(guard_body);
                let classification = classify_predicate(cond);
                segments.push(Segment::Guard {
                    condition: cond.clone(),
                    body: guard_body.clone(),
                    classification,
                    nested,
                });
            }
            other => compute.push(other.clone()),
        }
    }
    if !compute.is_empty() {
        segments.push(Segment::Compute(compute));
    }
    segments
}

/// Classify a guard predicate as always-true / always-false / runtime.
///
/// 2026-07-31: Only literal booleans and trivial constant comparisons are
/// classified statically. Anything else is Runtime (conservative — the
/// verifier could extend this with contract-based proof later).
pub fn classify_predicate(cond: &Expr) -> PredicateClass {
    match cond {
        Expr::Bool(true) => PredicateClass::AlwaysTrue,
        Expr::Bool(false) => PredicateClass::AlwaysFalse,
        Expr::BinaryOp(BinaryOpKind::Eq, l, r) => match (const_value(l), const_value(r)) {
            (Some(a), Some(b)) => {
                if a == b {
                    PredicateClass::AlwaysTrue
                } else {
                    PredicateClass::AlwaysFalse
                }
            }
            _ => PredicateClass::Runtime,
        },
        Expr::BinaryOp(BinaryOpKind::Neq, l, r) => match (const_value(l), const_value(r)) {
            (Some(a), Some(b)) => {
                if a != b {
                    PredicateClass::AlwaysTrue
                } else {
                    PredicateClass::AlwaysFalse
                }
            }
            _ => PredicateClass::Runtime,
        },
        _ => PredicateClass::Runtime,
    }
}

/// Extract a constant numeric value from an expression, if it is constant.
fn const_value(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Decimal(n) => Some(*n),
        Expr::Bool(b) => Some(if *b { 1 } else { 0 }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Expr;

    #[test]
    fn test_no_guards_single_compute_segment() {
        let body = vec![
            Statement::Assign(Expr::Identifier("x".to_string()), Expr::Decimal(1)),
            Statement::Assign(Expr::Identifier("y".to_string()), Expr::Decimal(2)),
        ];
        let segs = split_into_segments(&body);
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], Segment::Compute(s) if s.len() == 2));
    }

    #[test]
    fn test_single_guard_three_segments() {
        // [compute] when cond { body } [compute]
        let body = vec![
            Statement::Assign(Expr::Identifier("a".to_string()), Expr::Decimal(1)),
            Statement::Guarded(
                Expr::Identifier("cond".to_string()),
                vec![Statement::Assign(
                    Expr::Identifier("b".to_string()),
                    Expr::Decimal(2),
                )],
            ),
            Statement::Assign(Expr::Identifier("c".to_string()), Expr::Decimal(3)),
        ];
        let segs = split_into_segments(&body);
        assert_eq!(segs.len(), 3);
        assert!(matches!(&segs[0], Segment::Compute(_)));
        assert!(matches!(&segs[1], Segment::Guard { .. }));
        assert!(matches!(&segs[2], Segment::Compute(_)));
    }

    #[test]
    fn test_classify_literals() {
        assert_eq!(
            classify_predicate(&Expr::Bool(true)),
            PredicateClass::AlwaysTrue
        );
        assert_eq!(
            classify_predicate(&Expr::Bool(false)),
            PredicateClass::AlwaysFalse
        );
        assert_eq!(
            classify_predicate(&Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(Expr::Decimal(1)),
                Box::new(Expr::Decimal(1)),
            )),
            PredicateClass::AlwaysTrue
        );
        assert_eq!(
            classify_predicate(&Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(Expr::Decimal(1)),
                Box::new(Expr::Decimal(2)),
            )),
            PredicateClass::AlwaysFalse
        );
        assert_eq!(
            classify_predicate(&Expr::Identifier("count".to_string())),
            PredicateClass::Runtime
        );
    }

    #[test]
    fn test_nested_guard_recursion() {
        // when c1 { when c2 { body } }
        let body = vec![Statement::Guarded(
            Expr::Identifier("c1".to_string()),
            vec![Statement::Guarded(
                Expr::Identifier("c2".to_string()),
                vec![Statement::Assign(
                    Expr::Identifier("x".to_string()),
                    Expr::Decimal(1),
                )],
            )],
        )];
        let segs = split_into_segments(&body);
        assert_eq!(segs.len(), 1);
        match &segs[0] {
            Segment::Guard { nested, .. } => {
                assert_eq!(nested.len(), 1);
                assert!(matches!(&nested[0], Segment::Guard { .. }));
            }
            _ => panic!("expected guard"),
        }
    }
}
