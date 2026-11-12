// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-12 (H5 port): weighted inline-cost over typed HIR.
//!
//! Mirror of `analysis::inline_cost` (AST remains the fallback/oracle): call
//! = 10, binop = 1, operand = 1, statement = 1. FFI/trigger exclusion uses
//! the same rule as the AST (Term/EndProgram/Tail/InlineAsm/Call inside an
//! assignment).

use super::expr::{HirExpr, HirExprKind};
use super::program::HirCallable;
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::inline_cost::InlineDecision;
use crate::ast::BinaryOpKind;

pub fn callable_inline_decision_hir(callable: &HirCallable) -> InlineDecision {
    if has_ffi_or_trigger(&callable.body.statements) {
        return InlineDecision::No;
    }
    let w = weight_of_body(&callable.body.statements);
    let threshold = crate::config_tuning::ir_lowering().callable_inline_weight_threshold;
    if std::env::var_os("BRIEV_DIAG_INLINE").is_some() {
        eprintln!("HIR callable_inline weight={w} threshold={threshold}");
    }
    if w <= threshold {
        InlineDecision::AlwaysInline
    } else {
        InlineDecision::No
    }
}

pub fn node_inline_decision_hir(callable: &HirCallable) -> InlineDecision {
    if weight_of_body(&callable.body.statements)
        <= crate::config_tuning::ir_lowering().node_inline_weight_threshold
    {
        InlineDecision::AlwaysInline
    } else {
        InlineDecision::No
    }
}

fn has_ffi_or_trigger(body: &[HirStmt]) -> bool {
    body.iter().any(has_ffi_or_trigger_stmt)
}

fn has_ffi_or_trigger_stmt(stmt: &HirStmt) -> bool {
    match &stmt.kind {
        HirStmtKind::Term(_)
        | HirStmtKind::EndProgram(_)
        | HirStmtKind::Tail(_)
        | HirStmtKind::InlineAsm { .. } => true,
        HirStmtKind::Assign(_, rhs) => expr_has_call(rhs),
        HirStmtKind::Guarded(_, body)
        | HirStmtKind::Block(body)
        | HirStmtKind::Mutex(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Foreach { body, .. } => has_ffi_or_trigger(body),
        HirStmtKind::Match { arms, .. } => {
            arms.iter().any(|arm| has_ffi_or_trigger(&arm.body))
        }
        _ => false,
    }
}

fn expr_has_call(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Call { .. } | HirExprKind::Spawn { .. } => true,
        HirExprKind::Binary { left, right, .. } => expr_has_call(left) || expr_has_call(right),
        HirExprKind::Unary { inner, .. } => expr_has_call(inner),
        HirExprKind::Cast { inner, .. } => expr_has_call(inner),
        HirExprKind::Index(a, b) => expr_has_call(a) || expr_has_call(b),
        HirExprKind::Field { receiver, .. } => expr_has_call(receiver),
        _ => false,
    }
}

fn weight_of_body(body: &[HirStmt]) -> u32 {
    body.iter().map(weight_of_stmt).sum()
}

fn weight_of_stmt(stmt: &HirStmt) -> u32 {
    match &stmt.kind {
        HirStmtKind::Let { expr, .. } => 1 + expr.as_ref().map_or(0, weight_of_expr),
        HirStmtKind::Assign(lhs, rhs) => 1 + weight_of_expr(lhs) + weight_of_expr(rhs),
        HirStmtKind::Term(Some(e)) | HirStmtKind::EndProgram(Some(e)) | HirStmtKind::Tail(e) => {
            1 + weight_of_expr(e)
        }
        HirStmtKind::Term(None) | HirStmtKind::EndProgram(None) => 1,
        HirStmtKind::Expression(e) => 1 + weight_of_expr(e),
        HirStmtKind::Guarded(cond, body) => 1 + weight_of_expr(cond) + weight_of_body(body),
        HirStmtKind::Gate(e) => 1 + weight_of_expr(e),
        HirStmtKind::Block(b) => 1 + weight_of_body(b),
        HirStmtKind::Foreach { list, body, .. } => 1 + weight_of_expr(list) + weight_of_body(body),
        HirStmtKind::MetadataAssignment(..) => 1,
        HirStmtKind::Rollback(Some(e)) => 1 + weight_of_expr(e),
        HirStmtKind::Rollback(None) => 1,
        _ => 1,
    }
}

fn weight_of_expr(expr: &HirExpr) -> u32 {
    match &expr.kind {
        HirExprKind::Call { args, .. } => 10 + args.iter().map(weight_of_expr).sum::<u32>(),
        HirExprKind::Binary { left, right, .. } => {
            1 + weight_of_expr(left) + weight_of_expr(right)
        }
        HirExprKind::Unary { inner, .. } => 1 + weight_of_expr(inner),
        HirExprKind::Identifier(_) => 1,
        HirExprKind::Index(a, b) => 1 + weight_of_expr(a) + weight_of_expr(b),
        HirExprKind::Field { receiver, .. } => 1 + weight_of_expr(receiver),
        HirExprKind::Slice {
            array,
            start,
            end,
            stride,
        } => {
            1 + weight_of_expr(array)
                + start.as_ref().map_or(0, |v| weight_of_expr(v))
                + end.as_ref().map_or(0, |v| weight_of_expr(v))
                + stride.as_ref().map_or(0, |v| weight_of_expr(v))
        }
        HirExprKind::Cast { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner) => 1 + weight_of_expr(inner),
        HirExprKind::Tuple(ts) | HirExprKind::List(ts) => {
            1 + ts.iter().map(weight_of_expr).sum::<u32>()
        }
        HirExprKind::Block(stmts) => weight_of_body(stmts),
        HirExprKind::If { condition, then_branch, else_branch } => {
            1 + weight_of_expr(condition)
                + weight_of_expr(then_branch)
                + else_branch.as_ref().map_or(0, |v| weight_of_expr(v))
        }
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::inline_cost::InlineDecision as AstDecision;
    use crate::hir::expr::{HirExpr, HirExprKind, HirLiteral};
    use crate::hir::stmt::{HirStmt, HirStmtKind};
    use crate::hir::program::HirBody;

    fn parse(src: &str) -> Vec<crate::ast::TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    fn callable_with(body: Vec<HirStmt>) -> HirCallable {
        crate::hir::lower_program(&parse("defn dummy(a: Int) -> Int { term a; };"))
            .callables
            .first()
            .map(|c| HirCallable {
                body: HirBody { node: c.body.node, span: c.body.span, statements: body },
                ..c.clone()
            })
            .unwrap()
    }

    fn id(name: &str) -> HirExpr {
        HirExpr { id: crate::hir::ids::NodeId(0), span: None, ty: None,
                  kind: HirExprKind::Identifier(name.into()) }
    }

    fn int(v: i64) -> HirExpr {
        HirExpr { id: crate::hir::ids::NodeId(0), span: None, ty: None,
                  kind: HirExprKind::Literal(HirLiteral::Int(v)) }
    }

    fn let_stmt(name: &str, e: HirExpr) -> HirStmt {
        HirStmt { id: crate::hir::ids::NodeId(0), span: None,
                  kind: HirStmtKind::Let { name: name.into(), names: vec![], ty: None,
                                           expr: Some(e), modifiers: vec![] } }
    }

    #[test]
    fn small_body_is_always_inline() {
        let callable = callable_with(vec![let_stmt("v0", id("a")), let_stmt("v1", id("v0"))]);
        assert_eq!(callable_inline_decision_hir(&callable), AstDecision::AlwaysInline);
        assert_eq!(node_inline_decision_hir(&callable), AstDecision::AlwaysInline);
    }

    #[test]
    fn large_body_callable_stays_no() {
        let body: Vec<HirStmt> = (0..60)
            .map(|i| let_stmt(&format!("v{i}"), int(i as i64)))
            .collect();
        let callable = callable_with(body);
        assert_eq!(callable_inline_decision_hir(&callable), AstDecision::No);
        assert_eq!(node_inline_decision_hir(&callable), AstDecision::AlwaysInline);
    }

    #[test]
    fn large_body_node_stays_no() {
        let body: Vec<HirStmt> = (0..400)
            .map(|i| let_stmt(&format!("v{i}"), int(i as i64)))
            .collect();
        let callable = callable_with(body);
        assert_eq!(node_inline_decision_hir(&callable), AstDecision::No);
    }

    #[test]
    fn term_counts_as_ffi_for_callable_inline() {
        let callable = callable_with(vec![
            HirStmt { id: crate::hir::ids::NodeId(0), span: None,
                      kind: HirStmtKind::Term(Some(int(1))) },
        ]);
        assert_eq!(callable_inline_decision_hir(&callable), AstDecision::No);
        assert_eq!(node_inline_decision_hir(&callable), AstDecision::AlwaysInline);
    }
}
