// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.5 — task-body segmentation + spawn-target/ABI analysis over HIR.
//! Mirror of `analysis::task_segments` (AST, authoritative until parity).

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::ast::Type;
use std::collections::HashSet;

pub fn split_task_body(body: &[HirStmt], param_names: &[String]) -> Vec<Vec<HirStmt>> {
    let mut segments: Vec<Vec<HirStmt>> = vec![Vec::new()];
    for stmt in body {
        let is_port_read = mentions_param_field(stmt, param_names);
        let current_empty = segments.last().map(|s| s.is_empty()).unwrap_or(false);
        if matches!(stmt.kind, HirStmtKind::Yield) || (is_port_read && !current_empty) {
            segments.push(Vec::new());
        }
        if !matches!(stmt.kind, HirStmtKind::Yield) {
            segments.last_mut().unwrap().push(stmt.clone());
        }
    }
    segments
}

fn collect_stmt_spawns(stmts: &[super::stmt::HirStmt], out: &mut HashSet<String>) {
    for s in stmts {
        stmt_walk(s, out);
    }
}

pub fn collect_spawn_targets(program: &HirProgram) -> HashSet<String> {
    let mut out = HashSet::new();
    for c in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Defn | HirCallableKind::Transaction) && c.owner.is_none()
    }) {
        collect_stmt_spawns(&c.body.statements, &mut out);
    }
    for g in &program.globals {
        if let Some(e) = &g.expr {
            expr_walk(e, &mut out);
        }
    }
    for (_, e) in &program.const_exprs {
        expr_walk(e, &mut out);
    }
    for init in &program.inits {
        if let Some(e) = &init.value {
            expr_walk(e, &mut out);
        }
        collect_stmt_spawns(&init.body, &mut out);
    }
    out
}

pub fn collect_task_abi_errors(
    program: &HirProgram,
    universe: &crate::type_universe::TypeUniverse,
) -> Vec<String> {
    let graph = crate::casting::graph::CastingGraph::new();
    let targets = collect_spawn_targets(program);
    let mut errors = Vec::new();
    for c in program.callables.iter().filter(|c| is_top_defn(c)) {
        check_task_signature(c, &targets, &graph, universe, &mut errors);
    }
    errors.sort();
    errors.dedup();
    errors
}

fn is_top_defn(c: &HirCallable) -> bool {
    matches!(c.kind, HirCallableKind::Defn) && c.owner.is_none() && !c.trait_default && !c.exported
}

const TASK_ABI_HINT: &str =
    "compiled tasks carry arguments/results as i64 slots; \
     use Int/Bool/Char/Ptr (v1 scope)";

fn task_type_is_i64_abi(
    graph: &crate::casting::graph::CastingGraph,
    universe: &crate::type_universe::TypeUniverse,
    ty: &Type,
) -> bool {
    match ty {
        Type::Ptr(_) | Type::Bits(_) | Type::Void => true,
        Type::Applied(base, _) if base == "Event" => true,
        _ => {
            let (cat, _) = graph.type_to_protocol(universe, ty);
            matches!(cat.as_str(), "Int" | "Bit" | "Char" | "UInt")
        }
    }
}

fn result_types_of(c: &HirCallable) -> Vec<Type> {
    if !c.output_list.is_empty() {
        return c.output_list.clone();
    }
    c.outputs
        .as_ref()
        .map(|o| o.all_types())
        .unwrap_or_default()
}

fn check_task_signature(
    c: &HirCallable,
    targets: &HashSet<String>,
    graph: &crate::casting::graph::CastingGraph,
    universe: &crate::type_universe::TypeUniverse,
    errors: &mut Vec<String>,
) {
    if !targets.contains(&c.name) {
        return;
    }
    for p in &c.params {
        if !task_type_is_i64_abi(graph, universe, &p.ty) {
            errors.push(format!(
                "task '{}': parameter '{}' has type {} — {TASK_ABI_HINT}",
                c.name, p.name, p.ty
            ));
        }
    }
    for rty in result_types_of(c) {
        if !task_type_is_i64_abi(graph, universe, &rty) {
            errors.push(format!(
                "task '{}': result type {} is not i64-ABI — await returns it \
                 through an i64 slot; {TASK_ABI_HINT}",
                c.name, rty
            ));
        }
    }
}

fn stmt_walk(s: &HirStmt, out: &mut HashSet<String>) {
    for e in stmt_exprs(s) {
        expr_walk(e, out);
    }
    for inner in stmt_bodies(s).into_iter().flatten() {
        stmt_walk(inner, out);
    }
}

fn expr_walk(e: &HirExpr, out: &mut HashSet<String>) {
    if let HirExprKind::Spawn { type_name, .. } = &e.kind {
        out.insert(type_name.clone());
    }
    for child in expr_children(e) {
        expr_walk(child, out);
    }
}

fn stmt_exprs(s: &HirStmt) -> Vec<&HirExpr> {
    match &s.kind {
        HirStmtKind::Let { expr: Some(e), .. } => vec![e],
        HirStmtKind::Assign(lhs, rhs) => vec![lhs, rhs],
        HirStmtKind::Expression(e)
        | HirStmtKind::Term(Some(e))
        | HirStmtKind::Tail(e)
        | HirStmtKind::Rollback(Some(e))
        | HirStmtKind::EndProgram(Some(e))
        | HirStmtKind::Check(e)
        | HirStmtKind::Gate(e) => vec![e],
        HirStmtKind::Guarded(cond, _) => vec![cond],
        HirStmtKind::Foreach { list, .. } => vec![list],
        HirStmtKind::ArrowAssign { target, value, .. } => target
            .iter()
            .chain(std::iter::once(value))
            .collect(),
        HirStmtKind::Match { expr, .. } => vec![expr],
        _ => Vec::new(),
    }
}

fn stmt_bodies(s: &HirStmt) -> Vec<&[HirStmt]> {
    match &s.kind {
        HirStmtKind::Guarded(_, b)
        | HirStmtKind::Block(b)
        | HirStmtKind::SyncBlock(b)
        | HirStmtKind::Mutex(b)
        | HirStmtKind::Defer(b) => vec![b.as_slice()],
        HirStmtKind::Foreach { body, .. } => vec![body.as_slice()],
        HirStmtKind::Match { arms, .. } => arms.iter().map(|a| a.body.as_slice()).collect(),
        _ => Vec::new(),
    }
}

fn expr_children(e: &HirExpr) -> Vec<&HirExpr> {
    match &e.kind {
        HirExprKind::Field { receiver, .. }
        | HirExprKind::Await(receiver)
        | HirExprKind::Cast { inner: receiver, .. }
        | HirExprKind::IsType { inner: receiver, .. }
        | HirExprKind::Consume(receiver)
        | HirExprKind::Deref(receiver)
        | HirExprKind::AddrOf(receiver) => vec![receiver],
        HirExprKind::Index(l, r) => vec![l, r],
        HirExprKind::Range { start, end, .. } => vec![start, end],
        HirExprKind::Binary { left, right, .. } => vec![left, right],
        HirExprKind::Call { args, .. } | HirExprKind::MethodCall { args, .. } => {
            args.iter().collect()
        }
        _ => Vec::new(),
    }
}

pub fn mentions_param_field(stmt: &HirStmt, params: &[String]) -> bool {
    if params.is_empty() {
        return false;
    }
    fn root_is_param(e: &HirExpr, params: &[String]) -> bool {
        let mut root = e;
        loop {
            match &root.kind {
                HirExprKind::Identifier(name) => return params.iter().any(|p| p == name),
                HirExprKind::Field { receiver, .. } | HirExprKind::Index(receiver, _) => {
                    root = receiver;
                }
                _ => return false,
            }
        }
    }
    fn expr_walk(e: &HirExpr, params: &[String]) -> bool {
        match &e.kind {
            HirExprKind::Field { receiver, .. } => {
                root_is_param(receiver, params) || expr_walk(receiver, params)
            }
            _ => false,
        }
    }
    fn stmt_exprs(s: &HirStmt, params: &[String]) -> bool {
        match &s.kind {
            HirStmtKind::Let { expr: Some(e), .. }
            | HirStmtKind::Assign(_, e)
            | HirStmtKind::Expression(e)
            | HirStmtKind::Term(Some(e))
            | HirStmtKind::Tail(e)
            | HirStmtKind::Rollback(Some(e))
            | HirStmtKind::EndProgram(Some(e))
            | HirStmtKind::Check(e)
            | HirStmtKind::Gate(e) => expr_walk(e, params),
            HirStmtKind::Guarded(cond, body) => {
                expr_walk(cond, params) || body.iter().any(|s| stmt_exprs(s, params))
            }
            HirStmtKind::Block(body)
            | HirStmtKind::SyncBlock(body)
            | HirStmtKind::Mutex(body)
            | HirStmtKind::Defer(body) => body.iter().any(|s| stmt_exprs(s, params)),
            HirStmtKind::Foreach { list, body, .. } => {
                expr_walk(list, params) || body.iter().any(|s| stmt_exprs(s, params))
            }
            HirStmtKind::ArrowAssign { target: Some(t), value, .. } => {
                expr_walk(t, params) || expr_walk(value, params)
            }
            HirStmtKind::ArrowAssign { value, .. } => expr_walk(value, params),
            HirStmtKind::Match { expr, arms } => {
                expr_walk(expr, params)
                    || arms
                        .iter()
                        .any(|a| a.body.iter().any(|s| stmt_exprs(s, params)))
            }
            _ => false,
        }
    }
    stmt_exprs(stmt, params)
}
