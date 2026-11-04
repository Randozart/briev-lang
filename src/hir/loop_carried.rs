// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b — loop-carried / minimal-state classification over HIR.
//! Mirror of `analysis::loop_carried`, reusing its `FieldClass`.

use super::expr::{HirExpr, HirExprKind};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::loop_carried::FieldClass;
use std::collections::{HashMap, HashSet};

pub fn classify_fields(
    write_set: &HashSet<String>,
    body: &[HirStmt],
    contract_exprs: &[&HirExpr],
    observable_bodies: &[&[HirStmt]],
) -> HashMap<String, FieldClass> {
    let mut out = HashMap::new();
    for f in write_set {
        out.insert(f.clone(), classify_field(f, body, contract_exprs, observable_bodies));
    }
    out
}

fn classify_field(
    field: &str,
    body: &[HirStmt],
    contract_exprs: &[&HirExpr],
    observable_bodies: &[&[HirStmt]],
) -> FieldClass {
    if !body.iter().any(|s| statement_writes(s, field)) {
        return FieldClass::LoopInvariant;
    }
    let read_in_body = body.iter().any(|s| statement_reads(s, field));
    let read_in_contract = contract_exprs.iter().any(|e| expr_reads(e, field));
    let read_observable = observable_bodies
        .iter()
        .any(|b| b.iter().any(|s| statement_reads(s, field)));
    if read_in_body || read_in_contract || read_observable {
        FieldClass::LoopCarried
    } else {
        FieldClass::Dead
    }
}

fn statement_writes(stmt: &HirStmt, field: &str) -> bool {
    match &stmt.kind {
        HirStmtKind::Assign(HirExpr { kind: HirExprKind::Identifier(n), .. }, _) => n == field,
        HirStmtKind::Guarded(_, body) | HirStmtKind::Block(body) => {
            body.iter().any(|s| statement_writes(s, field))
        }
        _ => false,
    }
}

fn statement_reads(stmt: &HirStmt, field: &str) -> bool {
    match &stmt.kind {
        HirStmtKind::Assign(lhs, rhs) => {
            expr_reads(rhs, field)
                || match &lhs.kind {
                    HirExprKind::Identifier(n) => n != field && expr_reads(lhs, field),
                    _ => expr_reads(lhs, field),
                }
        }
        HirStmtKind::Let { expr, .. } => expr.as_ref().map_or(false, |e| expr_reads(e, field)),
        HirStmtKind::Term(Some(e)) | HirStmtKind::EndProgram(Some(e)) | HirStmtKind::Tail(e) => {
            expr_reads(e, field)
        }
        HirStmtKind::Expression(e) => expr_reads(e, field),
        HirStmtKind::Guarded(cond, body) => {
            expr_reads(cond, field) || body.iter().any(|s| statement_reads(s, field))
        }
        HirStmtKind::Gate(e) | HirStmtKind::Check(e) => expr_reads(e, field),
        HirStmtKind::Block(b) => b.iter().any(|s| statement_reads(s, field)),
        _ => false,
    }
}

fn expr_reads(expr: &HirExpr, field: &str) -> bool {
    match &expr.kind {
        HirExprKind::Identifier(n) => n == field,
        HirExprKind::Binary { left, right, .. } => {
            expr_reads(left, field) || expr_reads(right, field)
        }
        HirExprKind::Unary { inner, .. } => expr_reads(inner, field),
        HirExprKind::Call { args, .. } => args.iter().any(|a| expr_reads(a, field)),
        HirExprKind::Field { receiver, .. } => expr_reads(receiver, field),
        HirExprKind::Index(arr, idx) => expr_reads(arr, field) || expr_reads(idx, field),
        HirExprKind::Cast { inner, .. } => expr_reads(inner, field),
        HirExprKind::List(items) | HirExprKind::Tuple(items) => {
            items.iter().any(|i| expr_reads(i, field))
        }
        HirExprKind::Block(stmts) => stmts.iter().any(|s| statement_reads(s, field)),
        HirExprKind::If { condition, then_branch, else_branch } => {
            expr_reads(condition, field)
                || expr_reads(then_branch, field)
                || else_branch.as_ref().map_or(false, |e| expr_reads(e, field))
        }
        HirExprKind::Deref(inner) => expr_reads(inner, field),
        _ => false,
    }
}
