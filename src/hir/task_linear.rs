// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.5 — task-handle linearity over HIR.
//! Mirror of `analysis::task_linear` (AST, authoritative until parity).

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Scope {
    live: HashMap<String, String>,
    tombstones: HashSet<String>,
}

pub fn analyze(program: &HirProgram) -> Vec<String> {
    let mut errors = Vec::new();
    let bodies: HashMap<&str, &[HirStmt]> = program
        .callables
        .iter()
        .filter(|c| {
            matches!(c.kind, HirCallableKind::Defn | HirCallableKind::Transaction)
                && c.owner.is_none()
                && !c.exported
        })
        .map(|c| (c.name.as_str(), c.body.statements.as_slice()))
        .collect();

    for c in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Defn | HirCallableKind::Transaction)
            && c.owner.is_none()
            && !c.exported
    }) {
        let mut scope = Scope::default();
        let ctx = CheckCtx { owner: &c.name, bodies: &bodies };
        check_list(&ctx, &c.body.statements, &mut scope, &mut errors, true);
    }

    let mut top_scope = Scope::default();
    for g in &program.globals {
        let stmt = HirStmt {
            id: g.node,
            span: g.span,
            kind: HirStmtKind::Let {
                name: g.name.clone(),
                names: vec![g.name.clone()],
                ty: g.ty.clone(),
                expr: g.expr.clone(),
                modifiers: g.modifiers.iter().map(|m| crate::ast::Annotation { name: m.clone(), value: None }).collect(),
            },
        };
        let ctx = CheckCtx { owner: "top-level", bodies: &bodies };
        check_list(&ctx, std::slice::from_ref(&stmt), &mut top_scope, &mut errors, false);
    }
    let ctx = CheckCtx { owner: "top-level", bodies: &bodies };
    check_list(&ctx, &[], &mut top_scope, &mut errors, true);
    errors.sort();
    errors.dedup();
    errors
}

/// Shared context for the handle-linearity walk.
struct CheckCtx<'a> {
    owner: &'a str,
    bodies: &'a HashMap<&'a str, &'a [HirStmt]>,
}

fn check_list(
    ctx: &CheckCtx<'_>,
    body: &[HirStmt],
    scope: &mut Scope,
    errors: &mut Vec<String>,
    is_top: bool,
) {
    for stmt in body {
        check_stmt(ctx, stmt, scope, errors);
    }
    if is_top {
        report_drops(ctx, scope, errors);
    }
}

fn check_stmt(
    ctx: &CheckCtx<'_>,
    stmt: &HirStmt,
    scope: &mut Scope,
    errors: &mut Vec<String>,
) {
    match &stmt.kind {
        HirStmtKind::Let { name, expr: Some(e), .. } => {
            check_let(ctx, name, e, scope, errors)
        }
        HirStmtKind::Assign(lhs, rhs) => check_assign(ctx, lhs, rhs, scope, errors),
        HirStmtKind::FreeHint(name) | HirStmtKind::KeepHint(name) => {
            check_hint(ctx, name, matches!(stmt.kind, HirStmtKind::FreeHint(_)), scope, errors)
        }
        HirStmtKind::Expression(e) => {
            if let HirExprKind::Await(inner) = &e.kind {
                consume_await(ctx.owner, inner, scope, errors);
            }
        }
        HirStmtKind::Guarded(_, guarded_body) => {
            check_list(ctx, guarded_body, scope, errors, false);
        }
        HirStmtKind::Block(b) => check_list(ctx, b, scope, errors, false),
        _ => {}
    }
}

/// `let name = …` — spawn creates a handle; moving an identifier transfers it;
/// await consumes it.
fn check_let(
    ctx: &CheckCtx<'_>,
    name: &str,
    e: &HirExpr,
    scope: &mut Scope,
    errors: &mut Vec<String>,
) {
    if let HirExprKind::Spawn { type_name, .. } = &e.kind {
        if ctx.bodies.contains_key(type_name.as_str()) {
            scope.live.insert(name.to_string(), type_name.clone());
            return;
        }
    }
    if let HirExprKind::Identifier(src) = &e.kind {
        if let Some(target) = scope.live.remove(src) {
            scope.live.insert(name.to_string(), target);
            return;
        }
    }
    if let HirExprKind::Await(inner) = &e.kind {
        consume_await(ctx.owner, inner, scope, errors);
    }
}

/// `name = …` — handle transfer or await-consume.
fn check_assign(
    ctx: &CheckCtx<'_>,
    lhs: &HirExpr,
    rhs: &HirExpr,
    scope: &mut Scope,
    errors: &mut Vec<String>,
) {
    if let HirExprKind::Identifier(src) = &rhs.kind {
        if let HirExprKind::Identifier(name) = &lhs.kind {
            if let Some(target) = scope.live.remove(src) {
                scope.live.insert(name.clone(), target);
                return;
            }
        }
    }
    if let HirExprKind::Await(inner) = &rhs.kind {
        consume_await(ctx.owner, inner, scope, errors);
    }
}

/// `free name;` / `keep name;` — a free of a handle whose target has no
/// cancellation point is an error.
fn check_hint(
    ctx: &CheckCtx<'_>,
    name: &str,
    is_free: bool,
    scope: &mut Scope,
    errors: &mut Vec<String>,
) {
    let Some(target) = scope.live.remove(name) else {
        return;
    };
    if is_free && !has_checkpoint(&target, ctx.bodies) {
        errors.push(format!(
            "'{}' frees a task spawned from '{}' but that body has no              cancellation point — add `yield;` where stopping is safe,              or keep/await the handle instead",
            name, target
        ));
    }
}

/// Top-level scope exit: every still-live handle is dropped.
fn report_drops(ctx: &CheckCtx<'_>, scope: &mut Scope, errors: &mut Vec<String>) {
    let mut dropped: Vec<String> = scope.live.keys().cloned().collect();
    dropped.sort();
    for name in dropped {
        errors.push(format!(
            "'{}' drops a live task handle — await it, or keep/free it before              the end of {}",
            name, ctx.owner
        ));
        scope.live.remove(&name);
    }
}

fn consume_await(owner: &str, e: &HirExpr, scope: &mut Scope, errors: &mut Vec<String>) {
    if let HirExprKind::Identifier(name) = &e.kind {
        if scope.live.remove(name).is_some() {
            scope.tombstones.insert(name.clone());
        } else if scope.tombstones.contains(name) {
            errors.push(format!(
                "'{}' uses a task handle after it was consumed in {}",
                name, owner
            ));
        }
    }
}

fn has_checkpoint(fn_name: &str, bodies: &HashMap<&str, &[HirStmt]>) -> bool {
    let Some(body) = bodies.get(fn_name) else {
        return false;
    };
    fn walk(body: &[HirStmt]) -> bool {
        body.iter().any(|stmt| match &stmt.kind {
            HirStmtKind::Yield
            | HirStmtKind::Term(_)
            | HirStmtKind::EndProgram(_)
            | HirStmtKind::Tail(_) => true,
            HirStmtKind::Guarded(_, b) | HirStmtKind::Block(b) => walk(b),
            _ => false,
        })
    }
    walk(body)
}
