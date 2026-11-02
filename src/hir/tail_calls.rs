// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H2e direct self-tail-call facts. A call counts only in return position;
//! carrier layout remains a later proof/layout decision.

use super::expr::{HirExpr, HirExprKind};
use super::ids::{DefId, NodeId};
use super::program::HirProgram;
use super::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirTailCall {
    pub node: NodeId,
    pub args: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirCallableTailCalls {
    pub callable: DefId,
    pub name: String,
    pub calls: Vec<HirTailCall>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HirTailCallFacts {
    pub callables: BTreeMap<DefId, HirCallableTailCalls>,
}

pub fn build_tail_call_facts(program: &HirProgram) -> HirTailCallFacts {
    let mut facts = HirTailCallFacts::default();
    for callable in &program.callables {
        let mut calls = Vec::new();
        collect_body(&callable.name, &callable.body.statements, &mut calls);
        if !calls.is_empty() {
            facts.callables.insert(
                callable.id,
                HirCallableTailCalls {
                    callable: callable.id,
                    name: callable.name.clone(),
                    calls,
                },
            );
        }
    }
    facts
}

fn collect_body(name: &str, body: &[HirStmt], out: &mut Vec<HirTailCall>) {
    for stmt in body {
        collect_stmt(name, stmt, out);
    }
}

fn collect_stmt(name: &str, stmt: &HirStmt, out: &mut Vec<HirTailCall>) {
    match &stmt.kind {
        HirStmtKind::Term(Some(expr)) | HirStmtKind::Tail(expr) => {
            collect_tail_expr(name, expr, out)
        }
        HirStmtKind::Guarded(_, body)
        | HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => collect_body(name, body, out),
        HirStmtKind::Match { arms, .. } => collect_arms(name, arms, out),
        _ => {}
    }
}

fn collect_arms(name: &str, arms: &[HirStmtMatchArm], out: &mut Vec<HirTailCall>) {
    for arm in arms {
        collect_body(name, &arm.body, out);
    }
}

fn collect_tail_expr(name: &str, expr: &HirExpr, out: &mut Vec<HirTailCall>) {
    match &expr.kind {
        HirExprKind::Call { callee, args, .. } if callee == name => out.push(HirTailCall {
            node: expr.id,
            args: args.iter().map(|arg| arg.id).collect(),
        }),
        HirExprKind::If { then_branch, else_branch, .. } => {
            collect_tail_expr(name, then_branch, out);
            if let Some(else_branch) = else_branch {
                collect_tail_expr(name, else_branch, out);
            }
        }
        HirExprKind::Match { arms, .. } => {
            for arm in arms {
                collect_tail_expr(name, &arm.body, out);
            }
        }
        HirExprKind::Block(body) => collect_body(name, body, out),
        _ => {}
    }
}
