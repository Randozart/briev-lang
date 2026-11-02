// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H0c unsupported-syntax accounting. `Other(tag)` nodes are explicit gaps to
//! close before a consumer can rely on HIR.

use super::expr::{HirExpr, HirExprKind};
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{HirCallable, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnsupportedCounts {
    pub expressions: usize,
    pub statements: usize,
}

impl UnsupportedCounts {
    pub fn total(self) -> usize {
        self.expressions + self.statements
    }
}

pub fn unsupported_counts(program: &HirProgram) -> UnsupportedCounts {
    let mut counts = UnsupportedCounts::default();
    for callable in &program.callables {
        count_body(&callable.body.statements, &mut counts);
    }
    counts
}

pub fn unsupported_for_callable(callable: &HirCallable) -> UnsupportedCounts {
    let mut counts = UnsupportedCounts::default();
    count_body(&callable.body.statements, &mut counts);
    counts
}

fn count_body(body: &[HirStmt], counts: &mut UnsupportedCounts) {
    for stmt in body {
        count_stmt(stmt, counts);
    }
}

fn count_stmt(stmt: &HirStmt, counts: &mut UnsupportedCounts) {
    match &stmt.kind {
        HirStmtKind::Let { expr, .. } => count_opt_expr(expr.as_ref(), counts),
        HirStmtKind::Assign(left, right) => count_exprs([left, right], counts),
        HirStmtKind::ArrowAssign { target, value, .. } => {
            count_opt_expr(target.as_ref(), counts);
            count_expr(value, counts);
        }
        HirStmtKind::Term(expr) => count_opt_expr(expr.as_ref(), counts),
        HirStmtKind::Tail(expr) | HirStmtKind::Check(expr) | HirStmtKind::Expression(expr) => {
            count_expr(expr, counts)
        }
        HirStmtKind::Guarded(cond, body) => {
            count_expr(cond, counts);
            count_body(body, counts);
        }
        HirStmtKind::Block(body) => count_body(body, counts),
        HirStmtKind::Foreach { list, body, .. } => {
            count_expr(list, counts);
            count_body(body, counts);
        }
        HirStmtKind::Match { expr, arms } => {
            count_expr(expr, counts);
            count_stmt_arms(arms, counts);
        }
        HirStmtKind::Other(_) => counts.statements += 1,
    }
}

fn count_stmt_arms(arms: &[super::stmt::HirStmtMatchArm], counts: &mut UnsupportedCounts) {
    for arm in arms {
        count_patterns(&arm.patterns, counts);
        count_body(&arm.body, counts);
    }
}

fn count_expr_arms(arms: &[super::expr::HirMatchArm], counts: &mut UnsupportedCounts) {
    for arm in arms {
        count_pattern(&arm.pattern, counts);
        count_expr(&arm.body, counts);
    }
}

fn count_patterns(patterns: &[HirPattern], counts: &mut UnsupportedCounts) {
    for pattern in patterns {
        count_pattern(pattern, counts);
    }
}

fn count_opt_expr(expr: Option<&HirExpr>, counts: &mut UnsupportedCounts) {
    if let Some(expr) = expr {
        count_expr(expr, counts);
    }
}

fn count_exprs<'a>(
    exprs: impl IntoIterator<Item = &'a HirExpr>,
    counts: &mut UnsupportedCounts,
) {
    for expr in exprs {
        count_expr(expr, counts);
    }
}

fn count_expr(expr: &HirExpr, counts: &mut UnsupportedCounts) {
    match &expr.kind {
        HirExprKind::Call { args, .. } => count_exprs(args, counts),
        HirExprKind::Binary { left, right, .. } => count_exprs([left.as_ref(), right.as_ref()], counts),
        HirExprKind::Unary { inner, .. } | HirExprKind::Cast { inner, .. } => count_expr(inner, counts),
        HirExprKind::Field { receiver, .. } => count_expr(receiver, counts),
        HirExprKind::Tuple(items) | HirExprKind::List(items) => count_exprs(items, counts),
        HirExprKind::StructLiteral { fields, .. } => {
            count_exprs(fields.iter().map(|(_, expr)| expr), counts)
        }
        HirExprKind::Match { scrutinee, arms } => {
            count_expr(scrutinee, counts);
            count_expr_arms(arms, counts);
        }
        HirExprKind::Block(body) => count_body(body, counts),
        HirExprKind::Other(_) => counts.expressions += 1,
        HirExprKind::Literal(_) | HirExprKind::Identifier(_) => {}
    }
}

fn count_pattern(pattern: &HirPattern, counts: &mut UnsupportedCounts) {
    match &pattern.kind {
        HirPatternKind::Literal(expr)
        | HirPatternKind::Range(expr, _)
        | HirPatternKind::RangeInclusive(expr, _) => count_expr(expr, counts),
        HirPatternKind::Range(_, second) | HirPatternKind::RangeInclusive(_, second) => {
            count_expr(second, counts);
        }
        HirPatternKind::EnumVariant(_, pats) | HirPatternKind::Tuple(pats) | HirPatternKind::Multi(pats) => {
            for pat in pats {
                count_pattern(pat, counts);
            }
        }
        HirPatternKind::Wildcard | HirPatternKind::Binding(_) | HirPatternKind::TypedBinding(_, _) => {}
    }
}
