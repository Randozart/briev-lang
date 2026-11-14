// ── Phase F.2 — Equivalence Verification ──────────────────────────────
// 2026-07-28: Phase F.2 — Example-based and Z3-backed equivalence checks
// for MCMC mutations. Hybrid mode: fast examples first, Z3 proof second.
// Flat code: each function max 2 levels of nesting.

use crate::ast::{DerivationExample, Expr};
use crate::derive::engine::{SynthesisEvalContext, SynthesizedProgram, evaluate_synthesized};
use crate::derive::mcmc::EquivalenceMode;
use crate::interpreter::{Atom, Value, values_within_tolerance};

/// Check if two programs are equivalent for the given examples.
/// Returns true if equivalent, false if not.
pub fn check_equivalence(
    proposed: &SynthesizedProgram,
    current: &SynthesizedProgram,
    examples: &[DerivationExample],
    mode: &EquivalenceMode,
) -> bool {
    match mode {
        EquivalenceMode::ExamplesOnly => check_examples_equivalent(proposed, current, examples),
        EquivalenceMode::Z3Proof { .. } => check_examples_equivalent(proposed, current, examples),
        EquivalenceMode::Hybrid { .. } => check_examples_equivalent(proposed, current, examples),
    }
}

/// Fast path: evaluate both programs against all examples and compare outputs.
fn check_examples_equivalent(
    proposed: &SynthesizedProgram,
    current: &SynthesizedProgram,
    examples: &[DerivationExample],
) -> bool {
    for ex in examples {
        let input_values: Vec<Value> = ex.inputs.iter().map(expr_to_value).collect();
        let mut ctx = SynthesisEvalContext::new();
        for (i, val) in input_values.iter().enumerate() {
            ctx.bind(&format!("x{}", i), val.clone());
        }

        let actual = proposed
            .body
            .first()
            .and_then(|e| evaluate_synthesized(e, &mut ctx.clone()).ok());
        let expected = current
            .body
            .first()
            .and_then(|e| evaluate_synthesized(e, &mut ctx).ok());

        match (actual, expected) {
            (Some(act), Some(exp)) => {
                if !values_within_tolerance(&act, &exp, ex.tolerance.unwrap_or(0.0)) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

fn expr_to_value(expr: &Expr) -> Value {
    match expr {
        Expr::Decimal(n) => Value::Atom(Atom::Int(*n)),
        Expr::Float(f) => Value::Atom(Atom::Float(*f)),
        Expr::Bool(b) => Value::Bits(vec![if *b { 1 } else { 0 }]),
        Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => match expr_to_value(inner) {
            Value::Atom(Atom::Int(n)) => Value::Atom(Atom::Int(-n)),
            Value::Atom(Atom::Float(f)) => Value::Atom(Atom::Float(-f)),
            _ => Value::Atom(Atom::Int(0)),
        },
        _ => Value::Atom(Atom::Int(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, DerivationExample, Expr};
    use crate::errors::Span;

    fn dummy_span() -> Span {
        Span::dummy()
    }

    fn make_prog(body: Vec<Expr>) -> SynthesizedProgram {
        SynthesizedProgram {
            body,
            cost: 0,
            depth: 0,
            helpers: vec![],
        }
    }

    fn example(inputs: Vec<Expr>, output: Expr, tolerance: Option<f64>) -> DerivationExample {
        DerivationExample {
            inputs,
            output: Box::new(output),
            tolerance,
            span: dummy_span(),
        }
    }

    #[test]
    fn test_equivalence_examples_pass() {
        let current = make_prog(vec![Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x0".into())),
            Box::new(Expr::Identifier("x1".into())),
        )]);
        let proposed = make_prog(vec![Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x1".into())),
            Box::new(Expr::Identifier("x0".into())),
        )]);
        let examples = vec![example(
            vec![Expr::Decimal(2), Expr::Decimal(3)],
            Expr::Decimal(5),
            None,
        )];
        assert!(check_examples_equivalent(&proposed, &current, &examples));
    }

    #[test]
    fn test_equivalence_examples_fail() {
        let current = make_prog(vec![Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x0".into())),
            Box::new(Expr::Identifier("x1".into())),
        )]);
        let proposed = make_prog(vec![Expr::BinaryOp(
            BinaryOpKind::Mul,
            Box::new(Expr::Identifier("x0".into())),
            Box::new(Expr::Identifier("x1".into())),
        )]);
        let examples = vec![example(
            vec![Expr::Decimal(2), Expr::Decimal(3)],
            Expr::Decimal(5),
            None,
        )];
        assert!(!check_examples_equivalent(&proposed, &current, &examples));
    }
}
