// ── Phase F.1 — Mutation Operators ────────────────────────────────────
// 2026-07-28: Phase F.1 — AST mutation operators for MCMC search.
// Each mutation preserves the expression type and is structurally valid.
// Flat code: each function max 2 levels of nesting.

use crate::ast::{BinaryOpKind, Expr, UnaryOpKind};
use crate::derive::LcgRng;
use crate::derive::mcmc::MutationWeights;

/// Replace a random subtree with a different subtree of the same type.
/// 2026-07-28: Phase F.1 — subtree replacement.
pub fn mutate_replace_subtree(expr: &Expr, rng: &mut LcgRng, depth: u8) -> Expr {
    if depth == 0 || rng.gen_bool(0.3) {
        return generate_random_leaf(expr, rng);
    }
    match expr {
        Expr::BinaryOp(kind, lhs, rhs) => {
            if rng.gen_bool(0.5) {
                Expr::BinaryOp(
                    *kind,
                    Box::new(mutate_replace_subtree(lhs, rng, depth - 1)),
                    rhs.clone(),
                )
            } else {
                Expr::BinaryOp(
                    *kind,
                    lhs.clone(),
                    Box::new(mutate_replace_subtree(rhs, rng, depth - 1)),
                )
            }
        }
        Expr::UnaryOp(kind, inner) => Expr::UnaryOp(
            *kind,
            Box::new(mutate_replace_subtree(inner, rng, depth - 1)),
        ),
        Expr::If(cond, then_, else_) => Expr::If(
            Box::new(mutate_replace_subtree(cond, rng, depth - 1)),
            Box::new(mutate_replace_subtree(then_, rng, depth - 1)),
            else_
                .as_ref()
                .map(|e| Box::new(mutate_replace_subtree(e, rng, depth - 1))),
        ),
        _ => generate_random_leaf(expr, rng),
    }
}

/// Generate a random leaf expression compatible with the original expression.
fn generate_random_leaf(original: &Expr, rng: &mut LcgRng) -> Expr {
    match original {
        Expr::Decimal(_) | Expr::BinaryOp(_, _, _) if rng.gen_bool(0.5) => {
            Expr::Decimal(if rng.gen_bool(0.5) { 0 } else { 1 })
        }
        Expr::Float(_) => Expr::Float(if rng.gen_bool(0.5) { 0.0 } else { 1.0 }),
        Expr::Bool(_) => Expr::Bool(rng.gen_bool(0.5)),
        Expr::Identifier(name) => Expr::Identifier(name.clone()),
        _ => Expr::Decimal(0),
    }
}

/// Change a binary operator: x + y → x * y.
/// 2026-07-28: Phase F.1 — operator replacement.
pub fn mutate_change_operator(expr: &Expr, rng: &mut LcgRng) -> Expr {
    match expr {
        Expr::BinaryOp(_, lhs, rhs) => {
            let new_kind = random_binary_op(rng);
            Expr::BinaryOp(new_kind, lhs.clone(), rhs.clone())
        }
        Expr::UnaryOp(_, inner) => {
            let new_kind = random_unary_op(rng);
            Expr::UnaryOp(new_kind, inner.clone())
        }
        _ => expr.clone(),
    }
}

fn random_binary_op(rng: &mut LcgRng) -> BinaryOpKind {
    const OPS: &[BinaryOpKind] = &[
        BinaryOpKind::Add,
        BinaryOpKind::Sub,
        BinaryOpKind::Mul,
        BinaryOpKind::Div,
        BinaryOpKind::Eq,
        BinaryOpKind::Neq,
        BinaryOpKind::Lt,
        BinaryOpKind::Gt,
        BinaryOpKind::And,
        BinaryOpKind::Or,
    ];
    OPS[rng.gen_range(0, OPS.len())]
}

fn random_unary_op(rng: &mut LcgRng) -> UnaryOpKind {
    if rng.gen_bool(0.5) {
        UnaryOpKind::Neg
    } else {
        UnaryOpKind::Not
    }
}

/// Swap operands of commutative operations: x + y → y + x.
/// 2026-07-28: Phase F.1 — commutative swap.
pub fn mutate_swap_commutative(expr: &Expr, _rng: &mut LcgRng) -> Expr {
    match expr {
        Expr::BinaryOp(
            kind @ (BinaryOpKind::Add
            | BinaryOpKind::Mul
            | BinaryOpKind::And
            | BinaryOpKind::Or
            | BinaryOpKind::Eq
            | BinaryOpKind::Neq),
            lhs,
            rhs,
        ) => Expr::BinaryOp(*kind, rhs.clone(), lhs.clone()),
        _ => expr.clone(),
    }
}

/// Fold identity expressions: x + 0 → x, x * 1 → x.
/// 2026-07-28: Phase F.1 — constant folding.
pub fn mutate_fold_constant(expr: &Expr, _rng: &mut LcgRng) -> Expr {
    match expr {
        Expr::BinaryOp(BinaryOpKind::Add, lhs, rhs) if is_zero(rhs) => *lhs.clone(),
        Expr::BinaryOp(BinaryOpKind::Add, lhs, rhs) if is_zero(lhs) => *rhs.clone(),
        Expr::BinaryOp(BinaryOpKind::Sub, lhs, rhs) if is_zero(rhs) => *lhs.clone(),
        Expr::BinaryOp(BinaryOpKind::Mul, lhs, rhs) if is_one(rhs) => *lhs.clone(),
        Expr::BinaryOp(BinaryOpKind::Mul, lhs, rhs) if is_one(lhs) => *rhs.clone(),
        Expr::BinaryOp(BinaryOpKind::Mul, lhs, _) if is_zero(lhs) || is_zero_by_trait(lhs) => {
            Expr::Decimal(0)
        }
        Expr::BinaryOp(BinaryOpKind::Mul, _, rhs) if is_zero(rhs) || is_zero_by_trait(rhs) => {
            Expr::Decimal(0)
        }
        Expr::BinaryOp(BinaryOpKind::Div, lhs, rhs) if is_one(rhs) => *lhs.clone(),
        _ => expr.clone(),
    }
}

fn is_zero(expr: &Expr) -> bool {
    matches!(expr, Expr::Decimal(0))
}

fn is_one(expr: &Expr) -> bool {
    matches!(expr, Expr::Decimal(1))
}

fn is_zero_by_trait(expr: &Expr) -> bool {
    match expr {
        Expr::Decimal(0) => true,
        Expr::BinaryOp(BinaryOpKind::Sub, lhs, rhs) => lhs == rhs,
        Expr::BinaryOp(BinaryOpKind::Mul, lhs, rhs) => is_zero(lhs) || is_zero(rhs),
        _ => false,
    }
}

/// Insert an identity expression: x → x + 0, x → x * 1.
/// 2026-07-28: Phase F.1 — identity insertion.
pub fn mutate_insert_identity(expr: &Expr, rng: &mut LcgRng) -> Expr {
    match expr {
        Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Identifier(_) => {
            if rng.gen_bool(0.5) {
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(expr.clone()),
                    Box::new(expr_to_zero(expr)),
                )
            } else {
                Expr::BinaryOp(
                    BinaryOpKind::Mul,
                    Box::new(expr.clone()),
                    Box::new(expr_to_one(expr)),
                )
            }
        }
        _ => expr.clone(),
    }
}

fn expr_to_zero(expr: &Expr) -> Expr {
    match expr {
        Expr::Float(_) => Expr::Float(0.0),
        _ => Expr::Decimal(0),
    }
}

fn expr_to_one(expr: &Expr) -> Expr {
    match expr {
        Expr::Float(_) => Expr::Float(1.0),
        _ => Expr::Decimal(1),
    }
}

/// Delete a dead-code subtree (x + 0 → x, x * 1 → x, etc.).
/// 2026-07-28: Phase F.1 — dead code elimination.
pub fn mutate_delete_dead_code(expr: &Expr, rng: &mut LcgRng) -> Expr {
    let _ = rng;
    mutate_fold_constant(expr, &mut LcgRng::new(0))
}

/// Distribute multiplication over addition: a*(b+c) → a*b + a*c.
/// 2026-07-28: Phase F.1 — algebraic distribution.
pub fn mutate_distribute(expr: &Expr, _rng: &mut LcgRng) -> Expr {
    match expr {
        Expr::BinaryOp(BinaryOpKind::Mul, a, rhs) if is_add(rhs) => {
            if let Expr::BinaryOp(BinaryOpKind::Add, b, c) = rhs.as_ref() {
                return Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::BinaryOp(BinaryOpKind::Mul, a.clone(), b.clone())),
                    Box::new(Expr::BinaryOp(BinaryOpKind::Mul, a.clone(), c.clone())),
                );
            }
            expr.clone()
        }
        Expr::BinaryOp(BinaryOpKind::Mul, lhs, b) if is_add(lhs) => {
            if let Expr::BinaryOp(BinaryOpKind::Add, a, c) = lhs.as_ref() {
                return Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::BinaryOp(BinaryOpKind::Mul, a.clone(), b.clone())),
                    Box::new(Expr::BinaryOp(BinaryOpKind::Mul, c.clone(), b.clone())),
                );
            }
            expr.clone()
        }
        _ => expr.clone(),
    }
}

fn is_add(expr: &Expr) -> bool {
    matches!(expr, Expr::BinaryOp(BinaryOpKind::Add, _, _))
}

/// Fuse scalar ops into vector ops (placeholder, no-op for non-vector types).
pub fn mutate_vector_fuse(expr: &Expr, _rng: &mut LcgRng) -> Expr {
    expr.clone()
}

/// Apply a random mutation weighted by the mutation weights.
/// 2026-07-28: Phase F.1 — weighted random mutation dispatch.
pub fn apply_random_mutation(
    expr: &Expr,
    weights: &MutationWeights,
    rng: &mut LcgRng,
    depth: u8,
) -> Expr {
    let roll: f64 = rng.gen_f64();
    let mut cumulative = 0.0;

    cumulative += weights.replace_subtree;
    if roll < cumulative {
        return mutate_replace_subtree(expr, rng, depth);
    }
    cumulative += weights.change_operator;
    if roll < cumulative {
        return mutate_change_operator(expr, rng);
    }
    cumulative += weights.swap_commutative;
    if roll < cumulative {
        return mutate_swap_commutative(expr, rng);
    }
    cumulative += weights.fold_constant;
    if roll < cumulative {
        return mutate_fold_constant(expr, rng);
    }
    cumulative += weights.insert_identity;
    if roll < cumulative {
        return mutate_insert_identity(expr, rng);
    }
    cumulative += weights.delete_dead_code;
    if roll < cumulative {
        return mutate_delete_dead_code(expr, rng);
    }
    cumulative += weights.distribute;
    if roll < cumulative {
        return mutate_distribute(expr, rng);
    }
    cumulative += weights.vector_fuse;
    if roll < cumulative {
        return mutate_vector_fuse(expr, rng);
    }
    expr.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::LcgRng;

    fn make_rng() -> LcgRng {
        LcgRng::new(42)
    }

    #[test]
    fn test_mutate_change_operator_add_to_mul() {
        let expr = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Decimal(1)),
        );
        let mut rng = make_rng();
        let result = mutate_change_operator(&expr, &mut rng);
        match result {
            Expr::BinaryOp(kind, _, _) => assert_ne!(kind, BinaryOpKind::Add),
            _ => panic!("expected BinaryOp"),
        }
    }

    #[test]
    fn test_mutate_swap_commutative_add() {
        let expr = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Identifier("y".into())),
        );
        let mut rng = make_rng();
        let result = mutate_swap_commutative(&expr, &mut rng);
        match result {
            Expr::BinaryOp(BinaryOpKind::Add, lhs, rhs) => {
                assert_eq!(*lhs, Expr::Identifier("y".into()));
                assert_eq!(*rhs, Expr::Identifier("x".into()));
            }
            _ => panic!("expected swapped add"),
        }
    }

    #[test]
    fn test_mutate_fold_constant_add_zero() {
        let expr = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Decimal(0)),
        );
        let mut rng = make_rng();
        let result = mutate_fold_constant(&expr, &mut rng);
        assert_eq!(result, Expr::Identifier("x".into()));
    }

    #[test]
    fn test_mutate_insert_identity() {
        let expr = Expr::Identifier("x".into());
        let mut rng = make_rng();
        let result = mutate_insert_identity(&expr, &mut rng);
        match result {
            Expr::BinaryOp(_, _, _) => {}
            _ => panic!("expected identity wrapper"),
        }
    }

    #[test]
    fn test_mutate_distribute() {
        let expr = Expr::BinaryOp(
            BinaryOpKind::Mul,
            Box::new(Expr::Decimal(2)),
            Box::new(Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Decimal(3)),
            )),
        );
        let mut rng = make_rng();
        let result = mutate_distribute(&expr, &mut rng);
        assert_ne!(result, expr, "distribution should change the expression");
    }

    #[test]
    fn test_apply_random_mutation_all() {
        let expr = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Decimal(1)),
        );
        let weights = MutationWeights::default();
        let mut rng = make_rng();
        let types: Vec<String> = (0..50)
            .map(|_| format!("{:?}", apply_random_mutation(&expr, &weights, &mut rng, 3)))
            .collect();
        let distinct = types.iter().collect::<std::collections::HashSet<_>>().len();
        assert!(
            distinct >= 2,
            "should produce varied mutations, got {} unique",
            distinct
        );
    }
}
