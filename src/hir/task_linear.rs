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
        check_list(&c.name, &c.body.statements, &bodies, &mut scope, &mut errors, true);
    }

    let mut top_scope = Scope::default();
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
        check_list(
            "top-level",
            std::slice::from_ref(&stmt),
            &bodies,
            &mut top_scope,
            &mut errors,
            false,
        );
    }
    check_list("top-level", &[], &bodies, &mut top_scope, &mut errors, true);
    errors.sort();
    errors.dedup();
    errors
}

fn check_list(
    owner: &str,
    body: &[HirStmt],
    bodies: &HashMap<&str, &[HirStmt]>,
    scope: &mut Scope,
    errors: &mut Vec<String>,
    is_top: bool,
) {
    for stmt in body {
        match &stmt.kind {
            HirStmtKind::Let { name, expr: Some(e), .. } => {
                if let HirExprKind::Spawn { type_name, .. } = &e.kind {
                    if bodies.contains_key(type_name.as_str()) {
                        scope.live.insert(name.clone(), type_name.clone());
                        continue;
                    }
                }
                if let HirExprKind::Identifier(src) = &e.kind {
                    if let Some(target) = scope.live.remove(src) {
                        scope.live.insert(name.clone(), target);
                        continue;
                    }
                }
                if let HirExprKind::Await(inner) = &e.kind {
                    consume_await(owner, inner, scope, errors);
                }
            }
            HirStmtKind::Assign(lhs, rhs) => {
                if let HirExprKind::Identifier(src) = &rhs.kind {
                    if let HirExprKind::Identifier(name) = &lhs.kind {
                        if let Some(target) = scope.live.remove(src) {
                            scope.live.insert(name.clone(), target);
                            continue;
                        }
                    }
                }
                if let HirExprKind::Await(inner) = &rhs.kind {
                    consume_await(owner, inner, scope, errors);
                }
            }
            HirStmtKind::FreeHint(name) | HirStmtKind::KeepHint(name) => {
                let Some(target) = scope.live.remove(name) else {
                    continue;
                };
                if matches!(stmt.kind, HirStmtKind::FreeHint(_)) && !has_checkpoint(&target, bodies)
                {
                    errors.push(format!(
                        "'{}' frees a task spawned from '{}' but that body has no \
                         cancellation point — add `yield;` where stopping is safe, \
                         or keep/await the handle instead",
                        name, target
                    ));
                }
            }
            HirStmtKind::Expression(e) => {
                if let HirExprKind::Await(inner) = &e.kind {
                    consume_await(owner, inner, scope, errors);
                }
            }
            HirStmtKind::Guarded(_, guarded_body) => {
                check_list(owner, guarded_body, bodies, scope, errors, false);
            }
            HirStmtKind::Block(b) => check_list(owner, b, bodies, scope, errors, false),
            _ => {}
        }
    }
    if is_top {
        let mut dropped: Vec<String> = scope.live.keys().cloned().collect();
        dropped.sort();
        for name in dropped {
            errors.push(format!(
                "'{}' drops a live task handle — await it, or keep/free it before \
                 the end of {}",
                name, owner
            ));
            scope.live.remove(&name);
        }
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
