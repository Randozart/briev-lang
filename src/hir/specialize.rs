// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.6 — generic callable-txn specialization by HIR substitution.
//!
//! Produces a concrete instantiation of a polymorphic `defn`/`txn` from the
//! HIR type-parameter list: every type slot is substituted, every node gets a
//! fresh stable id, and call sites named in `renames` are retargeted at the
//! peer instantiations. No consumer is switched yet (H4 gates that on
//! byte-identical IR); this is the frontend primitive the backend's ad-hoc
//! monomorphization will be replaced with.

use super::expr::{HirExpr, HirExprKind};
use super::ids::IdSupply;
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{HirCallable, HirOutput, HirParam};
use super::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};
use crate::ast::Type;
use crate::typechecker::substitute_type;
use std::collections::HashMap;

/// Build the `{type-param → concrete}` map for `type_args`, or `None` when the
/// arity does not match the callable's declared type parameters.
pub fn substitution_for(c: &HirCallable, type_args: &[Type]) -> Option<HashMap<String, Type>> {
    if c.type_params.len() != type_args.len() {
        return None;
    }
    Some(
        c.type_params
            .iter()
            .cloned()
            .zip(type_args.iter().cloned())
            .collect(),
    )
}

/// Identifier-safe instantiation symbol, shared with backend emission
/// (`analysis::generic_specialization`, PR #62). Do not re-derive the mangling:
/// analysis and codegen must agree on the concrete callable identity.
pub fn mangled_instantiation_name(name: &str, type_args: &[Type]) -> String {
    crate::analysis::generic_specialization::mangle_specialization(name, type_args)
}

/// Bind the callable's type parameters from a concrete call's argument types
/// (declared param type vs concrete arg type), using the shared unifier.
/// Returns `None` when a declared type param remains unbound — an under-
/// constrained call is not silently specialized to the wrong type.
pub fn bind_from_arguments(
    c: &HirCallable,
    arg_types: &[Type],
) -> Option<HashMap<String, Type>> {
    if c.type_params.is_empty() || c.params.len() != arg_types.len() {
        return None;
    }
    let mut subst = HashMap::new();
    for (param, arg) in c.params.iter().zip(arg_types.iter()) {
        crate::analysis::generic_specialization::unify_generic_binding(
            &param.ty,
            arg,
            &c.type_params,
            &mut subst,
        );
    }
    c.type_params
        .iter()
        .all(|p| subst.contains_key(p))
        .then_some(subst)
}

/// Convenience wrapper without call-site retargeting.
pub fn specialize_callable(
    c: &HirCallable,
    type_args: &[Type],
    name: &str,
    ids: &mut IdSupply,
) -> Option<HirCallable> {
    specialize_callable_with_renames(c, type_args, name, &HashMap::new(), ids)
}

/// Specialize `c` for `type_args` under `name`, retargeting `Call` callees in
/// `renames` at their peer instantiations. Returns `None` on arity mismatch.
pub fn specialize_callable_with_renames(
    c: &HirCallable,
    type_args: &[Type],
    name: &str,
    renames: &HashMap<String, String>,
    ids: &mut IdSupply,
) -> Option<HirCallable> {
    let subst = substitution_for(c, type_args)?;
    specialize_with_substitution(c, &subst, name, renames, ids)
}

/// 2026-11-05 (H3.6 backend wiring): specialize from an explicit
/// `{type-param → concrete}` map — the form the backend's specialization
/// queue carries. Arity-independent; a param absent from the map keeps its
/// declared (generic) type.
pub fn specialize_with_substitution(
    c: &HirCallable,
    subst: &HashMap<String, Type>,
    name: &str,
    renames: &HashMap<String, String>,
    ids: &mut IdSupply,
) -> Option<HirCallable> {
    if c.type_params.is_empty() && !subst.is_empty() {
        // A monomorphic callable has nothing to substitute; the caller
        // constructing a subst for it is a dispatch bug.
        return None;
    }
    let mut out = c.clone();
    out.id = ids.def();
    out.item = ids.item();
    out.node = ids.node();
    out.name = name.to_string();
    out.type_params.clear();
    out.exported = false;
    out.export_symbol = None;
    out.params = c
        .params
        .iter()
        .map(|p| HirParam {
            name: p.name.clone(),
            ty: substitute_type(&p.ty, &subst),
        })
        .collect();
    out.result = c.result.as_ref().map(|t| substitute_type(t, &subst));
    out.output_list = c
        .output_list
        .iter()
        .map(|t| substitute_type(t, &subst))
        .collect();
    out.outputs = c.outputs.as_ref().map(|o| subst_output(o, &subst));
    if let Some(ct) = out.contract.as_mut() {
        subst_expr(&mut ct.pre, &subst, renames, ids);
        subst_expr(&mut ct.post, &subst, renames, ids);
        if let Some(wd) = ct.watchdog.as_mut() {
            subst_expr(&mut wd.condition, &subst, renames, ids);
            if let Some(fb) = wd.fallback.as_mut() {
                subst_expr(fb, &subst, renames, ids);
            }
        }
    }
    out.body.node = ids.node();
    for stmt in &mut out.body.statements {
        subst_stmt(stmt, &subst, renames, ids);
    }
    Some(out)
}

fn subst_output(o: &HirOutput, subst: &HashMap<String, Type>) -> HirOutput {
    match o {
        HirOutput::Single(t) => HirOutput::Single(substitute_type(t, subst)),
        HirOutput::Union(v) => HirOutput::Union(v.iter().map(|x| subst_output(x, subst)).collect()),
        HirOutput::Tuple(v) => HirOutput::Tuple(v.iter().map(|x| subst_output(x, subst)).collect()),
        HirOutput::Array(inner) => HirOutput::Array(Box::new(subst_output(inner, subst))),
        HirOutput::Named(n, inner) => HirOutput::Named(n.clone(), Box::new(subst_output(inner, subst))),
    }
}

fn subst_stmt(
    s: &mut HirStmt,
    subst: &HashMap<String, Type>,
    renames: &HashMap<String, String>,
    ids: &mut IdSupply,
) {
    s.id = ids.node();
    match &mut s.kind {
        HirStmtKind::Let { ty, expr, .. } => {
            if let Some(t) = ty {
                *t = substitute_type(t, subst);
            }
            if let Some(e) = expr {
                subst_expr(e, subst, renames, ids);
            }
        }
        HirStmtKind::Assign(l, r) => {
            subst_expr(l, subst, renames, ids);
            subst_expr(r, subst, renames, ids);
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                subst_expr(t, subst, renames, ids);
            }
            subst_expr(value, subst, renames, ids);
        }
        HirStmtKind::Term(Some(e))
        | HirStmtKind::Tail(e)
        | HirStmtKind::Check(e)
        | HirStmtKind::Expression(e)
        | HirStmtKind::EndProgram(Some(e))
        | HirStmtKind::Gate(e)
        | HirStmtKind::Rollback(Some(e))
        | HirStmtKind::TrgBinding { instance: e, .. } => subst_expr(e, subst, renames, ids),
        HirStmtKind::Open(l, r) => {
            subst_expr(l, subst, renames, ids);
            subst_expr(r, subst, renames, ids);
        }
        HirStmtKind::Guarded(c, body) => {
            subst_expr(c, subst, renames, ids);
            for stmt in body {
                subst_stmt(stmt, subst, renames, ids);
            }
        }
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => {
            for stmt in body {
                subst_stmt(stmt, subst, renames, ids);
            }
        }
        HirStmtKind::Foreach { list, body, .. } => {
            subst_expr(list, subst, renames, ids);
            for stmt in body {
                subst_stmt(stmt, subst, renames, ids);
            }
        }
        HirStmtKind::Match { expr, arms } => {
            subst_expr(expr, subst, renames, ids);
            for arm in arms {
                subst_match_arm(arm, subst, renames, ids);
            }
        }
        _ => {}
    }
}

fn subst_match_arm(
    arm: &mut HirStmtMatchArm,
    subst: &HashMap<String, Type>,
    renames: &HashMap<String, String>,
    ids: &mut IdSupply,
) {
    for pat in &mut arm.patterns {
        subst_pattern(pat, subst, renames, ids);
    }
    for stmt in &mut arm.body {
        subst_stmt(stmt, subst, renames, ids);
    }
}

fn subst_pattern(
    p: &mut HirPattern,
    subst: &HashMap<String, Type>,
    renames: &HashMap<String, String>,
    ids: &mut IdSupply,
) {
    match &mut p.kind {
        HirPatternKind::Literal(e) | HirPatternKind::Range(e, _) | HirPatternKind::RangeInclusive(e, _) => {
            subst_expr(e, subst, renames, ids);
            if let HirPatternKind::Range(_, r) | HirPatternKind::RangeInclusive(_, r) = &mut p.kind {
                subst_expr(r, subst, renames, ids);
            }
        }
        HirPatternKind::TypedBinding(_, ty) => *ty = substitute_type(ty, subst),
        HirPatternKind::EnumVariant(_, pats)
        | HirPatternKind::Tuple(pats)
        | HirPatternKind::Multi(pats) => {
            for sub in pats {
                subst_pattern(sub, subst, renames, ids);
            }
        }
        _ => {}
    }
}

fn subst_expr(
    e: &mut HirExpr,
    subst: &HashMap<String, Type>,
    renames: &HashMap<String, String>,
    ids: &mut IdSupply,
) {
    e.id = ids.node();
    if let Some(t) = e.ty.as_mut() {
        *t = substitute_type(t, subst);
    }
    match &mut e.kind {
        HirExprKind::Call { callee, args, analysis_id } => {
            if let Some(new_name) = renames.get(callee) {
                *callee = new_name.clone();
            }
            let _ = analysis_id;
            for a in args {
                subst_expr(a, subst, renames, ids);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            subst_expr(left, subst, renames, ids);
            subst_expr(right, subst, renames, ids);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => subst_expr(inner, subst, renames, ids),
        HirExprKind::Cast { inner, to, .. } => {
            *to = substitute_type(to, subst);
            subst_expr(inner, subst, renames, ids);
        }
        HirExprKind::IsType { inner, ty } => {
            *ty = substitute_type(ty, subst);
            subst_expr(inner, subst, renames, ids);
        }
        HirExprKind::Field { receiver, .. } => subst_expr(receiver, subst, renames, ids),
        HirExprKind::MethodCall { receiver, args, .. } => {
            subst_expr(receiver, subst, renames, ids);
            for a in args {
                subst_expr(a, subst, renames, ids);
            }
        }
        HirExprKind::Reflect { receiver, .. } => subst_expr(receiver, subst, renames, ids),
        HirExprKind::Index(a, b) => {
            subst_expr(a, subst, renames, ids);
            subst_expr(b, subst, renames, ids);
        }
        HirExprKind::Slice { array, start, end, stride } => {
            subst_expr(array, subst, renames, ids);
            for opt in [start, end, stride] {
                if let Some(x) = opt {
                    subst_expr(x, subst, renames, ids);
                }
            }
        }
        HirExprKind::Range { start, end, .. } => {
            subst_expr(start, subst, renames, ids);
            subst_expr(end, subst, renames, ids);
        }
        HirExprKind::Spawn { args, .. } => {
            for a in args {
                subst_expr(a, subst, renames, ids);
            }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            subst_expr(condition, subst, renames, ids);
            subst_expr(then_branch, subst, renames, ids);
            if let Some(e) = else_branch {
                subst_expr(e, subst, renames, ids);
            }
        }
        HirExprKind::Lambda { body, .. } => subst_expr(body, subst, renames, ids),
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            for i in items {
                subst_expr(i, subst, renames, ids);
            }
        }
        HirExprKind::StructLiteral { fields, .. } => {
            for (_, e) in fields {
                subst_expr(e, subst, renames, ids);
            }
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            for a in args {
                subst_expr(a, subst, renames, ids);
            }
            if let Some(r) = receiver {
                subst_expr(r, subst, renames, ids);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            subst_expr(scrutinee, subst, renames, ids);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    subst_expr(g, subst, renames, ids);
                }
                subst_expr(&mut arm.body, subst, renames, ids);
            }
        }
        HirExprKind::Block(stmts) => {
            for s in stmts {
                subst_stmt(s, subst, renames, ids);
            }
        }
        _ => {}
    }
}
