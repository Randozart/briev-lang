// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H2b explicit destination sinks. Sinks are recorded by stable HIR node IDs;
//! every sink starts boxed/unknown until the proof phase classifies it.

use super::expr::{HirExpr, HirExprKind};
use super::ids::{DefId, NodeId};
use super::program::HirCallable;
use super::program::HirProgram;
use super::stmt::{HirStmt, HirStmtKind};
use super::walk::walk_expr;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirAggregateKind {
    Tuple,
    Collection,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirSinkKey {
    Return { callable: DefId, expr: NodeId },
    CallArgument { call: NodeId, index: u32 },
    ObjectField { node: NodeId, field: String },
    CollectionElement { node: NodeId },
    Aggregate { node: NodeId, kind: HirAggregateKind },
    StateCell { node: NodeId, name: String },
    LocalBinding { statement: NodeId },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirSinkFact {
    pub key: HirSinkKey,
    pub value: NodeId,
    pub destination: super::ownership::HirDestinationClass,
    pub proof: super::ownership::HirProofStatus,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HirSinkFacts {
    pub sinks: BTreeMap<HirSinkKey, HirSinkFact>,
}

pub fn build_sink_facts(program: &HirProgram) -> HirSinkFacts {
    let mut facts = HirSinkFacts::default();
    for callable in &program.callables {
        collect_body(&mut facts, callable, &callable.body.statements);
    }
    facts
}

fn collect_body(facts: &mut HirSinkFacts, callable: &HirCallable, body: &[HirStmt]) {
    for stmt in body {
        collect_stmt(facts, callable, stmt);
    }
}

fn collect_stmt(facts: &mut HirSinkFacts, callable: &HirCallable, stmt: &HirStmt) {
    match &stmt.kind {
        HirStmtKind::Let { expr, .. } => {
            if let Some(expr) = expr {
                insert(facts, HirSinkKey::LocalBinding { statement: stmt.id }, expr.id);
                scan_expr(facts, expr);
            }
        }
        HirStmtKind::Assign(left, right) | HirStmtKind::Open(left, right) => {
            collect_assignment(facts, left);
            scan_expr(facts, left);
            scan_expr(facts, right);
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(target) = target {
                collect_assignment(facts, target);
                scan_expr(facts, target);
            }
            scan_expr(facts, value);
        }
        HirStmtKind::Term(Some(expr)) | HirStmtKind::Tail(expr) => {
            insert(facts, HirSinkKey::Return { callable: callable.id, expr: expr.id }, expr.id);
            scan_expr(facts, expr);
        }
        HirStmtKind::EndProgram(Some(expr)) => {
            insert(facts, HirSinkKey::Return { callable: callable.id, expr: expr.id }, expr.id);
            scan_expr(facts, expr);
        }
        HirStmtKind::Check(expr) | HirStmtKind::Expression(expr) | HirStmtKind::Gate(expr) => {
            scan_expr(facts, expr)
        }
        HirStmtKind::Guarded(cond, body) => {
            scan_expr(facts, cond);
            collect_body(facts, callable, body);
        }
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => collect_body(facts, callable, body),
        HirStmtKind::Foreach { list, body, .. } => {
            scan_expr(facts, list);
            collect_body(facts, callable, body);
        }
        HirStmtKind::Match { expr, arms } => {
            scan_expr(facts, expr);
            for arm in arms {
                collect_body(facts, callable, &arm.body);
            }
        }
        HirStmtKind::Rollback(Some(expr)) => scan_expr(facts, expr),
        HirStmtKind::TrgBinding { instance, .. } => scan_expr(facts, instance),
        HirStmtKind::Term(None)
        | HirStmtKind::EndProgram(None)
        | HirStmtKind::Rollback(None)
        | HirStmtKind::Break
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

fn collect_assignment(facts: &mut HirSinkFacts, target: &HirExpr) {
    let key = match &target.kind {
        HirExprKind::Field { name, .. } => HirSinkKey::ObjectField {
            node: target.id,
            field: name.clone(),
        },
        HirExprKind::Index(_, _) => HirSinkKey::CollectionElement { node: target.id },
        HirExprKind::Identifier(name) => HirSinkKey::StateCell {
            node: target.id,
            name: name.clone(),
        },
        _ => return,
    };
    insert(facts, key, target.id);
}

fn scan_expr(facts: &mut HirSinkFacts, expr: &HirExpr) {
    let mut visit = |expr: &HirExpr| collect_expr_sinks(facts, expr);
    walk_expr(expr, &mut visit);
}

fn collect_expr_sinks(facts: &mut HirSinkFacts, expr: &HirExpr) {
    match &expr.kind {
        HirExprKind::Call { args, .. } => collect_call_args(facts, expr.id, args),
        HirExprKind::StructLiteral { fields, .. } => {
            for (field, value) in fields {
                insert(
                    facts,
                    HirSinkKey::ObjectField {
                        node: expr.id,
                        field: field.clone(),
                    },
                    value.id,
                );
            }
        }
        HirExprKind::Tuple(items) => collect_aggregate(facts, expr.id, items, HirAggregateKind::Tuple),
        HirExprKind::List(items) => {
            collect_aggregate(facts, expr.id, items, HirAggregateKind::Collection)
        }
        _ => {}
    }
}

fn collect_call_args(facts: &mut HirSinkFacts, call: NodeId, args: &[HirExpr]) {
    for (index, arg) in args.iter().enumerate() {
        insert(
            facts,
            HirSinkKey::CallArgument {
                call,
                index: index as u32,
            },
            arg.id,
        );
    }
}

fn collect_aggregate(
    facts: &mut HirSinkFacts,
    node: NodeId,
    items: &[HirExpr],
    kind: HirAggregateKind,
) {
    insert(facts, HirSinkKey::Aggregate { node, kind }, node);
    let _ = items;
}

fn insert(facts: &mut HirSinkFacts, key: HirSinkKey, value: NodeId) {
    facts.sinks.insert(
        key.clone(),
        HirSinkFact {
            key,
            value,
            destination: super::ownership::HirDestinationClass::Boxed,
            proof: super::ownership::HirProofStatus::Unknown,
        },
    );
}
