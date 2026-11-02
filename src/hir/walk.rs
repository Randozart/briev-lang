// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Postorder mutable HIR traversal. Replacements are not revisited; callbacks
//! must produce already-normalized nodes.

use super::expr::{HirExpr, HirExprKind};
use super::pattern::{HirPattern, HirPatternKind};
use super::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};

pub fn walk_expr_mut<F: FnMut(&mut HirExpr)>(expr: &mut HirExpr, f: &mut F) {
    match &mut expr.kind {
        HirExprKind::Call { args, .. } => walk_exprs_mut(args, f),
        HirExprKind::Binary { left, right, .. } => {
            walk_expr_mut(left, f);
            walk_expr_mut(right, f);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Lambda { body: inner, .. } => walk_expr_mut(inner, f),
        HirExprKind::Field { receiver, .. }
        | HirExprKind::Reflect { receiver, .. } => walk_expr_mut(receiver, f),
        HirExprKind::MethodCall { receiver, args, .. } => {
            walk_expr_mut(receiver, f);
            walk_exprs_mut(args, f);
        }
        HirExprKind::Index(left, right) => {
            walk_expr_mut(left, f);
            walk_expr_mut(right, f);
        }
        HirExprKind::Slice { array, start, end, stride } => {
            walk_expr_mut(array, f);
            walk_opt_box_mut(start.as_deref_mut(), f);
            walk_opt_box_mut(end.as_deref_mut(), f);
            walk_opt_box_mut(stride.as_deref_mut(), f);
        }
        HirExprKind::Range { start, end, .. } => {
            walk_expr_mut(start, f);
            walk_expr_mut(end, f);
        }
        HirExprKind::Spawn { args, .. } => walk_exprs_mut(args, f),
        HirExprKind::If { condition, then_branch, else_branch } => {
            walk_expr_mut(condition, f);
            walk_expr_mut(then_branch, f);
            walk_opt_box_mut(else_branch.as_deref_mut(), f);
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => walk_exprs_mut(items, f),
        HirExprKind::StructLiteral { fields, .. } => {
            for (_, value) in fields.iter_mut() {
                walk_expr_mut(value, f);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            walk_expr_mut(scrutinee, f);
            for arm in arms {
                walk_pattern_mut(&mut arm.pattern, f);
                walk_expr_mut(&mut arm.body, f);
            }
        }
        HirExprKind::Block(body) => walk_body_mut(body, f),
        HirExprKind::Literal(_)
        | HirExprKind::Identifier(_)
        | HirExprKind::UnitLiteral { .. }
        | HirExprKind::Other(_) => {}
    }
    f(expr);
}

pub fn walk_body_mut<F: FnMut(&mut HirExpr)>(body: &mut [HirStmt], f: &mut F) {
    for stmt in body {
        walk_stmt_mut(stmt, f);
    }
}

pub fn walk_stmt_mut<F: FnMut(&mut HirExpr)>(stmt: &mut HirStmt, f: &mut F) {
    match &mut stmt.kind {
        HirStmtKind::Let { expr, .. } => walk_opt_mut(expr.as_mut(), f),
        HirStmtKind::Assign(left, right) => {
            walk_expr_mut(left, f);
            walk_expr_mut(right, f);
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(target) = target.as_mut() {
                walk_expr_mut(target, f);
            }
            walk_expr_mut(value, f);
        }
        HirStmtKind::Term(expr) => walk_opt_mut(expr.as_mut(), f),
        HirStmtKind::Tail(expr)
        | HirStmtKind::Check(expr)
        | HirStmtKind::Expression(expr)
        | HirStmtKind::Gate(expr) => walk_expr_mut(expr, f),
        HirStmtKind::Guarded(cond, body) => {
            walk_expr_mut(cond, f);
            walk_body_mut(body, f);
        }
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => walk_body_mut(body, f),
        HirStmtKind::Foreach { list, body, .. } => {
            walk_expr_mut(list, f);
            walk_body_mut(body, f);
        }
        HirStmtKind::Match { expr, arms } => {
            walk_expr_mut(expr, f);
            walk_stmt_arms_mut(arms, f);
        }
        HirStmtKind::EndProgram(expr) | HirStmtKind::Rollback(expr) => {
            walk_opt_mut(expr.as_mut(), f)
        }
        HirStmtKind::TrgBinding { instance, .. } => walk_expr_mut(instance, f),
        HirStmtKind::Open(left, right) => {
            walk_expr_mut(left, f);
            walk_expr_mut(right, f);
        }
        HirStmtKind::Break
        | HirStmtKind::Trap
        | HirStmtKind::Halt
        | HirStmtKind::Yield
        | HirStmtKind::FreeHint(_)
        | HirStmtKind::KeepHint(_)
        | HirStmtKind::MetadataAssignment(_, _)
        | HirStmtKind::InlineAsm { .. }
        | HirStmtKind::Other(_) => {}
    }
}

fn walk_stmt_arms_mut<F: FnMut(&mut HirExpr)>(arms: &mut [HirStmtMatchArm], f: &mut F) {
    for arm in arms {
        walk_patterns_mut(&mut arm.patterns, f);
        walk_body_mut(&mut arm.body, f);
    }
}

fn walk_patterns_mut<F: FnMut(&mut HirExpr)>(patterns: &mut [HirPattern], f: &mut F) {
    for pattern in patterns {
        walk_pattern_mut(pattern, f);
    }
}

pub fn walk_pattern_mut<F: FnMut(&mut HirExpr)>(pattern: &mut HirPattern, f: &mut F) {
    match &mut pattern.kind {
        HirPatternKind::Literal(expr) => walk_expr_mut(expr, f),
        HirPatternKind::Range(start, end) | HirPatternKind::RangeInclusive(start, end) => {
            walk_expr_mut(start, f);
            walk_expr_mut(end, f);
        }
        HirPatternKind::EnumVariant(_, pats)
        | HirPatternKind::Tuple(pats)
        | HirPatternKind::Multi(pats) => {
            for pat in pats {
                walk_pattern_mut(pat, f);
            }
        }
        HirPatternKind::Wildcard | HirPatternKind::Binding(_) | HirPatternKind::TypedBinding(_, _) => {}
    }
}

fn walk_exprs_mut<F: FnMut(&mut HirExpr)>(exprs: &mut [HirExpr], f: &mut F) {
    for expr in exprs {
        walk_expr_mut(expr, f);
    }
}

fn walk_opt_mut<F: FnMut(&mut HirExpr)>(expr: Option<&mut HirExpr>, f: &mut F) {
    if let Some(expr) = expr {
        walk_expr_mut(expr, f);
    }
}

fn walk_opt_box_mut<F: FnMut(&mut HirExpr)>(expr: Option<&mut HirExpr>, f: &mut F) {
    if let Some(expr) = expr {
        walk_expr_mut(expr, f);
    }
}
