// ── Backend Normalizer — Shared Helpers ───────────────────────────────
// 2026-07-14: Walks the AST and attaches backend-specific annotations.
// Shared across all backend normalizers. Max 2 nesting depth.

use std::collections::HashSet;
use crate::ast::*;

/// A collected intrinsic call from the AST.
#[derive(Debug, Clone)]
pub struct IntrinsicCall {
    pub name: String,
}

/// Walk the AST and collect all Expr::Call where name ends with '#'.
pub fn collect_intrinsic_calls(items: &[TopLevel]) -> Vec<IntrinsicCall> {
    let mut calls = Vec::new();
    for item in items {
        walk_toplevel(item, &mut |e| {
            if let Expr::Call(name, _, _) = e {
                if name.ends_with('#') {
                    calls.push(IntrinsicCall { name: name.clone() });
                }
            }
        });
    }
    calls
}

/// Validate that every intrinsic call in the REACHABLE program surface is in
/// the supported set.
///
/// 2026-09-09 (Family A/B, briev-native runtime): reachability-closed. The
/// prelude (prelude-native) injects cast_lanes.bv + float_fmt.bv into every
/// `.bv` unit; their defn bodies carry `Load#`/`Store#`/`SysCall#` that a
/// GPU/webstack unit never calls. Walking ALL items rejected uncalled
/// library code and failed every offload program. Dead code is not the
/// program (observability doctrine): the surface is txn bodies, exports,
/// constants, and ISR bodies, closed over transitively-called defns.
pub fn validate_intrinsics<'a>(
    items: &'a [TopLevel],
    supported: &HashSet<String>,
) -> Vec<String> {
    // Defn bodies by name, for call-graph expansion.
    let mut bodies: std::collections::HashMap<&'a str, &'a [Statement]> =
        std::collections::HashMap::new();
    for item in items {
        if let TopLevel::Definition(d) = item {
            bodies.insert(d.name.as_str(), d.body.as_slice());
        }
    }

    let mut intrinsics: Vec<String> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut work: Vec<&[Statement]> = Vec::new();

    // Seed with the program surface and harvest its first hop.
    for item in items {
        match item {
            TopLevel::Transaction(t) => work.push(t.body.as_slice()),
            TopLevel::IsrHandler(h) => work.push(h.body.as_slice()),
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    work.push(d.body.as_slice());
                }
            }
            TopLevel::Constant(c) => {
                harvest_expr(&c.expr, &mut intrinsics, &mut visited, &bodies, &mut work);
            }
            _ => {}
        }
    }

    // BFS: expand defn calls until fixpoint.
    while let Some(body) = work.pop() {
        harvest_body(body, &mut intrinsics, &mut visited, &bodies, &mut work);
    }

    let mut errors = Vec::new();
    for call in &intrinsics {
        if !supported.contains(call) {
            errors.push(format!("intrinsic '{}' is not supported by this backend", call));
        }
    }
    errors
}

/// Harvest one statement body: record intrinsic calls, enqueue uncalled-yet
/// defn bodies.
fn harvest_body<'a>(
    stmts: &'a [Statement],
    intrinsics: &mut Vec<String>,
    visited: &mut HashSet<String>,
    bodies: &std::collections::HashMap<&'a str, &'a [Statement]>,
    work: &mut Vec<&'a [Statement]>,
) {
    for stmt in stmts {
        match stmt {
            Statement::Assign(lhs, rhs) => {
                harvest_expr(lhs, intrinsics, visited, bodies, work);
                harvest_expr(rhs, intrinsics, visited, bodies, work);
            }
            Statement::Expression(e) | Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => {
                harvest_expr(e, intrinsics, visited, bodies, work);
            }
            Statement::Guarded(_, body) | Statement::Block(body) => {
                harvest_body(body, intrinsics, visited, bodies, work);
            }
            _ => {}
        }
    }
}

fn harvest_expr<'a>(
    e: &'a Expr,
    intrinsics: &mut Vec<String>,
    visited: &mut HashSet<String>,
    bodies: &std::collections::HashMap<&'a str, &'a [Statement]>,
    work: &mut Vec<&'a [Statement]>,
) {
    if let Expr::Call(name, _, _) = e {
        if name.ends_with('#') {
            intrinsics.push(name.clone());
        } else if !visited.contains(name) {
            if let Some(body) = bodies.get(name.as_str()) {
                visited.insert(name.clone());
                work.push(*body);
            }
        }
    }
}

/// Walk a TopLevel item, calling `f` on every Expr encountered.
fn walk_toplevel<F>(item: &TopLevel, f: &mut F)
where F: FnMut(&Expr) {
    match item {
        TopLevel::Definition(d) => walk_statements(&d.body, f),
        TopLevel::Transaction(t) => walk_statements(&t.body, f),
        TopLevel::StateDecl(_) | TopLevel::Trigger(_) => {}
        TopLevel::Constant(c) => f(&c.expr),
        _ => {}
    }
}

/// Walk a list of Statements, calling `f` on every Expr encountered.
fn walk_statements<F>(stmts: &[Statement], f: &mut F)
where F: FnMut(&Expr) {
    for stmt in stmts {
        match stmt {
            Statement::Assign(lhs, rhs) => { f(lhs); f(rhs); }
            Statement::Expression(e) => f(e),
            Statement::Term(Some(e)) => f(e),
            Statement::EndProgram(Some(e)) => f(e),
            Statement::Guarded(_, body) => walk_statements(body, f),
            Statement::Block(body) => walk_statements(body, f),
            _ => {}
        }
    }
}
