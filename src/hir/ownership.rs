// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H2a conservative ownership facts keyed by stable HIR identity.
//!
//! This table establishes the identity surface before any optimization: every
//! parameter, local, foreach item, and pattern binding gets one stable key.
//! Unknown ownership stays boxed. No backend consumes these facts yet.

use super::ids::{DefId, NodeId};
use super::pattern::{HirPattern, HirPatternKind};
use super::program::HirProgram;
use super::expr::HirExprKind;
use super::stmt::{HirStmt, HirStmtKind};
use super::walk::walk_stmt;
use crate::ast::Type;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirValueKey {
    Param { callable: DefId, index: u32 },
    Let { statement: NodeId },
    ForeachItem { statement: NodeId },
    PatternBinding { pattern: NodeId, name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirOwnershipClass {
    Owned,
    Borrowed,
    Consumed,
    Shared,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirDestinationClass {
    Scalar,
    CallerDestination,
    StableCarrier,
    PackedMemory,
    Boxed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HirProofStatus {
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirValueFact {
    pub key: HirValueKey,
    pub ty: Option<Type>,
    pub ownership: HirOwnershipClass,
    pub destination: HirDestinationClass,
    pub proof: HirProofStatus,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HirValueFacts {
    pub values: BTreeMap<HirValueKey, HirValueFact>,
}

/// Build conservative facts. Missing type information never changes the
/// decision: unknown ownership stays boxed.
pub fn build_value_facts(program: &HirProgram) -> HirValueFacts {
    let mut facts = HirValueFacts::default();
    for callable in &program.callables {
        collect_params(&mut facts, callable);
        collect_body(&mut facts, &callable.body.statements);
    }
    facts
}

fn collect_params(facts: &mut HirValueFacts, callable: &super::program::HirCallable) {
    for (index, param) in callable.params.iter().enumerate() {
        insert(
            facts,
            HirValueKey::Param {
                callable: callable.id,
                index: index as u32,
            },
            Some(param.ty.clone()),
        );
    }
}

fn collect_body(facts: &mut HirValueFacts, body: &[HirStmt]) {
    for stmt in body {
        collect_stmt(facts, stmt);
    }
}

fn collect_stmt(facts: &mut HirValueFacts, stmt: &HirStmt) {
    match &stmt.kind {
        HirStmtKind::Let { ty, expr, .. } => {
            insert(
                facts,
                HirValueKey::Let { statement: stmt.id },
                ty.clone().or_else(|| expr.as_ref().and_then(|expr| expr.ty.clone())),
            );
        }
        HirStmtKind::Foreach { body, .. } => {
            insert(facts, HirValueKey::ForeachItem { statement: stmt.id }, None);
            collect_body(facts, body);
        }
        HirStmtKind::Guarded(_, body)
        | HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => collect_body(facts, body),
        HirStmtKind::Match { arms, .. } => collect_arms(facts, arms),
        _ => {}
    }
    collect_expr_patterns(facts, stmt);
}

fn collect_arms(facts: &mut HirValueFacts, arms: &[super::stmt::HirStmtMatchArm]) {
    for arm in arms {
        collect_patterns(facts, &arm.patterns);
        collect_body(facts, &arm.body);
    }
}

fn collect_expr_patterns(facts: &mut HirValueFacts, stmt: &HirStmt) {
    let mut visit = |expr: &super::expr::HirExpr| {
        let HirExprKind::Match { arms, .. } = &expr.kind else {
            return;
        };
        for arm in arms {
            collect_pattern(facts, &arm.pattern);
        }
    };
    walk_stmt(stmt, &mut visit);
}

fn collect_pattern(facts: &mut HirValueFacts, pattern: &HirPattern) {
    match &pattern.kind {
        HirPatternKind::Binding(name) => {
            insert(
                facts,
                HirValueKey::PatternBinding {
                    pattern: pattern.id,
                    name: name.clone(),
                },
                None,
            );
        }
        HirPatternKind::TypedBinding(name, ty) => {
            insert(
                facts,
                HirValueKey::PatternBinding {
                    pattern: pattern.id,
                    name: name.clone(),
                },
                Some(ty.clone()),
            );
        }
        HirPatternKind::EnumVariant(_, patterns)
        | HirPatternKind::Tuple(patterns)
        | HirPatternKind::Multi(patterns) => collect_patterns(facts, patterns),
        HirPatternKind::Wildcard
        | HirPatternKind::Literal(_)
        | HirPatternKind::Range(_, _)
        | HirPatternKind::RangeInclusive(_, _) => {}
    }
}

fn collect_patterns(facts: &mut HirValueFacts, patterns: &[HirPattern]) {
    for pattern in patterns {
        collect_pattern(facts, pattern);
    }
}

fn insert(facts: &mut HirValueFacts, key: HirValueKey, ty: Option<Type>) {
    facts.values.insert(
        key.clone(),
        HirValueFact {
            key,
            ty,
            ownership: HirOwnershipClass::Unknown,
            destination: HirDestinationClass::Boxed,
            proof: HirProofStatus::Unknown,
        },
    );
}
