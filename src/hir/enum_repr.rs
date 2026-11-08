// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b — enum representation plan over HIR.
//!
//! Mirror of `analysis::enum_repr::build_enum_representation_plan` (AST,
//! authoritative until parity). Reuses `EnumRepresentation`; `sret_safe_defns`
//! stays AST-only until its backend consumer moves (H4).

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallable, HirCallableKind, HirProgram, HirTypeDefKind};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::enum_repr::EnumRepresentation;
use crate::ast::Type;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, Default)]
pub struct HirEnumRepresentationPlan {
    pub defns: BTreeMap<String, EnumRepresentation>,
    pub variants: Vec<String>,
    pub escaping_return_lets: BTreeMap<String, BTreeSet<String>>,
}

impl HirEnumRepresentationPlan {
    pub fn defn_class(&self, name: &str) -> EnumRepresentation {
        self.defns
            .get(name)
            .copied()
            .unwrap_or(EnumRepresentation::Boxed)
    }
}

pub fn build_enum_representation_plan(program: &HirProgram) -> HirEnumRepresentationPlan {
    let (enum_names, variants) = collect_enum_variants(program);
    let variant_set: HashMap<&str, ()> = variants.iter().map(|v| (v.as_str(), ())).collect();
    let enum_set: HashMap<&str, ()> = enum_names.iter().map(|n| (n.as_str(), ())).collect();

    let (mut defns, mut candidates) = seed_candidates(program, &enum_set, &variant_set);
    while converge_candidates(program, &enum_set, &variant_set, &mut candidates) {}
    for name in candidates.keys() {
        if let Some(class) = defns.get_mut(name) {
            *class = EnumRepresentation::ReturnedSmallCandidate;
        }
    }

    let escaping_return_lets = escaping_return_lets(program, &defns, &variants);
    HirEnumRepresentationPlan {
        defns,
        variants,
        escaping_return_lets,
    }
}

/// Enum type names and their fully-qualified variant names (sorted + deduped).
fn collect_enum_variants(program: &HirProgram) -> (Vec<String>, Vec<String>) {
    let mut enum_names = Vec::new();
    let mut variants = Vec::new();
    for td in &program.type_defs {
        collect_type_variants(td, &mut enum_names, &mut variants);
    }
    variants.sort();
    variants.dedup();
    (enum_names, variants)
}

/// One enum type's names: the type name plus `Type::Variant` for each variant.
fn collect_type_variants(
    td: &super::program::HirTypeDef,
    enum_names: &mut Vec<String>,
    variants: &mut Vec<String>,
) {
    let HirTypeDefKind::Enum { variants: vs } = &td.kind else {
        return;
    };
    enum_names.push(td.name.clone());
    for v in vs {
        let name = match v {
            super::program::HirEnumVariant::Unit(n)
            | super::program::HirEnumVariant::Tuple(n, _)
            | super::program::HirEnumVariant::Struct(n, _) => n,
        };
        variants.push(name.clone());
        variants.push(format!("{}::{}", td.name, name));
    }
}

/// Seed: every enum-returning defn is Boxed; constructor-only returns join the
/// candidate set.
fn seed_candidates(
    program: &HirProgram,
    enum_set: &HashMap<&str, ()>,
    variant_set: &HashMap<&str, ()>,
) -> (
    BTreeMap<String, EnumRepresentation>,
    HashMap<String, ()>,
) {
    let mut defns = BTreeMap::new();
    let mut candidates: HashMap<String, ()> = HashMap::new();
    for c in program.callables.iter().filter(|c| is_top_defn(c)) {
        if !returns_enum(c, enum_set) {
            continue;
        }
        if all_return_forms_constructor(c, variant_set, &candidates) {
            candidates.insert(c.name.clone(), ());
        }
        defns.insert(c.name.clone(), EnumRepresentation::Boxed);
    }
    (defns, candidates)
}

/// One constructor-fixpoint round. Returns whether the candidate set grew.
fn converge_candidates(
    program: &HirProgram,
    enum_set: &HashMap<&str, ()>,
    variant_set: &HashMap<&str, ()>,
    candidates: &mut HashMap<String, ()>,
) -> bool {
    let mut changed = false;
    for c in program.callables.iter().filter(|c| is_top_defn(c)) {
        if candidates.contains_key(&c.name) {
            continue;
        }
        if returns_enum(c, enum_set) && all_return_forms_constructor(c, variant_set, candidates) {
            candidates.insert(c.name.clone(), ());
            changed = true;
        }
    }
    changed
}

fn returns_enum(c: &HirCallable, enum_set: &HashMap<&str, ()>) -> bool {
    c.result.as_ref().map_or(false, |t| type_is_enum(t, enum_set))
}

fn is_top_defn(c: &HirCallable) -> bool {
    matches!(c.kind, HirCallableKind::Defn) && c.owner.is_none() && !c.trait_default && !c.exported
}

fn type_is_enum(ty: &Type, enum_set: &HashMap<&str, ()>) -> bool {
    match ty {
        Type::Custom(name) | Type::Applied(name, _) => enum_set.contains_key(name.as_str()),
        _ => false,
    }
}

fn all_return_forms_constructor(
    c: &HirCallable,
    variants: &HashMap<&str, ()>,
    sret_calls: &HashMap<String, ()>,
) -> bool {
    let mut saw = false;
    for stmt in &c.body.statements {
        let ret = match &stmt.kind {
            HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e) => Some(e),
            _ => None,
        };
        if let Some(e) = ret {
            saw = true;
            if !return_shaped(e, c, variants, sret_calls) {
                return false;
            }
        }
    }
    saw
}

fn return_shaped(
    expr: &HirExpr,
    c: &HirCallable,
    variants: &HashMap<&str, ()>,
    sret_calls: &HashMap<String, ()>,
) -> bool {
    if constructor_shaped_return(expr, variants, sret_calls) {
        return true;
    }
    let HirExprKind::Identifier(local) = &expr.kind else {
        return false;
    };
    c.body
        .statements
        .iter()
        .find_map(|s| match &s.kind {
            HirStmtKind::Let { name, expr: Some(init), .. } if name == local => Some(init),
            _ => None,
        })
        .map(|init| constructor_shaped_return(init, variants, sret_calls))
        .unwrap_or(false)
}

fn constructor_shaped_return(
    expr: &HirExpr,
    variants: &HashMap<&str, ()>,
    sret_calls: &HashMap<String, ()>,
) -> bool {
    match &expr.kind {
        HirExprKind::Call { callee, .. } => {
            is_variant_name(callee, variants) || sret_calls.contains_key(callee)
        }
        HirExprKind::Field { name, .. } => is_variant_name(name, variants),
        HirExprKind::Match { arms, .. } => arms
            .iter()
            .all(|arm| constructor_shaped_return(&arm.body, variants, sret_calls)),
        HirExprKind::Block(stmts) => stmts
            .iter()
            .rev()
            .find_map(|s| match &s.kind {
                HirStmtKind::Expression(e) | HirStmtKind::Term(Some(e)) => Some(e),
                _ => None,
            })
            .map(|e| constructor_shaped_return(e, variants, sret_calls))
            .unwrap_or(false),
        _ => false,
    }
}

fn is_variant_name(name: &str, variants: &HashMap<&str, ()>) -> bool {
    variants.contains_key(name)
        || name
            .rsplit("::")
            .next()
            .map(|bare| variants.contains_key(bare))
            .unwrap_or(false)
}

fn escaping_return_lets(
    program: &HirProgram,
    defns: &BTreeMap<String, EnumRepresentation>,
    variants: &[String],
) -> BTreeMap<String, BTreeSet<String>> {
    let variants: HashMap<&str, ()> = variants.iter().map(|v| (v.as_str(), ())).collect();
    let mut out = BTreeMap::new();
    for c in program.callables.iter().filter(|c| is_top_defn(c)) {
        if !defns.contains_key(&c.name) {
            continue;
        }
        let names = escaping_let_names(c, &variants, defns);
        if !names.is_empty() {
            out.insert(c.name.clone(), names);
        }
    }
    out
}

fn escaping_let_names(
    c: &HirCallable,
    variants: &HashMap<&str, ()>,
    defns: &BTreeMap<String, EnumRepresentation>,
) -> BTreeSet<String> {
    let returned = return_identifiers(&c.body.statements);
    let reassigned = reassigned_identifiers(&c.body.statements);
    let mut names = BTreeSet::new();
    for stmt in &c.body.statements {
        let HirStmtKind::Let { name, expr: Some(init), .. } = &stmt.kind else {
            continue;
        };
        let immutable = !reassigned.contains(name);
        let heap_owned = heap_owned_initializer(init, variants, defns);
        if returned.contains(name) && immutable && heap_owned {
            names.insert(name.clone());
        }
    }
    names
}

fn return_identifiers(body: &[HirStmt]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_return_identifiers(body, &mut out);
    out
}

fn collect_return_identifiers(body: &[HirStmt], out: &mut BTreeSet<String>) {
    for stmt in body {
        collect_stmt_return_ids(stmt, out);
    }
}

fn collect_stmt_return_ids(stmt: &super::stmt::HirStmt, out: &mut BTreeSet<String>) {
    match &stmt.kind {
        HirStmtKind::Term(Some(HirExpr { kind: HirExprKind::Identifier(name), .. }))
        | HirStmtKind::Tail(HirExpr { kind: HirExprKind::Identifier(name), .. }) => {
            out.insert(name.clone());
        }
        HirStmtKind::Guarded(_, inner)
        | HirStmtKind::Block(inner)
        | HirStmtKind::SyncBlock(inner)
        | HirStmtKind::Defer(inner)
        | HirStmtKind::Mutex(inner)
        | HirStmtKind::Foreach { body: inner, .. } => {
            collect_return_identifiers(inner, out);
        }
        HirStmtKind::Match { arms, .. } => {
            for arm in arms {
                collect_return_identifiers(&arm.body, out);
            }
        }
        _ => {}
    }
}

fn reassigned_identifiers(body: &[HirStmt]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    walk_assign_targets(body, &mut |target| {
        if let HirExprKind::Identifier(name) = &target.kind {
            out.insert(name.clone());
        }
    });
    out
}

fn walk_assign_targets(body: &[HirStmt], f: &mut dyn FnMut(&HirExpr)) {
    for stmt in body {
        walk_stmt_assign_targets(stmt, f);
    }
}

fn walk_stmt_assign_targets(stmt: &super::stmt::HirStmt, f: &mut dyn FnMut(&HirExpr)) {
    match &stmt.kind {
        HirStmtKind::Assign(lhs, _) => f(lhs),
        HirStmtKind::ArrowAssign { target: Some(t), .. } => f(t),
        HirStmtKind::Guarded(_, inner)
        | HirStmtKind::Block(inner)
        | HirStmtKind::SyncBlock(inner)
        | HirStmtKind::Defer(inner)
        | HirStmtKind::Mutex(inner)
        | HirStmtKind::Foreach { body: inner, .. } => walk_assign_targets(inner, f),
        HirStmtKind::Match { arms, .. } => {
            for arm in arms {
                walk_assign_targets(&arm.body, f);
            }
        }
        _ => {}
    }
}

fn heap_owned_initializer(
    expr: &HirExpr,
    variants: &HashMap<&str, ()>,
    defns: &BTreeMap<String, EnumRepresentation>,
) -> bool {
    match &expr.kind {
        HirExprKind::Identifier(_) => false,
        HirExprKind::Call { callee, .. } => {
            is_variant_name(callee, variants) || defns.contains_key(callee)
        }
        HirExprKind::Match { arms, .. } => arms
            .iter()
            .all(|arm| heap_owned_initializer(&arm.body, variants, defns)),
        HirExprKind::Block(stmts) => stmts
            .iter()
            .rev()
            .find_map(|stmt| match &stmt.kind {
                HirStmtKind::Expression(expr) | HirStmtKind::Term(Some(expr)) => Some(expr),
                _ => None,
            })
            .map(|expr| heap_owned_initializer(expr, variants, defns))
            .unwrap_or(false),
        _ => false,
    }
}
