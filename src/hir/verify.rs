// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! HIR invariants. H0c verifies every nested statement/expression/pattern ID,
//! not only callable/body headers.

use super::expr::{HirExpr, HirExprKind, HirMatchArm};
use super::ids::{DefId, ItemId, NodeId};
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{HirCallable, HirProgram};
use super::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};
use std::collections::BTreeSet;

pub fn verify(program: &HirProgram) -> Result<(), String> {
    let mut defs = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    let mut items = BTreeSet::new();
    for callable in &program.callables {
        verify_callable(callable, &mut defs, &mut nodes, &mut items)?;
        verify_body(&callable.body.statements, &mut nodes)?;
    }
    Ok(())
}

fn verify_callable(
    callable: &HirCallable,
    defs: &mut BTreeSet<DefId>,
    nodes: &mut BTreeSet<NodeId>,
    items: &mut BTreeSet<ItemId>,
) -> Result<(), String> {
    if callable.name.is_empty() {
        return Err(format!("callable {:?} has an empty name", callable.id));
    }
    verify_unique_ids(callable, defs, nodes, items)?;
    verify_export_metadata(callable)
}

fn verify_unique_ids(
    callable: &HirCallable,
    defs: &mut BTreeSet<DefId>,
    nodes: &mut BTreeSet<NodeId>,
    items: &mut BTreeSet<ItemId>,
) -> Result<(), String> {
    if !defs.insert(callable.id) {
        return Err(format!("duplicate DefId {:?}", callable.id));
    }
    if !items.insert(callable.item) {
        return Err(format!("duplicate ItemId {:?}", callable.item));
    }
    verify_node(nodes, callable.node, &callable.name)?;
    verify_node(nodes, callable.body.node, &callable.name)
}

fn verify_export_metadata(callable: &HirCallable) -> Result<(), String> {
    match (callable.exported, callable.export_symbol.as_deref()) {
        (true, Some(symbol)) if !symbol.is_empty() => Ok(()),
        (true, _) => Err(format!("exported callable '{}' has no symbol", callable.name)),
        (false, Some(_)) => Err(format!(
            "non-exported callable '{}' carries an export symbol",
            callable.name
        )),
        (false, None) => Ok(()),
    }
}

fn verify_body(body: &[HirStmt], nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    for stmt in body {
        verify_stmt(stmt, nodes)?;
    }
    Ok(())
}

fn verify_stmt(stmt: &HirStmt, nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    verify_node(nodes, stmt.id, "statement")?;
    match &stmt.kind {
        HirStmtKind::Let { expr, .. } => verify_opt_expr(expr.as_ref(), nodes),
        HirStmtKind::Assign(left, right) => verify_exprs([left, right], nodes),
        HirStmtKind::ArrowAssign { target, value, .. } => {
            verify_opt_expr(target.as_ref(), nodes)?;
            verify_expr(value, nodes)
        }
        HirStmtKind::Term(expr) => verify_opt_expr(expr.as_ref(), nodes),
        HirStmtKind::Tail(expr) | HirStmtKind::Check(expr) | HirStmtKind::Expression(expr) => {
            verify_expr(expr, nodes)
        }
        HirStmtKind::Guarded(cond, body) => {
            verify_expr(cond, nodes)?;
            verify_body(body, nodes)
        }
        HirStmtKind::Block(body) => verify_body(body, nodes),
        HirStmtKind::Foreach { list, body, .. } => {
            verify_expr(list, nodes)?;
            verify_body(body, nodes)
        }
        HirStmtKind::Match { expr, arms } => {
            verify_expr(expr, nodes)?;
            verify_stmt_arms(arms, nodes)
        }
        HirStmtKind::Other(_) => Ok(()),
    }
}

fn verify_stmt_arms(
    arms: &[HirStmtMatchArm],
    nodes: &mut BTreeSet<NodeId>,
) -> Result<(), String> {
    for arm in arms {
        verify_patterns(&arm.patterns, nodes)?;
        verify_body(&arm.body, nodes)?;
    }
    Ok(())
}

fn verify_opt_expr(expr: Option<&HirExpr>, nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    match expr {
        Some(expr) => verify_expr(expr, nodes),
        None => Ok(()),
    }
}

fn verify_exprs<'a>(
    exprs: impl IntoIterator<Item = &'a HirExpr>,
    nodes: &mut BTreeSet<NodeId>,
) -> Result<(), String> {
    for expr in exprs {
        verify_expr(expr, nodes)?;
    }
    Ok(())
}

fn verify_expr(expr: &HirExpr, nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    verify_node(nodes, expr.id, "expression")?;
    match &expr.kind {
        HirExprKind::Call { args, .. } => verify_exprs(args, nodes),
        HirExprKind::Binary { left, right, .. } => verify_exprs([left.as_ref(), right.as_ref()], nodes),
        HirExprKind::Unary { inner, .. } | HirExprKind::Cast { inner, .. } => verify_expr(inner, nodes),
        HirExprKind::Field { receiver, .. } => verify_expr(receiver, nodes),
        HirExprKind::Tuple(items) | HirExprKind::List(items) => verify_exprs(items, nodes),
        HirExprKind::StructLiteral { fields, .. } => {
            verify_exprs(fields.iter().map(|(_, expr)| expr), nodes)
        }
        HirExprKind::Match { scrutinee, arms } => {
            verify_expr(scrutinee, nodes)?;
            verify_expr_arms(arms, nodes)
        }
        HirExprKind::Block(body) => verify_body(body, nodes),
        HirExprKind::Literal(_) | HirExprKind::Identifier(_) | HirExprKind::Other(_) => Ok(()),
    }
}

fn verify_expr_arms(arms: &[HirMatchArm], nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    for arm in arms {
        verify_pattern(&arm.pattern, nodes)?;
        verify_expr(&arm.body, nodes)?;
    }
    Ok(())
}

fn verify_patterns(patterns: &[HirPattern], nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    for pattern in patterns {
        verify_pattern(pattern, nodes)?;
    }
    Ok(())
}

fn verify_pattern(pattern: &HirPattern, nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    verify_node(nodes, pattern.id, "pattern")?;
    match &pattern.kind {
        HirPatternKind::Literal(expr)
        | HirPatternKind::Range(expr, _)
        | HirPatternKind::RangeInclusive(expr, _) => verify_expr(expr, nodes),
        HirPatternKind::EnumVariant(_, pats) | HirPatternKind::Tuple(pats) | HirPatternKind::Multi(pats) => {
            verify_patterns(pats, nodes)
        }
        HirPatternKind::Range(_, second) | HirPatternKind::RangeInclusive(_, second) => {
            verify_expr(second, nodes)
        }
        HirPatternKind::Wildcard | HirPatternKind::Binding(_) | HirPatternKind::TypedBinding(_, _) => Ok(()),
    }
}

fn verify_node(
    nodes: &mut BTreeSet<NodeId>,
    id: NodeId,
    what: &str,
) -> Result<(), String> {
    if nodes.insert(id) {
        Ok(())
    } else {
        Err(format!("duplicate NodeId {:?} in {what}", id))
    }
}
