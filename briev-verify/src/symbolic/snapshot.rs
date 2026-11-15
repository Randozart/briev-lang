// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Convert evaluated values without re-reading later mutable bindings.

use super::SymbolicValue;
use crate::ast::{BinaryOpKind as Op, Expr};
use std::collections::HashMap;

const VALUE_DEPTH_CAP: usize = 8;

/// Only identifiers in the expression being evaluated consult current bindings.
pub(crate) fn expression(expr: &Expr, bindings: &HashMap<String, SymbolicValue>) -> Option<Expr> {
    match expr {
        Expr::Identifier(name) => match bindings.get(name) {
            Some(value) => value_expr(value),
            None => Some(expr.clone()),
        },
        Expr::BinaryOp(op, left, right) => Some(Expr::BinaryOp(
            *op,
            Box::new(expression(left, bindings)?),
            Box::new(expression(right, bindings)?),
        )),
        Expr::UnaryOp(op, inner) => {
            Some(Expr::UnaryOp(*op, Box::new(expression(inner, bindings)?)))
        }
        other => Some(other.clone()),
    }
}

/// Saved identifiers are entry atoms, not references to the current binding map.
pub(crate) fn value_expr(value: &SymbolicValue) -> Option<Expr> {
    convert(value, 0)
}

fn convert(value: &SymbolicValue, depth: usize) -> Option<Expr> {
    if depth > VALUE_DEPTH_CAP {
        return None;
    }
    match value {
        SymbolicValue::Binary(op, left, right) => Some(Expr::BinaryOp(
            operator(op)?,
            Box::new(convert(left, depth + 1)?),
            Box::new(convert(right, depth + 1)?),
        )),
        other => leaf(other),
    }
}

/// Freeze primitive values without a mutable environment lookup. This is the
/// production leaf boundary shared by postconditions, summaries and guards.
fn leaf(value: &SymbolicValue) -> Option<Expr> {
    match value {
        SymbolicValue::Literal(value, _) => Some(Expr::Decimal(*value)),
        SymbolicValue::Identifier(name) => Some(Expr::Identifier(name.clone())),
        _ => None,
    }
}

fn operator(symbol: &str) -> Option<Op> {
    match symbol {
        "+" => Some(Op::Add),
        "-" => Some(Op::Sub),
        "*" => Some(Op::Mul),
        "/" => Some(Op::Div),
        "%" => Some(Op::Mod),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_saved_trees_fail_closed() {
        let mut value = SymbolicValue::Identifier("input".into());
        for _ in 0..=VALUE_DEPTH_CAP {
            value = SymbolicValue::Binary(
                "+".into(),
                Box::new(value),
                Box::new(SymbolicValue::int_literal(1)),
            );
        }
        assert!(value_expr(&value).is_none());
    }

    #[test]
    fn unknown_bindings_cannot_supply_path_facts() {
        let mut state = super::super::SymbolicState::empty();
        state.assignments.insert("x".into(), SymbolicValue::Unknown);
        let condition = Expr::BinaryOp(
            Op::Eq,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Decimal(0)),
        );
        state.add_constraint(&condition, true);
        state.add_constraint(&condition, false);
        assert!(state.path_constraints.is_empty());
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Every literal bit and the saved input atom survive production leaf
    /// conversion; unknown values cannot acquire an expression. This proves the
    /// leaf kernel, not recursive AST conversion or the HashMap caller.
    #[kani::proof]
    #[kani::unwind(6)]
    fn snapshot_leaf_conversion_is_exact() {
        let literal: i64 = kani::any();
        let which: u8 = kani::any();
        kani::assume(which < 3);
        let value = std::mem::ManuallyDrop::new(match which {
            0 => SymbolicValue::int_literal(literal),
            1 => SymbolicValue::Identifier("input".into()),
            _ => SymbolicValue::Unknown,
        });
        // Exclude recursive input/output destructors from this conversion proof.
        let converted = std::mem::ManuallyDrop::new(leaf(&value));
        match which {
            0 => assert!(matches!(converted.as_ref(), Some(Expr::Decimal(n)) if *n == literal)),
            1 => assert!(
                matches!(converted.as_ref(), Some(Expr::Identifier(name)) if name == "input")
            ),
            _ => assert!(converted.is_none()),
        }
        kani::cover!(which == 0 && literal == i64::MIN);
        kani::cover!(which == 0 && literal == i64::MAX);
        kani::cover!(which == 1);
        kani::cover!(which == 2);
    }
}
