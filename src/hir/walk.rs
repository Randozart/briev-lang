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
        HirExprKind::Wildcard => {}
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
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            for a in args.iter_mut() {
                walk_expr_mut(a, f);
            }
            if let Some(r) = receiver {
                walk_expr_mut(r, f);
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
        HirStmtKind::TempTransfer { expr, .. } => walk_expr_mut(expr, f),
        HirStmtKind::ReleaseOwned { .. } => {}
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

pub fn walk_expr<F: FnMut(&HirExpr)>(expr: &HirExpr, f: &mut F) {
    match &expr.kind {
        HirExprKind::Call { args, .. } => walk_exprs(args, f),
        HirExprKind::Binary { left, right, .. } => {
            walk_expr(left, f);
            walk_expr(right, f);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Lambda { body: inner, .. } => walk_expr(inner, f),
        HirExprKind::Field { receiver, .. }
        | HirExprKind::Reflect { receiver, .. } => walk_expr(receiver, f),
        HirExprKind::MethodCall { receiver, args, .. } => {
            walk_expr(receiver, f);
            walk_exprs(args, f);
        }
        HirExprKind::Index(left, right) => {
            walk_expr(left, f);
            walk_expr(right, f);
        }
        HirExprKind::Wildcard => {}
        HirExprKind::Slice { array, start, end, stride } => {
            walk_expr(array, f);
            walk_opt_box(start.as_deref(), f);
            walk_opt_box(end.as_deref(), f);
            walk_opt_box(stride.as_deref(), f);
        }
        HirExprKind::Range { start, end, .. } => {
            walk_expr(start, f);
            walk_expr(end, f);
        }
        HirExprKind::Spawn { args, .. } => walk_exprs(args, f),
        HirExprKind::If { condition, then_branch, else_branch } => {
            walk_expr(condition, f);
            walk_expr(then_branch, f);
            walk_opt_box(else_branch.as_deref(), f);
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => walk_exprs(items, f),
        HirExprKind::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                walk_expr(value, f);
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
        HirExprKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, f);
            for arm in arms {
                walk_expr(&arm.body, f);
            }
        }
        HirExprKind::Block(body) => walk_body(body, f),
        HirExprKind::Literal(_)
        | HirExprKind::Identifier(_)
        | HirExprKind::UnitLiteral { .. }
        | HirExprKind::Other(_) => {}
    }
    f(expr);
}

pub fn walk_body<F: FnMut(&HirExpr)>(body: &[HirStmt], f: &mut F) {
    for stmt in body {
        walk_stmt(stmt, f);
    }
}

pub fn walk_stmt<F: FnMut(&HirExpr)>(stmt: &HirStmt, f: &mut F) {
    match &stmt.kind {
        HirStmtKind::Let { expr, .. } => walk_opt(expr.as_ref(), f),
        HirStmtKind::Assign(left, right) => {
            walk_expr(left, f);
            walk_expr(right, f);
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            walk_opt(target.as_ref(), f);
            walk_expr(value, f);
        }
        HirStmtKind::Term(expr) => walk_opt(expr.as_ref(), f),
        HirStmtKind::TempTransfer { expr, .. } => walk_expr(expr, f),
        HirStmtKind::ReleaseOwned { .. } => {}
        HirStmtKind::Tail(expr)
        | HirStmtKind::Check(expr)
        | HirStmtKind::Expression(expr)
        | HirStmtKind::Gate(expr) => walk_expr(expr, f),
        HirStmtKind::Guarded(cond, body) => {
            walk_expr(cond, f);
            walk_body(body, f);
        }
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => walk_body(body, f),
        HirStmtKind::Foreach { list, body, .. } => {
            walk_expr(list, f);
            walk_body(body, f);
        }
        HirStmtKind::Match { expr, arms } => {
            walk_expr(expr, f);
            for arm in arms {
                walk_body(&arm.body, f);
            }
        }
        HirStmtKind::EndProgram(expr) | HirStmtKind::Rollback(expr) => {
            walk_opt(expr.as_ref(), f)
        }
        HirStmtKind::TrgBinding { instance, .. } => walk_expr(instance, f),
        HirStmtKind::Open(left, right) => {
            walk_expr(left, f);
            walk_expr(right, f);
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

fn walk_exprs<F: FnMut(&HirExpr)>(exprs: &[HirExpr], f: &mut F) {
    for expr in exprs {
        walk_expr(expr, f);
    }
}

fn walk_opt<F: FnMut(&HirExpr)>(expr: Option<&HirExpr>, f: &mut F) {
    if let Some(expr) = expr {
        walk_expr(expr, f);
    }
}

fn walk_opt_box<F: FnMut(&HirExpr)>(expr: Option<&HirExpr>, f: &mut F) {
    if let Some(expr) = expr {
        walk_expr(expr, f);
    }
}

pub fn walk_pattern<F: FnMut(&HirPattern)>(pattern: &HirPattern, f: &mut F) {
    match &pattern.kind {
        HirPatternKind::EnumVariant(_, patterns)
        | HirPatternKind::Tuple(patterns)
        | HirPatternKind::Multi(patterns) => walk_patterns(patterns, f),
        HirPatternKind::Wildcard
        | HirPatternKind::Literal(_)
        | HirPatternKind::Binding(_)
        | HirPatternKind::TypedBinding(_, _)
        | HirPatternKind::Range(_, _)
        | HirPatternKind::RangeInclusive(_, _) => {}
    }
    f(pattern);
}

pub fn walk_patterns<F: FnMut(&HirPattern)>(patterns: &[HirPattern], f: &mut F) {
    for pattern in patterns {
        walk_pattern(pattern, f);
    }
}
