// ── Loop-Carried / Minimal-State Classification ────────────────
//
// 2026-07-31: Classify each state field by its use-def position relative
// to the loop, to emit the MINIMAL loop-carried state set. LLVM vectorizes
// a loop only when it provably has no cross-iteration dependencies; %State
// memory traffic in the body obscures that proof. Fields that are never
// written in the loop are hoisted (not phis); fields that are loop-carried
// become phis; fields that are written but never read are dropped.
//
// See docs/architecture/minimal-state-and-purity.md.

use crate::ast::{Expr, Statement};
use std::collections::{HashMap, HashSet};

/// How a state field behaves relative to the hot loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldClass {
    /// Never written in the loop — hoist to a preheader register (load once).
    LoopInvariant,
    /// Written in the loop and read in a later iteration, or by a convergence
    /// contract / observable side effect — a phi node carries it.
    LoopCarried,
    /// Written in the loop but never read anywhere — eliminate.
    Dead,
}

/// Classify each field in `write_set` against the loop body and contract.
///
/// `contract_exprs` contains the `[pre]`/`[post]` condition expressions —
/// fields read by them must survive (LoopCarried).
/// `observable_bodies` contains guard bodies and post-loop hoisted statements
/// whose side effects read fields — those fields must survive.
pub fn classify_fields(
    write_set: &HashSet<String>,
    body: &[Statement],
    contract_exprs: &[&Expr],
    observable_bodies: &[&[Statement]],
) -> HashMap<String, FieldClass> {
    let mut out: HashMap<String, FieldClass> = HashMap::new();
    for f in write_set {
        out.insert(f.clone(), classify_field(f, body, contract_exprs, observable_bodies));
    }
    out
}

fn classify_field(
    field: &str,
    body: &[Statement],
    contract_exprs: &[&Expr],
    observable_bodies: &[&[Statement]],
) -> FieldClass {
    let written = body.iter().any(|s| statement_writes(s, field));
    if !written {
        return FieldClass::LoopInvariant;
    }
    // Loop-carried if read in the body (may be a later iteration), read by a
    // contract expression, or read by an observable body.
    let read_in_body = body.iter().any(|s| statement_reads(s, field));
    let read_in_contract = contract_exprs.iter().any(|e| expr_reads(e, field));
    let read_observable = observable_bodies.iter().any(|b| b.iter().any(|s| statement_reads(s, field)));
    if read_in_body || read_in_contract || read_observable {
        FieldClass::LoopCarried
    } else {
        FieldClass::Dead
    }
}

fn statement_writes(stmt: &Statement, field: &str) -> bool {
    match stmt {
        Statement::Assign(Expr::Identifier(n), _) => n == field,
        Statement::Guarded(_, body) => body.iter().any(|s| statement_writes(s, field)),
        Statement::Block(b) => b.iter().any(|s| statement_writes(s, field)),
        _ => false,
    }
}

fn statement_reads(stmt: &Statement, field: &str) -> bool {
    match stmt {
        Statement::Assign(lhs, rhs) => {
            expr_reads(rhs, field) || match lhs {
                Expr::Identifier(n) => n != field && expr_reads(lhs, field),
                _ => expr_reads(lhs, field),
            }
        }
        Statement::Let { expr, .. } => expr.as_ref().map_or(false, |e| expr_reads(e, field)),
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Tail(e) => expr_reads(e, field),
        Statement::Expression(e) => expr_reads(e, field),
        Statement::Guarded(cond, body) => {
            expr_reads(cond, field) || body.iter().any(|s| statement_reads(s, field))
        }
        Statement::Gate(e) => expr_reads(e, field),
        Statement::Block(b) => b.iter().any(|s| statement_reads(s, field)),
        _ => false,
    }
}

fn expr_reads(expr: &Expr, field: &str) -> bool {
    match expr {
        Expr::Identifier(n) => n == field,
        Expr::BinaryOp(_, l, r) => expr_reads(l, field) || expr_reads(r, field),
        Expr::UnaryOp(_, e) => expr_reads(e, field),
        Expr::Call(_, args, _) => args.iter().any(|a| expr_reads(a, field)),
        Expr::Field(obj, _) => expr_reads(obj, field),
        Expr::Index(arr, idx) => expr_reads(arr, field) || expr_reads(idx, field),
        Expr::Cast(e, _) => expr_reads(e, field),
        Expr::List(items) => items.iter().any(|i| expr_reads(i, field)),
        Expr::Tuple(items) => items.iter().any(|i| expr_reads(i, field)),
        Expr::Block(stmts) => stmts.iter().any(|s| statement_reads(s, field)),
        Expr::If(c, t, e) => expr_reads(c, field) || expr_reads(t, field) || e.as_ref().map_or(false, |e| expr_reads(e, field)),
        Expr::Deref(inner) => expr_reads(inner, field),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loop_invariant_not_written() {
        // `bound` is read but never written in the body → hoist.
        let mut ws = HashSet::new();
        ws.insert("bound".to_string());
        ws.insert("count".to_string());
        let body = vec![Statement::Assign(
            Expr::Identifier("count".to_string()),
            Expr::BinaryOp(
                crate::ast::BinaryOpKind::Add,
                Box::new(Expr::Identifier("count".to_string())),
                Box::new(Expr::Identifier("bound".to_string())),
            ),
        )];
        let classes = classify_fields(&ws, &body, &[], &[]);
        assert_eq!(classes["bound"], FieldClass::LoopInvariant);
        assert_eq!(classes["count"], FieldClass::LoopCarried);
    }

    #[test]
    fn test_dead_field_dropped() {
        // `tmp` is written but never read → Dead.
        let mut ws = HashSet::new();
        ws.insert("tmp".to_string());
        let body = vec![Statement::Assign(
            Expr::Identifier("tmp".to_string()),
            Expr::Decimal(5),
        )];
        let classes = classify_fields(&ws, &body, &[], &[]);
        assert_eq!(classes["tmp"], FieldClass::Dead);
    }

    #[test]
    fn test_contract_read_of_invariant_is_invariant() {
        // `total` is not written in the body — its value never changes, so it
        // is hoisted (loaded once), even though the contract reads it.
        let mut ws = HashSet::new();
        ws.insert("total".to_string());
        let body: Vec<Statement> = vec![];
        let contract: Expr = Expr::Identifier("total".to_string());
        let classes = classify_fields(&ws, &body, &[&contract], &[]);
        assert_eq!(classes["total"], FieldClass::LoopInvariant);
    }

    #[test]
    fn test_observable_read_makes_carried() {
        // `acc` written in body, read by an observable (post-loop print).
        let mut ws = HashSet::new();
        ws.insert("acc".to_string());
        let body = vec![Statement::Assign(
            Expr::Identifier("acc".to_string()),
            Expr::BinaryOp(
                crate::ast::BinaryOpKind::Add,
                Box::new(Expr::Identifier("acc".to_string())),
                Box::new(Expr::Decimal(1)),
            ),
        )];
        let observable = vec![Statement::Term(Some(Expr::Identifier("acc".to_string())))];
        let classes = classify_fields(&ws, &body, &[], &[&observable]);
        assert_eq!(classes["acc"], FieldClass::LoopCarried);
    }
}
