// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H5 inventory port: proof-directed garbage scheduling over HIR.
//! Mirror of `analysis::global_lifetime` (AST, authoritative until the
//! consumer flip). Reuses `GlobalLifetime` (returns by value; `PartialEq`
//! added for the debug assert). `field_initializers` is HIR-typed
//! (`HashMap<String, HirExpr>`); the consumer builds it from
//! `HirProgram::globals` (state decls default to an Int 0 image, matching
//! the AST builder's `Decimal(0)`).

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::global_lifetime::GlobalLifetime;
use std::collections::{HashMap, HashSet};

pub fn analyze(
    program: &HirProgram,
    field_initializers: &HashMap<String, HirExpr>,
    node_order: &[String],
    foldable: &HashSet<String>,
) -> GlobalLifetime {
    let state_fields: HashSet<String> = field_initializers.keys().cloned().collect();
    let mut manually_freed: HashSet<String> = HashSet::new();
    let mut kept: HashSet<String> = HashSet::new();
    let mut txn_touches: HashMap<String, HashSet<String>> = HashMap::new();
    // Mirror the AST: raw `TopLevel::Transaction` items only — bare,
    // non-exported, non-sync-wrapped; defns are not scanned here.
    for c in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Transaction)
            && c.owner.is_none()
            && !c.exported
            && c.sync_domains.is_empty()
    }) {
        for stmt in &c.body.statements {
            if let Some(target) = manual_free_target(stmt, &state_fields) {
                manually_freed.insert(target);
            }
            if let HirStmtKind::KeepHint(name) = &stmt.kind {
                kept.insert(name.clone());
            }
        }
        let mut touches = HashSet::new();
        for stmt in &c.body.statements {
            collect_statement_identifiers(stmt, &state_fields, &mut touches);
        }
        if !touches.is_empty() {
            txn_touches.insert(c.name.clone(), touches);
        }
    }
    let heap_backed: Vec<String> = field_initializers
        .iter()
        .filter(|(_, init)| contains_heap_alloc(init))
        .filter(|(name, _)| !manually_freed.contains(name.as_str()))
        .map(|(name, _)| name.clone())
        .collect();
    if heap_backed.is_empty() {
        return GlobalLifetime {
            free_after: HashMap::new(),
            redundant_keeps: kept.into_iter().collect(),
            lifetime_fallbacks: Vec::new(),
        };
    }
    let mut free_after: HashMap<String, Vec<String>> = HashMap::new();
    let mut fallbacks: Vec<(String, &'static str)> = Vec::new();
    for field in &heap_backed {
        let consumers: Vec<&String> = node_order
            .iter()
            .filter(|n| txn_touches.get(*n).map_or(false, |t| t.contains(field)))
            .collect();
        let Some(last) = consumers.last() else {
            fallbacks.push((field.clone(), "no transaction reads it"));
            continue;
        };
        if !foldable.contains(last.as_str()) {
            fallbacks.push((
                field.clone(),
                "its last consumer has no bounded-loop shape to free after",
            ));
            continue;
        }
        free_after.entry((*last).clone()).or_default().push(field.clone());
    }
    for v in free_after.values_mut() {
        v.sort();
    }
    let scheduled: HashSet<String> = free_after.values().flatten().cloned().collect();
    let redundant_keeps: Vec<String> = kept
        .into_iter()
        .filter(|k| !scheduled.contains(k))
        .collect();
    GlobalLifetime {
        free_after,
        redundant_keeps,
        lifetime_fallbacks: fallbacks,
    }
}

fn manual_free_target(stmt: &HirStmt, state_fields: &HashSet<String>) -> Option<String> {
    fn walk_expr(e: &HirExpr, state_fields: &HashSet<String>) -> Option<String> {
        match &e.kind {
            HirExprKind::Call { callee, args, .. } => {
                if callee == "Free#" {
                    if let Some(arg) = args.first() {
                        let mut inner = arg;
                        while let HirExprKind::Cast { inner: i, .. } = &inner.kind {
                            inner = i;
                        }
                        if let HirExprKind::Identifier(n) = &inner.kind {
                            if state_fields.contains(n) {
                                return Some(n.clone());
                            }
                        }
                    }
                    return None;
                }
                args.iter().find_map(|a| walk_expr(a, state_fields))
            }
            HirExprKind::Cast { inner, .. } | HirExprKind::AddrOf(inner) => {
                walk_expr(inner, state_fields)
            }
            _ => None,
        }
    }
    match &stmt.kind {
        HirStmtKind::Expression(e) => walk_expr(e, state_fields),
        HirStmtKind::Guarded(_, body) => {
            body.iter().find_map(|s| manual_free_target(s, state_fields))
        }
        HirStmtKind::Block(stmts) => {
            stmts.iter().find_map(|s| manual_free_target(s, state_fields))
        }
        HirStmtKind::FreeHint(name) | HirStmtKind::KeepHint(name) => {
            state_fields.contains(name).then(|| name.clone())
        }
        _ => None,
    }
}

/// Mirror of `transition_graph::collect_state_identifiers` over HIR.
fn collect_state_identifiers(
    expr: &HirExpr,
    state_fields: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    match &expr.kind {
        HirExprKind::Identifier(name) => {
            if state_fields.contains(name) {
                out.insert(name.clone());
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_state_identifiers(left, state_fields, out);
            collect_state_identifiers(right, state_fields, out);
        }
        HirExprKind::Unary { inner, .. } => collect_state_identifiers(inner, state_fields, out),
        HirExprKind::Index(obj, idx) => {
            collect_state_identifiers(obj, state_fields, out);
            collect_state_identifiers(idx, state_fields, out);
        }
        HirExprKind::Field { receiver, .. } => collect_state_identifiers(receiver, state_fields, out),
        HirExprKind::AddrOf(inner)
        | HirExprKind::Deref(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Cast { inner, .. } => collect_state_identifiers(inner, state_fields, out),
        HirExprKind::Call { args, .. } => {
            for arg in args {
                collect_state_identifiers(arg, state_fields, out);
            }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            collect_state_identifiers(receiver, state_fields, out);
            for arg in args {
                collect_state_identifiers(arg, state_fields, out);
            }
        }
        HirExprKind::Reflect { receiver, .. } => collect_state_identifiers(receiver, state_fields, out),
        HirExprKind::Block(stmts) => {
            scan_for_state_identifiers(stmts, state_fields, out);
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            for e in items {
                collect_state_identifiers(e, state_fields, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_state_identifiers(scrutinee, state_fields, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_state_identifiers(guard, state_fields, out);
                }
                collect_state_identifiers(&arm.body, state_fields, out);
            }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            collect_state_identifiers(condition, state_fields, out);
            collect_state_identifiers(then_branch, state_fields, out);
            if let Some(else_) = else_branch {
                collect_state_identifiers(else_, state_fields, out);
            }
        }
        HirExprKind::Lambda { body, .. } => collect_state_identifiers(body, state_fields, out),
        HirExprKind::Range { start, end, .. } => {
            collect_state_identifiers(start, state_fields, out);
            collect_state_identifiers(end, state_fields, out);
        }
        HirExprKind::Slice { array, start, end, stride } => {
            collect_state_identifiers(array, state_fields, out);
            if let Some(s) = start {
                collect_state_identifiers(s, state_fields, out);
            }
            if let Some(e) = end {
                collect_state_identifiers(e, state_fields, out);
            }
            if let Some(s) = stride {
                collect_state_identifiers(s, state_fields, out);
            }
        }
        HirExprKind::Spawn { args, .. } => {
            for e in args {
                collect_state_identifiers(e, state_fields, out);
            }
        }
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, e) in fields.iter().chain(specs.iter()) {
                collect_state_identifiers(e, state_fields, out);
            }
        }
        HirExprKind::IsType { inner, .. } | HirExprKind::Await(inner) => {
            collect_state_identifiers(inner, state_fields, out)
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            if let Some(r) = receiver {
                collect_state_identifiers(r, state_fields, out);
            }
            for e in args {
                collect_state_identifiers(e, state_fields, out);
            }
        }
        _ => {}
    }
}

/// Mirror of `transition_graph::scan_for_state_identifiers` over HIR
/// (statement-list variant used by the Block-expression arm).
fn scan_for_state_identifiers(
    stmts: &[HirStmt],
    state_fields: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    for s in stmts {
        collect_statement_identifiers(s, state_fields, out);
    }
}

/// Mirror of `transition_graph::collect_statement_identifiers` over HIR.
pub(crate) fn collect_statement_identifiers(
    stmt: &HirStmt,
    state_fields: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    match &stmt.kind {
        HirStmtKind::Assign(lhs, rhs) => {
            collect_state_identifiers(lhs, state_fields, out);
            collect_state_identifiers(rhs, state_fields, out);
        }
        HirStmtKind::Let { expr, .. } => {
            if let Some(e) = expr {
                collect_state_identifiers(e, state_fields, out);
            }
        }
        HirStmtKind::Expression(e) => collect_state_identifiers(e, state_fields, out),
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                collect_state_identifiers(t, state_fields, out);
            }
            collect_state_identifiers(value, state_fields, out);
        }
        HirStmtKind::Guarded(cond, body) => {
            collect_state_identifiers(cond, state_fields, out);
            for s in body {
                collect_statement_identifiers(s, state_fields, out);
            }
        }
        HirStmtKind::Term(val) | HirStmtKind::EndProgram(val) => {
            if let Some(v) = val {
                collect_state_identifiers(v, state_fields, out);
            }
        }
        HirStmtKind::Foreach { list, body, .. } => {
            collect_state_identifiers(list, state_fields, out);
            for s in body {
                collect_statement_identifiers(s, state_fields, out);
            }
        }
        HirStmtKind::Block(stmts) => {
            for s in stmts {
                collect_statement_identifiers(s, state_fields, out);
            }
        }
        _ => {}
    }
}

/// Mirror of `analysis::global_lifetime::contains_heap_alloc` over HIR.
pub fn contains_heap_alloc(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Call { callee, args, .. } => {
            if callee == "Malloc#" || callee == "Alloc#" {
                return true;
            }
            args.iter().any(contains_heap_alloc)
        }
        HirExprKind::Cast { inner, .. } => contains_heap_alloc(inner),
        HirExprKind::Binary { left, right, .. } => {
            contains_heap_alloc(left) || contains_heap_alloc(right)
        }
        HirExprKind::Index(obj, idx) => contains_heap_alloc(obj) || contains_heap_alloc(idx),
        HirExprKind::Field { receiver, .. } => contains_heap_alloc(receiver),
        HirExprKind::AddrOf(inner) | HirExprKind::Deref(inner) => contains_heap_alloc(inner),
        _ => false,
    }
}
