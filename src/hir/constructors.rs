// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H2d enum constructor/variant resolution. Names resolve against HIR type
//! facts; duplicate variant names stay as an explicit candidate set instead of
//! being guessed. No codegen consumes this table yet.

use super::expr::{HirExpr, HirExprKind};
use super::ids::NodeId;
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{HirEnumVariant, HirProgram};
use super::type_facts::build_type_facts;
use super::walk::{walk_expr, walk_pattern, walk_stmt};
use crate::ast::Type;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct HirVariantRef {
    pub enum_name: String,
    pub payload_types: Vec<Type>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirVariantFact {
    pub name: String,
    pub candidates: Vec<HirVariantRef>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HirConstructorFacts {
    pub calls: BTreeMap<NodeId, HirVariantFact>,
    pub patterns: BTreeMap<NodeId, HirVariantFact>,
}

pub fn build_constructor_facts(program: &HirProgram) -> HirConstructorFacts {
    let index = variant_index(program);
    let mut facts = HirConstructorFacts::default();
    for callable in &program.callables {
        collect_callable(&index, &mut facts, callable);
    }
    facts
}

fn collect_callable(
    index: &BTreeMap<String, Vec<HirVariantRef>>,
    facts: &mut HirConstructorFacts,
    callable: &super::program::HirCallable,
) {
    for stmt in &callable.body.statements {
        collect_stmt(index, facts, stmt);
    }
}

fn variant_index(program: &HirProgram) -> BTreeMap<String, Vec<HirVariantRef>> {
    let type_facts = build_type_facts(program);
    let mut index: BTreeMap<String, Vec<HirVariantRef>> = BTreeMap::new();
    for (enum_name, facts) in &type_facts.enums {
        index_variants(enum_name, &facts.variants, &mut index);
    }
    index
}

fn index_variants(
    enum_name: &str,
    variants: &[HirEnumVariant],
    index: &mut BTreeMap<String, Vec<HirVariantRef>>,
) {
    for variant in variants {
        let (name, payload_types) = variant_name_and_payload(variant);
        index.entry(name).or_default().push(HirVariantRef {
            enum_name: enum_name.to_string(),
            payload_types,
        });
    }
}

fn variant_name_and_payload(variant: &HirEnumVariant) -> (String, Vec<Type>) {
    match variant {
        HirEnumVariant::Unit(name) => (name.clone(), Vec::new()),
        HirEnumVariant::Tuple(name, types) => (name.clone(), types.clone()),
        HirEnumVariant::Struct(name, fields) => {
            (name.clone(), fields.iter().map(|field| field.ty.clone()).collect())
        }
    }
}

fn collect_stmt(
    index: &BTreeMap<String, Vec<HirVariantRef>>,
    facts: &mut HirConstructorFacts,
    stmt: &super::stmt::HirStmt,
) {
    let mut visit_expr = |expr: &HirExpr| collect_expr(index, facts, expr);
    walk_stmt(stmt, &mut visit_expr);
}

fn collect_expr(
    index: &BTreeMap<String, Vec<HirVariantRef>>,
    facts: &mut HirConstructorFacts,
    expr: &HirExpr,
) {
    if let HirExprKind::Call { callee, .. } = &expr.kind {
        if let Some(candidates) = index.get(callee) {
            facts.calls.insert(
                expr.id,
                HirVariantFact {
                    name: callee.clone(),
                    candidates: candidates.clone(),
                },
            );
        }
    }
    match &expr.kind {
        HirExprKind::Match { arms, .. } => {
            for arm in arms {
                collect_pattern(index, facts, &arm.pattern);
            }
        }
        _ => {}
    }
}

fn collect_pattern(
    index: &BTreeMap<String, Vec<HirVariantRef>>,
    facts: &mut HirConstructorFacts,
    pattern: &HirPattern,
) {
    let mut visit = |pattern: &HirPattern| {
        let name = match &pattern.kind {
            HirPatternKind::EnumVariant(name, _) | HirPatternKind::Binding(name) => name,
            _ => return,
        };
        let Some(candidates) = index.get(name) else {
            return;
        };
        facts.patterns.insert(
            pattern.id,
            HirVariantFact {
                name: name.clone(),
                candidates: candidates.clone(),
            },
        );
    };
    walk_pattern(pattern, &mut visit);
}
