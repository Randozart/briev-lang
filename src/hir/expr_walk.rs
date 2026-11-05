// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Shared HIR expression walker (mirrors `ast::visit::walk_stmts`): every
//! expression reachable from a statement tree is visited exactly once.

use super::expr::{HirExpr, HirExprKind};
use super::stmt::{HirStmt, HirStmtKind};

/// Visit every expression in a statement tree (mirrors AST `visit::walk_stmts`).
pub(crate) fn walk_stmts(stmts: &[HirStmt], f: &mut dyn FnMut(&HirExpr)) {
    for s in stmts {
        walk_stmt(s, f);
    }
}

fn walk_stmt(stmt: &HirStmt, f: &mut dyn FnMut(&HirExpr)) {
    let rec = |e: &HirExpr, f: &mut dyn FnMut(&HirExpr)| walk_expr(e, f);
    match &stmt.kind {
        HirStmtKind::Let { expr: Some(e), .. } => rec(e, f),
        HirStmtKind::Assign(l, r) => {
            rec(l, f);
            rec(r, f);
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                rec(t, f);
            }
            rec(value, f);
        }
        HirStmtKind::Term(Some(e))
        | HirStmtKind::EndProgram(Some(e))
        | HirStmtKind::Rollback(Some(e))
        | HirStmtKind::Tail(e)
        | HirStmtKind::Check(e)
        | HirStmtKind::Gate(e)
        | HirStmtKind::Expression(e) => rec(e, f),
        HirStmtKind::Guarded(c, b) => {
            rec(c, f);
            walk_stmts(b, f);
        }
        HirStmtKind::Block(b)
        | HirStmtKind::SyncBlock(b)
        | HirStmtKind::Defer(b)
        | HirStmtKind::Mutex(b)
        | HirStmtKind::Foreach { body: b, .. } => walk_stmts(b, f),
        HirStmtKind::Match { expr: e, arms } => {
            rec(e, f);
            for a in arms {
                walk_stmts(&a.body, f);
            }
        }
        HirStmtKind::TrgBinding { instance, .. } => rec(instance, f),
        HirStmtKind::Open(l, r) => {
            rec(l, f);
            rec(r, f);
        }
        _ => {}
    }
}

pub(crate) fn walk_expr(expr: &HirExpr, f: &mut dyn FnMut(&HirExpr)) {
    f(expr);
    match &expr.kind {
        HirExprKind::Call { args, .. } | HirExprKind::Tuple(args) | HirExprKind::List(args) => {
            for a in args {
                walk_expr(a, f);
            }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            walk_expr(receiver, f);
            for a in args {
                walk_expr(a, f);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            walk_expr(left, f);
            walk_expr(right, f);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Reflect { receiver: inner, .. }
        | HirExprKind::Lambda { body: inner, .. } => walk_expr(inner, f),
        HirExprKind::Index(a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        HirExprKind::Slice { array, start, end, stride } => {
            walk_expr(array, f);
            if let Some(s) = start {
                walk_expr(s, f);
            }
            if let Some(e) = end {
                walk_expr(e, f);
            }
            if let Some(s) = stride {
                walk_expr(s, f);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            walk_expr(start, f);
            walk_expr(end, f);
        }
        HirExprKind::Spawn { args, .. } => {
            for a in args {
                walk_expr(a, f);
            }
        }
        HirExprKind::Block(stmts) => walk_stmts(stmts, f),
        HirExprKind::If { condition, then_branch, else_branch } => {
            walk_expr(condition, f);
            walk_expr(then_branch, f);
            if let Some(e) = else_branch {
                walk_expr(e, f);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, f);
            for a in arms {
                if let Some(guard) = &a.guard {
                    walk_expr(guard, f);
                }
                walk_expr(&a.body, f);
            }
        }
        HirExprKind::StructLiteral { fields, .. } => {
            for (_, e) in fields {
                walk_expr(e, f);
            }
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            for a in args {
                walk_expr(a, f);
            }
            if let Some(r) = receiver {
                walk_expr(r, f);
            }
        }
        _ => {}
    }
}
