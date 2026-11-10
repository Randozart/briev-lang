// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Closure free variables over HIR (mirror of
//! `backend::llvm::context::collect_free_vars`; first-appearance order,
//! deterministic env-slot layout). Scope rules match the AST walker exactly:
//! params and nested-lambda params bind; block/let names bind in order;
//! intrinsic names (`x#`) are excluded.

use super::expr::{HirExpr, HirExprKind};
use super::stmt::{HirStmt, HirStmtKind};
use std::collections::HashSet;

pub fn collect_free_vars(body: &HirExpr, params: &[String]) -> Vec<String> {
    let mut bound: HashSet<String> = params.iter().cloned().collect();
    let mut free: Vec<String> = Vec::new();
    collect_expr(body, &mut bound, &mut free);
    free
}

fn collect_expr(e: &HirExpr, bound: &mut HashSet<String>, free: &mut Vec<String>) {
    match &e.kind {
        HirExprKind::Identifier(n) => {
            if !bound.contains(n) && !n.ends_with('#') && !free.contains(n) {
                free.push(n.clone());
            }
        }
        HirExprKind::Lambda { params, body } => {
            let mut nested = bound.clone();
            for p in params {
                nested.insert(p.clone());
            }
            collect_expr(body, &mut nested, free);
        }
        HirExprKind::Block(stmts) => collect_stmts(stmts, &mut bound.clone(), free),
        HirExprKind::Binary { left, right, .. } => {
            collect_expr(left, bound, free);
            collect_expr(right, bound, free);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Consume(inner)
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Reflect { receiver: inner, .. } => collect_expr(inner, bound, free),
        HirExprKind::Call { args, .. }
        | HirExprKind::List(args)
        | HirExprKind::Tuple(args)
        | HirExprKind::Spawn { args, .. } => collect_exprs(args, bound, free),
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            if let Some(r) = receiver {
                collect_expr(r, bound, free);
            }
            collect_exprs(args, bound, free);
        }
        HirExprKind::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_expr(value, bound, free);
            }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            collect_expr(receiver, bound, free);
            collect_exprs(args, bound, free);
        }
        HirExprKind::Index(a, b) => {
            collect_expr(a, bound, free);
            collect_expr(b, bound, free);
        }
        HirExprKind::Slice { array, start, end, stride } => {
            collect_expr(array, bound, free);
            for part in [start, end, stride].into_iter().flatten() {
                collect_expr(part, bound, free);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            collect_expr(start, bound, free);
            collect_expr(end, bound, free);
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            collect_expr(condition, bound, free);
            collect_expr(then_branch, bound, free);
            if let Some(other) = else_branch {
                collect_expr(other, bound, free);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_expr(scrutinee, bound, free);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr(guard, bound, free);
                }
                collect_expr(&arm.body, bound, free);
            }
        }
        _ => {}
    }
}

fn collect_exprs(exprs: &[HirExpr], bound: &mut HashSet<String>, free: &mut Vec<String>) {
    for e in exprs {
        collect_expr(e, bound, free);
    }
}

fn collect_stmts(stmts: &[HirStmt], bound: &mut HashSet<String>, free: &mut Vec<String>) {
    for stmt in stmts {
        match &stmt.kind {
            HirStmtKind::Let { name, expr, .. } => {
                if let Some(e) = expr {
                    collect_expr(e, bound, free);
                }
                bound.insert(name.clone());
            }
            HirStmtKind::Assign(_, rhs) => collect_expr(rhs, bound, free),
            HirStmtKind::Term(Some(v))
            | HirStmtKind::EndProgram(Some(v))
            | HirStmtKind::Rollback(Some(v))
            | HirStmtKind::Tail(v)
            | HirStmtKind::Expression(v)
            | HirStmtKind::Gate(v)
            | HirStmtKind::Check(v) => collect_expr(v, bound, free),
            _ => {}
        }
    }
}
