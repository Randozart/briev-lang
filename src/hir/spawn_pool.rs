// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.5 — spawn-pool capacity analysis over HIR.
//!
//! Mirror of `analysis::spawn_pool` (AST, authoritative until parity).
//! Reuses `SpawnStorage`; `DependentTerm` is HIR-typed because it carries a
//! bound expression.

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::ast::top::{BoundSpec, BoundTerm};
use crate::ast::SpawnStorage;
use std::collections::HashMap;

#[derive(Clone, PartialEq)]
enum Firing {
    Static(i64),
    Dependent(HirExpr),
    Unprovable,
}

#[derive(Clone, Debug)]
pub struct HirDependentTerm {
    pub multiplier: i64,
    pub bound: HirExpr,
}

pub fn analyze(
    program: &HirProgram,
) -> (
    HashMap<String, usize>,
    HashMap<String, Vec<HirDependentTerm>>,
    Vec<String>,
    HashMap<String, SpawnStorage>,
) {
    let mut capacities: HashMap<String, usize> = HashMap::new();
    let mut dependent: HashMap<String, Vec<HirDependentTerm>> = HashMap::new();
    let mut errors: Vec<String> = Vec::new();
    let init_maxes: HashMap<String, i64> = program
        .inits
        .iter()
        .filter_map(|i| i.bound.as_ref().and_then(bound_set_max).map(|m| (i.name.clone(), m)))
        .collect();

    for c in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Defn | HirCallableKind::Transaction)
            && c.owner.is_none()
            && !c.exported
    }) {
        let firing = node_firing(c, &init_maxes);
        let mut live: HashMap<String, i64> = HashMap::new();
        let mut terms: HashMap<String, Vec<HirDependentTerm>> = HashMap::new();
        let mut ctx = WalkCtx { firing: &firing, bound_terms: &[] };
        walk_stmts(&c.body.statements, 1, &mut ctx, &mut live, &mut terms, &mut errors);
        merge_total(&mut capacities, &live);
        for (base, ts) in terms {
            dependent.entry(base).or_default().extend(ts);
        }
    }
    for g in &program.globals {
        let stmt = HirStmt {
            id: g.node,
            span: g.span,
            kind: HirStmtKind::Let {
                name: g.name.clone(),
                ty: g.ty.clone(),
                expr: g.expr.clone(),
            },
        };
        let mut live: HashMap<String, i64> = HashMap::new();
        let mut terms: HashMap<String, Vec<HirDependentTerm>> = HashMap::new();
        let mut ctx = WalkCtx { firing: &Firing::Static(1), bound_terms: &[] };
        walk_stmt(&stmt, 1, &mut ctx, &mut live, &mut terms, &mut errors);
        merge_total(&mut capacities, &live);
        for (base, ts) in terms {
            dependent.entry(base).or_default().extend(ts);
        }
    }

    let storage_classes = collect_storage_classes(program);
    (capacities, dependent, errors, storage_classes)
}

fn collect_storage_classes(program: &HirProgram) -> HashMap<String, SpawnStorage> {
    let mut out = HashMap::new();
    for c in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Defn | HirCallableKind::Transaction)
            && c.owner.is_none()
            && !c.exported
    }) {
        for s in &c.body.statements {
            walk_stmt_for_storage(s, &mut out);
        }
    }
    for g in &program.globals {
        if let Some(e) = &g.expr {
            walk_expr_for_storage(e, &mut out);
        }
    }
    out
}

fn walk_stmt_for_storage(s: &HirStmt, out: &mut HashMap<String, SpawnStorage>) {
    match &s.kind {
        HirStmtKind::Let { expr, .. } => {
            if let Some(e) = expr {
                walk_expr_for_storage(e, out);
            }
        }
        HirStmtKind::Assign(l, r) => {
            walk_expr_for_storage(l, out);
            walk_expr_for_storage(r, out);
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                walk_expr_for_storage(t, out);
            }
            walk_expr_for_storage(value, out);
        }
        HirStmtKind::Term(Some(e)) | HirStmtKind::EndProgram(Some(e)) | HirStmtKind::Tail(e) => {
            walk_expr_for_storage(e, out);
        }
        HirStmtKind::Expression(e) | HirStmtKind::Gate(e) => walk_expr_for_storage(e, out),
        HirStmtKind::Guarded(_, body) => {
            for s in body {
                walk_stmt_for_storage(s, out);
            }
        }
        HirStmtKind::Block(b) | HirStmtKind::SyncBlock(b) => {
            for s in b {
                walk_stmt_for_storage(s, out);
            }
        }
        _ => {}
    }
}

fn walk_expr_for_storage(e: &HirExpr, out: &mut HashMap<String, SpawnStorage>) {
    match &e.kind {
        HirExprKind::Spawn { type_name, storage, .. } if *storage != SpawnStorage::Pooled => {
            out.entry(type_name.clone()).or_insert(*storage);
        }
        HirExprKind::Spawn { args, .. }
        | HirExprKind::Call { args, .. }
        | HirExprKind::List(args)
        | HirExprKind::Tuple(args) => {
            for a in args {
                walk_expr_for_storage(a, out);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            walk_expr_for_storage(left, out);
            walk_expr_for_storage(right, out);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Consume(inner)
        | HirExprKind::IsType { inner, .. } => walk_expr_for_storage(inner, out),
        HirExprKind::Field { receiver, .. }
        | HirExprKind::Index(receiver, _)
        | HirExprKind::Reflect { receiver, .. } => walk_expr_for_storage(receiver, out),
        HirExprKind::MethodCall { receiver, args, .. } => {
            walk_expr_for_storage(receiver, out);
            for a in args {
                walk_expr_for_storage(a, out);
            }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            walk_expr_for_storage(condition, out);
            walk_expr_for_storage(then_branch, out);
            if let Some(e) = else_branch {
                walk_expr_for_storage(e, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            walk_expr_for_storage(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr_for_storage(g, out);
                }
                walk_expr_for_storage(&arm.body, out);
            }
        }
        HirExprKind::Slice { array, start, end, stride } => {
            walk_expr_for_storage(array, out);
            for b in [start, end, stride].into_iter().flatten() {
                walk_expr_for_storage(b, out);
            }
        }
        HirExprKind::StructLiteral { fields, .. } => {
            for (_, f) in fields {
                walk_expr_for_storage(f, out);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            walk_expr_for_storage(start, out);
            walk_expr_for_storage(end, out);
        }
        HirExprKind::Block(stmts) => {
            for s in stmts {
                walk_stmt_for_storage(s, out);
            }
        }
        _ => {}
    }
}

struct WalkCtx<'a> {
    firing: &'a Firing,
    bound_terms: &'a [HirExpr],
}

fn node_firing(
    c: &super::program::HirCallable,
    init_maxes: &HashMap<String, i64>,
) -> Firing {
    if !c.is_reactive {
        return Firing::Static(1);
    }
    let Some(pre) = c.contract.as_ref().map(|ct| &ct.pre) else {
        return Firing::Unprovable;
    };
    match &pre.kind {
        HirExprKind::Literal(super::expr::HirLiteral::Bool(true)) => Firing::Unprovable,
        HirExprKind::Binary {
            op: crate::ast::BinaryOpKind::Lt | crate::ast::BinaryOpKind::Le,
            right,
            ..
        } => match &right.kind {
            HirExprKind::Literal(super::expr::HirLiteral::Int(n)) if *n >= 0 => {
                let inclusive = matches!(pre.kind, HirExprKind::Binary {
                    op: crate::ast::BinaryOpKind::Le, ..
                });
                Firing::Static(if inclusive { n + 1 } else { *n })
            }
            HirExprKind::Identifier(name) => match init_maxes.get(name) {
                Some(&max) => Firing::Static(max),
                None => Firing::Dependent((**right).clone()),
            },
            _ => Firing::Unprovable,
        },
        _ => Firing::Unprovable,
    }
}

fn bound_set_max(bound: &BoundSpec) -> Option<i64> {
    match bound {
        BoundSpec::Single(term) => bound_term_value(term),
        BoundSpec::Range(_, hi) => bound_term_value(hi),
        BoundSpec::Choice(parts) => parts
            .iter()
            .map(bound_set_max)
            .try_fold(i64::MIN, |acc, v| v.map(|v| acc.max(v))),
    }
}

fn bound_term_value(term: &BoundTerm) -> Option<i64> {
    match term {
        BoundTerm::Lit(n) => Some(*n),
        BoundTerm::Ref(_) => None,
    }
}

fn foreach_bound(list: &HirExpr) -> Option<HirExpr> {
    match &list.kind {
        HirExprKind::Range { start, end, .. } => match (&start.kind, &end.kind) {
            (
                HirExprKind::Literal(super::expr::HirLiteral::Int(_)),
                HirExprKind::Literal(super::expr::HirLiteral::Int(_)),
            ) => None,
            (_, HirExprKind::Identifier(_) | HirExprKind::Field { .. }) => Some((**end).clone()),
            _ => None,
        },
        _ => None,
    }
}

fn walk_stmts(
    stmts: &[HirStmt],
    multiplier: i64,
    ctx: &mut WalkCtx,
    live: &mut HashMap<String, i64>,
    terms: &mut HashMap<String, Vec<HirDependentTerm>>,
    errors: &mut Vec<String>,
) {
    for s in stmts {
        walk_stmt(s, multiplier, ctx, live, terms, errors);
    }
}

fn walk_stmt(
    stmt: &HirStmt,
    multiplier: i64,
    ctx: &mut WalkCtx,
    live: &mut HashMap<String, i64>,
    terms: &mut HashMap<String, Vec<HirDependentTerm>>,
    errors: &mut Vec<String>,
) {
    match &stmt.kind {
        HirStmtKind::Foreach { list, body, .. } => {
            let count = match &list.kind {
                HirExprKind::Range { start, end, inclusive } => {
                    match (&start.kind, &end.kind) {
                        (
                            HirExprKind::Literal(super::expr::HirLiteral::Int(s)),
                            HirExprKind::Literal(super::expr::HirLiteral::Int(e)),
                        ) if *s <= *e => Some((if *inclusive { e - s + 1 } else { e - s }).max(0)),
                        _ => None,
                    }
                }
                HirExprKind::List(elems) => Some(elems.len() as i64),
                _ => None,
            };
            match count {
                Some(c) => walk_stmts(body, multiplier * c, ctx, live, terms, errors),
                None => {
                    if let Some(bound) = foreach_bound(list) {
                        let nested = ctx.bound_terms;
                        let extended: Vec<HirExpr> = nested
                            .iter()
                            .cloned()
                            .chain(std::iter::once(bound))
                            .collect();
                        let mut inner_ctx = WalkCtx { firing: ctx.firing, bound_terms: &extended };
                        walk_stmts(body, multiplier, &mut inner_ctx, live, terms, errors);
                    } else {
                        let mut inner_live: HashMap<String, i64> = HashMap::new();
                        let mut inner_terms: HashMap<String, Vec<HirDependentTerm>> =
                            HashMap::new();
                        let mut inner_errors = Vec::new();
                        let extended: Vec<HirExpr> = ctx.bound_terms.to_vec();
                        let mut inner_ctx = WalkCtx { firing: ctx.firing, bound_terms: &extended };
                        walk_stmts(
                            body,
                            0,
                            &mut inner_ctx,
                            &mut inner_live,
                            &mut inner_terms,
                            &mut inner_errors,
                        );
                        for e in inner_errors {
                            errors.push(e);
                        }
                        for (base, ts) in inner_terms {
                            terms.entry(base).or_default().extend(ts);
                        }
                    }
                }
            }
        }
        HirStmtKind::Guarded(_, body) => walk_stmts(body, multiplier, ctx, live, terms, errors),
        HirStmtKind::Block(body) | HirStmtKind::SyncBlock(body) => {
            walk_stmts(body, multiplier, ctx, live, terms, errors)
        }
        HirStmtKind::Assign(_, rhs) => walk_expr(rhs, multiplier, ctx, live, terms, errors),
        HirStmtKind::Term(Some(e)) | HirStmtKind::EndProgram(Some(e)) | HirStmtKind::Tail(e) => {
            walk_expr(e, multiplier, ctx, live, terms, errors)
        }
        HirStmtKind::Expression(e) | HirStmtKind::Gate(e) => {
            walk_expr(e, multiplier, ctx, live, terms, errors)
        }
        HirStmtKind::FreeHint(_) | HirStmtKind::KeepHint(_) => {}
        HirStmtKind::Let { expr: Some(e), .. } => {
            walk_expr(e, multiplier, ctx, live, terms, errors)
        }
        _ => {}
    }
}

fn walk_expr(
    expr: &HirExpr,
    multiplier: i64,
    ctx: &mut WalkCtx,
    live: &mut HashMap<String, i64>,
    terms: &mut HashMap<String, Vec<HirDependentTerm>>,
    errors: &mut Vec<String>,
) {
    match &expr.kind {
        HirExprKind::Spawn { type_name, args, storage } => {
            if *storage != SpawnStorage::Pooled {
                for a in args {
                    walk_expr(a, multiplier, ctx, live, terms, errors);
                }
                return;
            }
            match ctx.firing {
                Firing::Static(n) if ctx.bound_terms.is_empty() => {
                    let entry = live.entry(type_name.clone()).or_insert(0);
                    *entry += multiplier * (*n).max(1);
                }
                Firing::Static(n) => {
                    let bound = product(ctx.bound_terms);
                    let entry = terms.entry(type_name.clone()).or_default();
                    entry.push(HirDependentTerm {
                        multiplier: multiplier * (*n).max(1),
                        bound,
                    });
                }
                Firing::Dependent(countdown_bound) => {
                    let mut bounds: Vec<HirExpr> =
                        Vec::with_capacity(ctx.bound_terms.len() + 1);
                    bounds.push(countdown_bound.clone());
                    bounds.extend(ctx.bound_terms.iter().cloned());
                    let bound = product(&bounds);
                    let entry = terms.entry(type_name.clone()).or_default();
                    entry.push(HirDependentTerm { multiplier, bound });
                }
                Firing::Unprovable => {
                    errors.push(format!(
                        "spawn of '{}' is not statically bounded — the pool must be predictably \
                         inexhaustible; spawn inside a countdown node with a compile-time constant \
                         or runtime-field bound",
                        type_name
                    ));
                }
            }
            for a in args {
                walk_expr(a, multiplier, ctx, live, terms, errors);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            walk_expr(left, multiplier, ctx, live, terms, errors);
            walk_expr(right, multiplier, ctx, live, terms, errors);
        }
        HirExprKind::Call { args, .. } | HirExprKind::List(args) | HirExprKind::Tuple(args) => {
            for a in args {
                walk_expr(a, multiplier, ctx, live, terms, errors);
            }
        }
        HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Consume(inner)
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner) => {
            walk_expr(inner, multiplier, ctx, live, terms, errors)
        }
        HirExprKind::Field { receiver, .. }
        | HirExprKind::Index(receiver, _)
        | HirExprKind::Reflect { receiver, .. } => {
            walk_expr(receiver, multiplier, ctx, live, terms, errors)
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            walk_expr(receiver, multiplier, ctx, live, terms, errors);
            for a in args {
                walk_expr(a, multiplier, ctx, live, terms, errors);
            }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            walk_expr(condition, multiplier, ctx, live, terms, errors);
            walk_expr(then_branch, multiplier, ctx, live, terms, errors);
            if let Some(e) = else_branch {
                walk_expr(e, multiplier, ctx, live, terms, errors);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, multiplier, ctx, live, terms, errors);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr(g, multiplier, ctx, live, terms, errors);
                }
                walk_expr(&arm.body, multiplier, ctx, live, terms, errors);
            }
        }
        HirExprKind::Slice { array, start, end, stride } => {
            walk_expr(array, multiplier, ctx, live, terms, errors);
            for b in [start, end, stride].into_iter().flatten() {
                walk_expr(b, multiplier, ctx, live, terms, errors);
            }
        }
        HirExprKind::StructLiteral { fields, .. } => {
            for (_, f) in fields {
                walk_expr(f, multiplier, ctx, live, terms, errors);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            walk_expr(start, multiplier, ctx, live, terms, errors);
            walk_expr(end, multiplier, ctx, live, terms, errors);
        }
        _ => {}
    }
}

fn product(bounds: &[HirExpr]) -> HirExpr {
    if bounds.is_empty() {
        return HirExpr {
            id: super::ids::NodeId(0),
            span: None,
            ty: None,
            kind: HirExprKind::Literal(super::expr::HirLiteral::Int(1)),
        };
    }
    let mut acc = bounds[0].clone();
    for b in &bounds[1..] {
        let kind = HirExprKind::Binary {
            op: crate::ast::BinaryOpKind::Mul,
            left: Box::new(std::mem::replace(
                &mut acc,
                HirExpr {
                    id: super::ids::NodeId(0),
                    span: None,
                    ty: None,
                    kind: HirExprKind::Other("temporary"),
                },
            )),
            right: Box::new(b.clone()),
        };
        acc = HirExpr { id: super::ids::NodeId(0), span: None, ty: None, kind };
    }
    acc
}

fn merge_total(out: &mut HashMap<String, usize>, live: &HashMap<String, i64>) {
    for (base, n) in live {
        let entry = out.entry(base.clone()).or_insert(0);
        *entry += (*n as usize).max(1);
    }
}
