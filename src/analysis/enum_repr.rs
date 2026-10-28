// Copyright 2026 Randy Smits-Schreuder Goedheijt
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Enum/small-sum representation analysis (P2-1, Phase A).
//!
//! Classifies enum-returning defns by whether every return form is a
//! constructor (or a match of constructors), which is the prerequisite for the
//! `InlineLocal` and `ReturnedSmall` (sret) lowering. Defaults to `Boxed` when
//! any fact is unknown; nothing here changes codegen.

use crate::ast::{EnumVariant, Expr, Statement, TopLevel};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Representation class for an enum-producing surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumRepresentation {
    /// Constructor value consumed entirely inside its defining function.
    InlineLocalCandidate,
    /// Internal defn whose return forms are all constructor-shaped.
    ReturnedSmallCandidate,
    /// Existing malloc-backed `{tag, payload}` ABI.
    Boxed,
}

#[derive(Debug, Clone, Default)]
pub struct EnumRepresentationPlan {
    /// Enum-returning defn → classification. Deterministic by name.
    pub defns: BTreeMap<String, EnumRepresentation>,
    /// All declared variant names (bare and `Enum::Variant` where derivable).
    pub variants: Vec<String>,
    /// Immutable local bindings returned directly by an enum-returning defn
    /// whose initializer can be given heap ownership. Empty means the caller
    /// must retain a stack-backed image at return.
    pub escaping_return_lets: BTreeMap<String, BTreeSet<String>>,
}

impl EnumRepresentationPlan {
    pub fn defn_class(&self, name: &str) -> EnumRepresentation {
        self.defns
            .get(name)
            .copied()
            .unwrap_or(EnumRepresentation::Boxed)
    }
}

/// Build the representation plan from the top-level surface.
///
/// This is deliberately conservative: only constructor-shaped return forms are
/// candidates. A local-let candidate is recorded only when the initializer is
/// an enum constructor and every use is a direct tag/payload consumer in the
/// same function; otherwise the value is boxed.
pub fn build_enum_representation_plan(items: &[TopLevel]) -> EnumRepresentationPlan {
    let mut variants = Vec::new();
    let mut enum_names = Vec::new();
    for item in items {
        match item {
            TopLevel::Enum(e) => {
                enum_names.push(e.name.clone());
                for v in &e.variants {
                    match v {
                        EnumVariant::Unit(n)
                        | EnumVariant::Tuple(n, _)
                        | EnumVariant::Struct(n, _) => {
                            variants.push(n.clone());
                            variants.push(format!("{}::{}", e.name, n));
                        }
                    }
                }
            }
            TopLevel::TypeDef(td) => {
                let is_enum = td
                    .body
                    .slots
                    .iter()
                    .any(|s| s.name.starts_with("__variant_"));
                if is_enum {
                    enum_names.push(td.name.clone());
                    for slot in &td.body.slots {
                        if let Some(v) = slot.name.strip_prefix("__variant_") {
                            variants.push(v.to_string());
                            variants.push(format!("{}::{}", td.name, v));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    variants.sort();
    variants.dedup();

    let variant_set: HashMap<&str, ()> = variants.iter().map(|v| (v.as_str(), ())).collect();
    let enum_set: HashMap<&str, ()> = enum_names.iter().map(|n| (n.as_str(), ())).collect();
    let mut defns = BTreeMap::new();
    let mut candidates: HashMap<String, ()> = HashMap::new();

    // Seed: enum-returning defns whose returns are constructors.
    for item in items {
        let TopLevel::Definition(d) = item else {
            continue;
        };
        let return_is_enum = defn_return_type(d)
            .map(|t| type_is_enum(&t, &enum_set))
            .unwrap_or(false);
        if !return_is_enum {
            continue;
        }
        if all_return_forms_constructor(d, &variant_set, &candidates) {
            candidates.insert(d.name.clone(), ());
        }
        defns.insert(d.name.clone(), EnumRepresentation::Boxed);
    }

    // Fixpoint: an enum-returning defn that returns a call to an sret
    // candidate is itself an sret passthrough (`ws_decode` -> `ws_decode_mask`).
    loop {
        let mut changed = false;
        for item in items {
            let TopLevel::Definition(d) = item else {
                continue;
            };
            if candidates.contains_key(&d.name) {
                continue;
            }
            let return_is_enum = defn_return_type(d)
                .map(|t| type_is_enum(&t, &enum_set))
                .unwrap_or(false);
            if return_is_enum && all_return_forms_constructor(d, &variant_set, &candidates) {
                candidates.insert(d.name.clone(), ());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for name in candidates.keys() {
        if let Some(class) = defns.get_mut(name) {
            *class = EnumRepresentation::ReturnedSmallCandidate;
        }
    }

    let escaping_return_lets = escaping_return_lets(items, &defns, &variants);
    EnumRepresentationPlan { defns, variants, escaping_return_lets }
}

/// Prove the local bindings whose enum image can be heap-owned at return.
///
/// This is deliberately narrower than liveness: a returned alias still points
/// at another local, and a reassigned binding may expose more than one image.
/// Both keep the existing return-time copy.
fn escaping_return_lets(
    items: &[TopLevel],
    defns: &BTreeMap<String, EnumRepresentation>,
    variants: &[String],
) -> BTreeMap<String, BTreeSet<String>> {
    let variants: HashMap<&str, ()> = variants.iter().map(|v| (v.as_str(), ())).collect();
    let mut out = BTreeMap::new();
    for item in items {
        let TopLevel::Definition(d) = item else { continue };
        if !defns.contains_key(&d.name) {
            continue;
        }
        let names = escaping_let_names(d, &variants, defns);
        if !names.is_empty() {
            out.insert(d.name.clone(), names);
        }
    }
    out
}

fn escaping_let_names(
    d: &crate::ast::Definition,
    variants: &HashMap<&str, ()>,
    defns: &BTreeMap<String, EnumRepresentation>,
) -> BTreeSet<String> {
    let returned = return_identifiers(&d.body);
    let reassigned = reassigned_identifiers(&d.body);
    let mut names = BTreeSet::new();
    for stmt in &d.body {
        let Statement::Let { name, expr: Some(init), .. } = stmt else {
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

fn return_identifiers(body: &[Statement]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_return_identifiers(body, &mut out);
    out
}

fn collect_return_identifiers(body: &[Statement], out: &mut BTreeSet<String>) {
    for stmt in body {
        match stmt {
            Statement::Term(Some(Expr::Identifier(name)))
            | Statement::Tail(Expr::Identifier(name)) => {
                out.insert(name.clone());
            }
            Statement::Guarded(_, inner)
            | Statement::Block(inner)
            | Statement::SyncBlock(inner)
            | Statement::Defer(inner)
            | Statement::Mutex(inner)
            | Statement::Foreach { body: inner, .. } => {
                collect_return_identifiers(inner, out);
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    collect_return_identifiers(&arm.body, out);
                }
            }
            _ => {}
        }
    }
}

fn reassigned_identifiers(body: &[Statement]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    crate::ast::visit::walk_assign_targets(body, &mut |target| {
        if let Expr::Identifier(name) = target {
            out.insert(name.clone());
        }
    });
    out
}

fn heap_owned_initializer(
    expr: &Expr,
    variants: &HashMap<&str, ()>,
    defns: &BTreeMap<String, EnumRepresentation>,
) -> bool {
    match expr {
        // A direct alias points at another local's image; only the alias's
        // own initializer can prove heap ownership.
        Expr::Identifier(_) => false,
        Expr::Call(name, _, _) => {
            is_variant_name(name, variants) || defns.contains_key(name)
        }
        Expr::Match(_, arms) => arms
            .iter()
            .all(|arm| heap_owned_initializer(&arm.body, variants, defns)),
        Expr::Block(stmts) => stmts
            .iter()
            .rev()
            .find_map(|stmt| match stmt {
                Statement::Expression(expr) | Statement::Term(Some(expr)) => Some(expr),
                _ => None,
            })
            .map(|expr| heap_owned_initializer(expr, variants, defns))
            .unwrap_or(false),
        _ => false,
    }
}

/// Conservative sret eligibility: a candidate must not be called from another
/// enum-returning defn. That caller may hold the returned handle in a local
/// let and return it, which would dangle the caller-frame slot. Keeping such
/// callees boxed is safe; SC (single-caller node) remains sret.
pub fn sret_safe_defns(items: &[TopLevel], plan: &EnumRepresentationPlan) -> Vec<String> {
    let enum_defs: HashMap<&str, ()> = plan.defns.keys().map(|n| (n.as_str(), ())).collect();
    let enum_names = collect_enum_names(items);
    let mut unsafe_names: HashMap<String, ()> = HashMap::new();
    scan_unsafe_sret_callers(items, plan, &enum_defs, &enum_names, &mut unsafe_names);
    plan.defns
        .iter()
        .filter(|(name, class)| {
            **class == EnumRepresentation::ReturnedSmallCandidate
                && !unsafe_names.contains_key(*name)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn scan_unsafe_sret_callers(
    items: &[TopLevel],
    plan: &EnumRepresentationPlan,
    enum_defs: &HashMap<&str, ()>,
    enum_names: &HashMap<&str, ()>,
    unsafe_names: &mut HashMap<String, ()>,
) {
    for item in items {
        match item {
            TopLevel::Definition(d) => {
                scan_definition_sret_callers(d, plan, enum_defs, unsafe_names);
            }
            TopLevel::Transaction(t) => {
                scan_transaction_return_calls(t, plan, enum_names, unsafe_names);
            }
            _ => {}
        }
    }
}

fn scan_definition_sret_callers(
    d: &crate::ast::Definition,
    plan: &EnumRepresentationPlan,
    enum_defs: &HashMap<&str, ()>,
    unsafe_names: &mut HashMap<String, ()>,
) {
    if !enum_defs.contains_key(d.name.as_str()) {
        return;
    }
    if plan.defn_class(&d.name) == EnumRepresentation::ReturnedSmallCandidate {
        return;
    }
    use crate::ast::visit::walk_stmts;
    walk_stmts(&d.body, &mut |e| {
        mark_sret_candidate_call(e, plan, unsafe_names);
    });
}

fn scan_transaction_return_calls(
    t: &crate::ast::Transaction,
    plan: &EnumRepresentationPlan,
    enum_names: &HashMap<&str, ()>,
    unsafe_names: &mut HashMap<String, ()>,
) {
    let returns_enum = txn_return_type(t)
        .map(|ty| type_is_enum(&ty, enum_names))
        .unwrap_or(false);
    if !returns_enum || plan.defn_class(&t.name) == EnumRepresentation::ReturnedSmallCandidate {
        return;
    }
    collect_txn_return_calls(&t.body, plan, unsafe_names);
}

fn mark_sret_candidate_call(
    expr: &Expr,
    plan: &EnumRepresentationPlan,
    unsafe_names: &mut HashMap<String, ()>,
) {
    if let Expr::Call(callee, _, _) = expr {
        if plan.defn_class(callee) == EnumRepresentation::ReturnedSmallCandidate {
            unsafe_names.insert(callee.clone(), ());
        }
    }
}

fn collect_enum_names(items: &[TopLevel]) -> HashMap<&str, ()> {
    let mut names = HashMap::new();
    for item in items {
        match item {
            TopLevel::Enum(e) => {
                names.insert(e.name.as_str(), ());
            }
            TopLevel::TypeDef(td)
                if td
                    .body
                    .slots
                    .iter()
                    .any(|slot| slot.name.starts_with("__variant_")) =>
            {
                names.insert(td.name.as_str(), ());
            }
            _ => {}
        }
    }
    names
}

fn collect_txn_return_calls(
    stmts: &[crate::ast::Statement],
    plan: &EnumRepresentationPlan,
    unsafe_names: &mut HashMap<String, ()>,
) {
    for stmt in stmts {
        match stmt {
            crate::ast::Statement::Term(Some(expr)) | crate::ast::Statement::Tail(expr) => {
                collect_return_expr_calls(expr, plan, unsafe_names);
            }
            crate::ast::Statement::Guarded(_, body)
            | crate::ast::Statement::Block(body)
            | crate::ast::Statement::SyncBlock(body)
            | crate::ast::Statement::Mutex(body)
            | crate::ast::Statement::Foreach { body, .. } => {
                collect_txn_return_calls(body, plan, unsafe_names);
            }
            _ => {}
        }
    }
}

fn collect_return_expr_calls(
    expr: &Expr,
    plan: &EnumRepresentationPlan,
    unsafe_names: &mut HashMap<String, ()>,
) {
    match expr {
        Expr::Call(callee, _, _) => mark_sret_candidate_call(expr, plan, unsafe_names),
        Expr::Match(_, arms) => {
            for arm in arms {
                collect_return_expr_calls(&arm.body, plan, unsafe_names);
            }
        }
        Expr::Block(stmts) => collect_txn_return_calls(stmts, plan, unsafe_names),
        _ => {}
    }
}

fn txn_return_type(t: &crate::ast::Transaction) -> Option<crate::ast::Type> {
    if let Some(ot) = &t.output_type {
        return ot.all_types().into_iter().next();
    }
    t.outputs.first().cloned()
}

fn defn_return_type(d: &crate::ast::Definition) -> Option<crate::ast::Type> {
    if let Some(ot) = &d.output_type {
        return ot.all_types().into_iter().next();
    }
    d.outputs.first().cloned()
}

fn type_is_enum(ty: &crate::ast::Type, enum_set: &HashMap<&str, ()>) -> bool {
    match ty {
        crate::ast::Type::Custom(name) | crate::ast::Type::Applied(name, _) => {
            enum_set.contains_key(name.as_str())
        }
        _ => false,
    }
}

fn all_return_forms_constructor(
    d: &crate::ast::Definition,
    variants: &HashMap<&str, ()>,
    sret_calls: &HashMap<String, ()>,
) -> bool {
    let mut saw = false;
    for stmt in &d.body {
        let ret = match stmt {
            Statement::Term(Some(e)) | Statement::Tail(e) => Some(e),
            _ => None,
        };
        if let Some(e) = ret {
            saw = true;
            if !return_shaped(e, d, variants, sret_calls) {
                return false;
            }
        }
    }
    saw
}

/// Return expression shaped like a constructor or an sret call, including a
/// local let initialized by either (`let r = inner(x); term r`).
fn return_shaped(
    expr: &Expr,
    d: &crate::ast::Definition,
    variants: &HashMap<&str, ()>,
    sret_calls: &HashMap<String, ()>,
) -> bool {
    if constructor_shaped_return(expr, variants, sret_calls) {
        return true;
    }
    let Expr::Identifier(local) = expr else {
        return false;
    };
    d.body
        .iter()
        .find_map(|s| match s {
            Statement::Let {
                name,
                expr: Some(init),
                ..
            } if name == local => Some(init),
            _ => None,
        })
        .map(|init| constructor_shaped_return(init, variants, sret_calls))
        .unwrap_or(false)
}

/// A return expression is constructor-shaped when it is a constructor call,
/// a qualified variant field, or a match whose every arm is constructor-shaped.
fn constructor_shaped_return(
    expr: &Expr,
    variants: &HashMap<&str, ()>,
    sret_calls: &HashMap<String, ()>,
) -> bool {
    match expr {
        Expr::Call(name, _, _) => is_variant_name(name, variants) || sret_calls.contains_key(name),
        Expr::Field(_, name) => is_variant_name(name, variants),
        Expr::Match(_, arms) => arms
            .iter()
            .all(|arm| constructor_shaped_return(&arm.body, variants, sret_calls)),
        Expr::Block(stmts) => stmts
            .iter()
            .rev()
            .find_map(|s| match s {
                Statement::Expression(e) | Statement::Term(Some(e)) => Some(e),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<TopLevel> {
        let toks = crate::lexer::tokenize(src).expect("tokenize");
        crate::parser::Parser::new(toks, src)
            .parse_program()
            .expect("parse")
    }

    #[test]
    fn direct_constructor_return_is_candidate() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn f(x: Int) -> R { term Ok(x); };\n",
        );
        let plan = build_enum_representation_plan(&items);
        assert_eq!(
            plan.defn_class("f"),
            EnumRepresentation::ReturnedSmallCandidate
        );
    }

    #[test]
    fn match_of_constructors_is_candidate() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn f(x: Int) -> R { term match x { 0 => Ok(1), _ => Err(2) }; };\n",
        );
        let plan = build_enum_representation_plan(&items);
        assert_eq!(
            plan.defn_class("f"),
            EnumRepresentation::ReturnedSmallCandidate
        );
    }

    #[test]
    fn sret_passthrough_caller_keeps_callee_safe() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn inner(x: Int) -> R { term Ok(x); };\n\
             defn outer(x: Int) -> R { let r: R = inner(x); term r; };\n",
        );
        let plan = build_enum_representation_plan(&items);
        let safe = sret_safe_defns(&items, &plan);
        assert!(
            safe.contains(&"inner".to_string()),
            "sret caller can pass its out slot through"
        );
        assert!(
            safe.contains(&"outer".to_string()),
            "local return of an sret value is an sret passthrough"
        );
    }

    #[test]
    fn enum_returning_txn_cannot_leak_callee_sret_slot() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn inner(x: Int) -> R { term Ok(x); };\n\
             txn outer(x: Int) [x >= 0][true] -> R { term inner(x); };\n",
        );
        let plan = build_enum_representation_plan(&items);
        let safe = sret_safe_defns(&items, &plan);
        assert!(
            !safe.contains(&"inner".to_string()),
            "enum-returning txn returns a pointer to the callee frame slot"
        );
    }

    #[test]
    fn returned_sret_call_local_is_escape_eligible() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn inner(x: Int) -> R { term Ok(x); };\n\
             defn outer(x: Int) -> R { let r: R = inner(x); term r; };\n",
        );
        let plan = build_enum_representation_plan(&items);
        assert!(plan.escaping_return_lets.get("outer").map(|s| s.contains("r")).unwrap_or(false));
    }

    #[test]
    fn returned_match_local_is_escape_eligible() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn inner(x: Int) -> R { term Ok(x); };\n\
             defn outer(x: Int, y: Int) -> R { let r: R = match y { 0 => inner(x), _ => Err(1) }; term r; };\n",
        );
        let plan = build_enum_representation_plan(&items);
        assert!(plan.escaping_return_lets.get("outer").map(|s| s.contains("r")).unwrap_or(false));
    }

    #[test]
    fn alias_and_reassigned_returns_are_not_eligible() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn inner(x: Int) -> R { term Ok(x); };\n\
             defn alias(x: Int) -> R { let r: R = inner(x); let passed: R = r; term passed; };\n\
             defn reassigned(x: Int) -> R { let r: R = inner(x); r = Err(2); term r; };\n",
        );
        let plan = build_enum_representation_plan(&items);
        assert!(!plan.escaping_return_lets.get("alias").map(|s| s.contains("passed")).unwrap_or(false));
        assert!(!plan.escaping_return_lets.get("reassigned").map(|s| s.contains("r")).unwrap_or(false));
    }

    #[test]
    fn non_constructor_return_is_boxed() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn f(x: R) -> R { term x; };\n",
        );
        let plan = build_enum_representation_plan(&items);
        assert_eq!(plan.defn_class("f"), EnumRepresentation::Boxed);
    }

    #[test]
    fn non_enum_return_is_not_in_plan() {
        let items = parse("defn f(x: Int) -> Int { term x; };\n");
        let plan = build_enum_representation_plan(&items);
        assert_eq!(plan.defn_class("f"), EnumRepresentation::Boxed);
    }
}
