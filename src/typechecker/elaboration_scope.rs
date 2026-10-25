//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! 2026-10-22: retain pattern payload types while elaborating nested operators.
//! Authored by Otto Westerlund (@odecode); adopted with attribution.
//! The normal checker validates coverage; this pass consumes its enum slots
//! and generic substitutions so nested loops see the same lexical bindings.
use super::*;

pub(super) fn bindings(pattern: &Pattern, matched: &Type, ctx: &TypecheckContext) -> Vec<(String, Type)> {
    match pattern {
        Pattern::Binding(name) => vec![(name.clone(), matched.clone())],
        Pattern::TypedBinding(name, ty) => vec![(name.clone(), (**ty).clone())],
        Pattern::EnumVariant(name, patterns) => variant_bindings(name, patterns, matched, ctx),
        _ => Vec::new(),
    }
}

fn variant_bindings(name: &str, patterns: &[Pattern], matched: &Type, ctx: &TypecheckContext) -> Vec<(String, Type)> {
    let Some(base) = matched.universe_key() else { return Vec::new(); };
    let bare = name.rsplit("::").next().unwrap_or(name);
    let slot_name = format!("__variant_{bare}");
    let Some(slot) = ctx.type_slots.get(base).and_then(|slots| slots.iter().find(|slot| slot.name == slot_name)) else { return Vec::new(); };
    let payload = match &slot.ty { Type::Tuple(types) => types.clone(), ty => vec![ty.clone()] };
    let params = ctx.type_params.get(base).cloned().unwrap_or_default();
    let args = match matched { Type::Applied(_, args) => args.clone(), _ => Vec::new() };
    let subst: HashMap<String, Type> = params.into_iter().zip(args).collect();
    patterns.iter().zip(payload).flat_map(|(pattern, ty)| bindings(pattern, &substitute_type(&ty, &subst), ctx)).collect()
}

pub(super) fn expression_match(scrutinee: &mut Expr, arms: &mut [MatchArm], ctx: &mut TypecheckContext, errors: &mut Vec<TypeError>) {
    elaborate_expr(scrutinee, ctx, errors);
    let matched = match infer_type_only(scrutinee, ctx) { Ok(ty) => ty, Err(error) => { errors.push(error); return; } };
    for arm in arms {
        let saved = ctx.bindings.clone();
        let facts = ctx.known_facts.clone();
        let payload_bindings = bindings(&arm.pattern, &matched, ctx);
        ctx.bindings.extend(payload_bindings);
        if let Some(guard) = &mut arm.guard { elaborate_expr(guard, ctx, errors); }
        elaborate_expr(&mut arm.body, ctx, errors);
        ctx.bindings = saved;
        ctx.known_facts = facts;
    }
}
