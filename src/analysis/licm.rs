// ── Loop-Invariant Code Motion ──────────────────────────────────
//
// 2026-07-29: Hoist loop-invariant let-bindings out of the loop body
// to reduce redundant computation in the hot path.
//
// An expression is loop-invariant if all its operands are:
//   - Constants (Decimal, Float, Bool)
//   - State fields that are never written (read-only state)
//   - Other loop-invariant let-bindings
//
// Safety: Side-effecting intrinsics (PrintInt#, Malloc#, GetEnvInt#)
// are never hoisted. State fields that appear in the write_set are
// treated as variant (change each iteration).
//
// See docs/plans/2026-07-29-frontend-ir-quality-improvements.md §4.

use crate::ast::{Expr, Statement};
use std::collections::{HashMap, HashSet};

/// Hoist loop-invariant let-bindings from a transaction body.
///
/// Returns (hoisted, remaining) where:
///   - hoisted: let-bindings that are loop-invariant (to emit before the loop)
///   - remaining: the original body with hoisted bindings removed
///
/// write_set contains the names of state fields that are written in the loop.
/// Any reference to a written state field makes an expression variant.
/// state_fields contains the names of ALL state fields (used to distinguish
/// state field identifiers from local variable identifiers).
pub fn hoist_loop_invariants(
    body: &[Statement],
    write_set: &HashSet<String>,
    state_fields: &HashSet<String>,
) -> (Vec<Statement>, Vec<Statement>) {
    let mut invariant_names: HashSet<String> = HashSet::new();
    let mut binding_map: HashMap<String, &Statement> = HashMap::new();

    // First pass: collect all let-bindings and their names
    for stmt in body {
        if let Statement::Let { name, .. } = stmt {
            binding_map.insert(name.clone(), stmt);
        }
    }

    // Fixed-point iteration: mark bindings as invariant if all their
    // operands are constants or already-marked invariant bindings.
    loop {
        let mut changed = false;
        for (name, stmt) in &binding_map {
            if invariant_names.contains(name) {
                continue;
            }
            if let Statement::Let { expr, .. } = stmt {
                if let Some(e) = expr {
                    if is_invariant_expression(e, write_set, &invariant_names, state_fields) {
                        invariant_names.insert(name.clone());
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Build output: separate hoisted from remaining
    let mut hoisted: Vec<Statement> = Vec::new();
    let hoisted_set: HashSet<&str> = invariant_names.iter().map(|s| s.as_str()).collect();
    let mut remaining: Vec<Statement> = Vec::new();

    for stmt in body {
        if let Statement::Let { name, .. } = stmt {
            if hoisted_set.contains(name.as_str()) {
                hoisted.push(stmt.clone());
                continue;
            }
        }
        remaining.push(stmt.clone());
    }

    (hoisted, remaining)
}

/// Check if an expression is loop-invariant.
fn is_invariant_expression(
    expr: &Expr,
    write_set: &HashSet<String>,
    invariant_names: &HashSet<String>,
    state_fields: &HashSet<String>,
) -> bool {
    match expr {
        Expr::Decimal(_) | Expr::Char(_) | Expr::Float(_) | Expr::Bool(_) | Expr::BeginProgram => true,
        Expr::Quoted(_) => true,
        Expr::Identifier(name) => {
            // Previously proven invariant let-binding
            if invariant_names.contains(name.as_str()) {
                return true;
            }
            // State field that's never written — read-only, always invariant
            if state_fields.contains(name.as_str()) {
                return !write_set.contains(name.as_str());
            }
            // Local variable not yet proven invariant → variant (conservative)
            false
        }
        Expr::BinaryOp(_, l, r) => {
            is_invariant_expression(l, write_set, invariant_names, state_fields)
                && is_invariant_expression(r, write_set, invariant_names, state_fields)
        }
        Expr::UnaryOp(_, e) => is_invariant_expression(e, write_set, invariant_names, state_fields),
        Expr::Field(obj, _) => is_invariant_expression(obj, write_set, invariant_names, state_fields),
        Expr::Cast(e, _) => is_invariant_expression(e, write_set, invariant_names, state_fields),
        Expr::Call(_, _, _) => false,
        Expr::Index(_, _) => false,
        Expr::TaggedLiteral(_, _) | Expr::TaggedQuotedLiteral(_, _) => false,
        Expr::List(items) => items.iter().all(|i| is_invariant_expression(i, write_set, invariant_names, state_fields)),
        Expr::Slice { .. } => false,
Expr::Slice { .. } => false,
        Expr::Range { .. } => false,
        Expr::Spawn { .. } => false,
        Expr::Tuple(items) => items.iter().all(|i| is_invariant_expression(i, write_set, invariant_names, state_fields)),
        Expr::Block(_) | Expr::If(_, _, _) | Expr::Match(_, _) | Expr::Lambda(_, _) => false,
        Expr::Deref(_) | Expr::AddrOf(_) | Expr::Consume(_) | Expr::Await(_) => false,
        Expr::Within(_, _) | Expr::IsType(_, _) | Expr::Exists(_) => false,
        Expr::StructLiteral { .. } => false,
        Expr::Field(_, _) | Expr::Reflect(_, _, _) | Expr::MethodCall(..) | Expr::FormattingAnnotation(_) => false,
        Expr::PluginIntercept { .. } => false,
        Expr::DerivationBlock(_) => false,
        Expr::Named { inner, .. } => is_invariant_expression(inner, write_set, invariant_names, state_fields),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, Expr, Statement};

    #[test]
    fn test_const_float_expression_is_invariant() {
        let write_set = HashSet::new();
        let state_fields = HashSet::new();
        let invariant_names = HashSet::new();
        let expr = Expr::Float(0.5);
        assert!(is_invariant_expression(&expr, &write_set, &invariant_names, &state_fields));
    }

    #[test]
    fn test_const_decimal_is_invariant() {
        let write_set = HashSet::new();
        let state_fields = HashSet::new();
        let invariant_names = HashSet::new();
        let expr = Expr::Decimal(42);
        assert!(is_invariant_expression(&expr, &write_set, &invariant_names, &state_fields));
    }

    #[test]
    fn test_written_state_field_is_variant() {
        let mut write_set = HashSet::new();
        write_set.insert("bx0".to_string());
        let mut state_fields = HashSet::new();
        state_fields.insert("bx0".to_string());
        let invariant_names = HashSet::new();
        let expr = Expr::Identifier("bx0".to_string());
        assert!(!is_invariant_expression(&expr, &write_set, &invariant_names, &state_fields));
    }

    #[test]
    fn test_read_only_state_field_is_invariant() {
        let write_set = HashSet::new();
        let mut state_fields = HashSet::new();
        state_fields.insert("bx0".to_string());
        let invariant_names = HashSet::new();
        let expr = Expr::Identifier("bx0".to_string());
        assert!(is_invariant_expression(&expr, &write_set, &invariant_names, &state_fields));
    }

    #[test]
    fn test_mul_of_const_and_invariant_is_invariant() {
        let write_set = HashSet::new();
        let state_fields = HashSet::new();
        let mut invariant_names = HashSet::new();
        invariant_names.insert("dt".to_string());
        let expr = Expr::BinaryOp(
            BinaryOpKind::Mul,
            Box::new(Expr::Identifier("dt".to_string())),
            Box::new(Expr::Float(0.5)),
        );
        assert!(is_invariant_expression(&expr, &write_set, &invariant_names, &state_fields));
    }

    #[test]
    fn test_hoist_const_let() {
        let body = vec![
            Statement::Let {
                names: vec![],
                name: "dt".to_string(),
                ty: None,
                expr: Some(Expr::Float(0.01)),
                modifiers: vec![],
            },
            Statement::Let {
                names: vec![],
                name: "x".to_string(),
                ty: None,
                expr: Some(Expr::Identifier("dt".to_string())),
                modifiers: vec![],
            },
            Statement::Term(None),
        ];
        let write_set = HashSet::new();
        let state_fields = HashSet::new();
        let (hoisted, remaining) = hoist_loop_invariants(&body, &write_set, &state_fields);
        assert_eq!(hoisted.len(), 2);
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn test_written_field_prevents_hoist() {
        let body = vec![
            Statement::Let {
                names: vec![],
                name: "x".to_string(),
                ty: None,
                expr: Some(Expr::Identifier("bx0".to_string())),
                modifiers: vec![],
            },
            Statement::Term(None),
        ];
        let mut write_set = HashSet::new();
        write_set.insert("bx0".to_string());
        let mut state_fields = HashSet::new();
        state_fields.insert("bx0".to_string());
        let (hoisted, remaining) = hoist_loop_invariants(&body, &write_set, &state_fields);
        assert_eq!(hoisted.len(), 0);
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn test_call_not_hoisted() {
        let body = vec![
            Statement::Let {
                names: vec![],
                name: "x".to_string(),
                ty: None,
                expr: Some(Expr::Call("PrintInt#".to_string(), vec![Expr::Decimal(1)], None)),
                modifiers: vec![],
            },
            Statement::Term(None),
        ];
        let write_set = HashSet::new();
        let state_fields = HashSet::new();
        let (hoisted, _remaining) = hoist_loop_invariants(&body, &write_set, &state_fields);
        assert_eq!(hoisted.len(), 0);
    }

    #[test]
    fn test_chained_invariant_lets() {
        let body = vec![
            Statement::Let {
                names: vec![],
                name: "a".to_string(),
                ty: None,
                expr: Some(Expr::Decimal(1)),
                modifiers: vec![],
            },
            Statement::Let {
                names: vec![],
                name: "b".to_string(),
                ty: None,
                expr: Some(Expr::BinaryOp(
                    BinaryOpKind::Add, Box::new(Expr::Identifier("a".to_string())),
                    Box::new(Expr::Decimal(2)),
                )),
                modifiers: vec![],
            },
            Statement::Let {
                names: vec![],
                name: "c".to_string(),
                ty: None,
                expr: Some(Expr::BinaryOp(
                    BinaryOpKind::Mul, Box::new(Expr::Identifier("b".to_string())),
                    Box::new(Expr::Decimal(3)),
                )),
                modifiers: vec![],
            },
            Statement::Term(None),
        ];
        let write_set = HashSet::new();
        let state_fields = HashSet::new();
        let (hoisted, remaining) = hoist_loop_invariants(&body, &write_set, &state_fields);
        assert_eq!(hoisted.len(), 3);
        assert_eq!(remaining.len(), 1);
    }
}
