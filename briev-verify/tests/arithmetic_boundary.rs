// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

use briev_verify::ast::{BinaryOpKind, Expr};
use briev_verify::symbolic::{SymbolicState, SymbolicValue, eval_symbolic, eval_symbolic_expr};
use std::collections::HashMap;

#[test]
fn literal_arithmetic_matches_wrapping_execution_in_both_evaluators() {
    for (op, a, b, expected) in [
        (BinaryOpKind::Add, i64::MAX, 1, i64::MIN),
        (BinaryOpKind::Sub, i64::MIN, 1, i64::MAX),
        (BinaryOpKind::Mul, i64::MAX, 2, -2),
        (BinaryOpKind::Mul, i64::MIN, -1, i64::MIN),
        (BinaryOpKind::Div, -7, 3, -2),
        (BinaryOpKind::Div, 7, -3, -2),
        (BinaryOpKind::Div, i64::MIN, 1, i64::MIN),
    ] {
        let expr = Expr::BinaryOp(op, Box::new(Expr::Decimal(a)), Box::new(Expr::Decimal(b)));
        let expected = SymbolicValue::int_literal(expected);
        assert_eq!(
            eval_symbolic(&expr, &SymbolicState::new(&Expr::Bool(true))),
            expected
        );
        assert_eq!(eval_symbolic_expr(&expr, &HashMap::new()), expected);
    }
}

#[test]
fn exceptional_literal_division_does_not_panic_or_invent_a_constant() {
    for (a, b) in [(1, 0), (i64::MIN, -1)] {
        let expr = Expr::BinaryOp(
            BinaryOpKind::Div,
            Box::new(Expr::Decimal(a)),
            Box::new(Expr::Decimal(b)),
        );
        assert!(matches!(
            eval_symbolic(&expr, &SymbolicState::new(&Expr::Bool(true))),
            SymbolicValue::Binary(..)
        ));
        assert!(matches!(
            eval_symbolic_expr(&expr, &HashMap::new()),
            SymbolicValue::Binary(..)
        ));
    }
}
