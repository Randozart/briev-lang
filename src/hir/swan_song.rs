// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b — swan-song hoisting over HIR. Mirror of `analysis::swan_song`.

use super::expr::{HirExpr, HirExprKind};
use super::stmt::{HirStmt, HirStmtKind};
use std::collections::{HashMap, HashSet};

pub fn has_swan_song(body: &[HirStmt]) -> bool {
    let stmts: Vec<&HirStmt> = body
        .iter()
        .filter(|s| {
            !matches!(
                s.kind,
                HirStmtKind::Term(..) | HirStmtKind::EndProgram(..) | HirStmtKind::Tail(_)
            )
        })
        .collect();
    match stmts.last() {
        Some(HirStmt {
            kind: HirStmtKind::Guarded(_, statements),
            ..
        }) => statements
            .iter()
            .any(|s| matches!(s.kind, HirStmtKind::EndProgram(..))),
        _ => false,
    }
}

pub fn hoist_swan_song(
    body: &[HirStmt],
    state_fields: &HashSet<String>,
) -> (Vec<HirStmt>, Vec<Vec<HirStmt>>) {
    let mut stmts = without_terminators(body);
    let let_to_field = let_to_field_map(body, state_fields);
    let mut hoist: Vec<Vec<HirStmt>> = Vec::new();
    if let Some(last) = stmts.last() {
        if is_swan_song_guard(last) {
            hoist.push(hoisted_guard_body(last, &let_to_field));
            stmts.pop();
        }
    }
    (stmts.into_iter().cloned().collect(), hoist)
}

/// Body statements minus trailing terminators (`endprogram`/bare `term`).
fn without_terminators(body: &[HirStmt]) -> Vec<&HirStmt> {
    body.iter()
        .filter(|s| !matches!(s.kind, HirStmtKind::EndProgram(..) | HirStmtKind::Term(None)))
        .collect()
}

/// `let x = state_field;` bindings at body scope, mapped to the field name.
fn let_to_field_map(
    body: &[HirStmt],
    state_fields: &HashSet<String>,
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for s in body {
        let HirStmtKind::Assign(lhs, HirExpr {
            kind: HirExprKind::Identifier(let_name),
            ..
        }) = &s.kind
        else {
            continue;
        };
        if let HirExprKind::Identifier(field_name) = &lhs.kind {
            if state_fields.contains(field_name) {
                map.insert(let_name.clone(), field_name.clone());
            }
        }
    }
    map
}

fn is_swan_song_guard(stmt: &HirStmt) -> bool {
    let HirStmtKind::Guarded(_, statements) = &stmt.kind else {
        return false;
    };
    statements
        .iter()
        .any(|s| matches!(s.kind, HirStmtKind::EndProgram(..)))
}

/// The hoisted block for a terminating guard: its body statements with field
/// remapping, followed by the `endprogram` value as an expression.
fn hoisted_guard_body(stmt: &HirStmt, let_to_field: &HashMap<String, String>) -> Vec<HirStmt> {
    let HirStmtKind::Guarded(_, statements) = &stmt.kind else {
        return Vec::new();
    };
    let mut body_stmts: Vec<HirStmt> = statements
        .iter()
        .filter(|s| !matches!(s.kind, HirStmtKind::EndProgram(..)))
        .cloned()
        .collect();
    for s in &mut body_stmts {
        remap_stmt_identifiers(s, let_to_field);
    }
    let swan = statements.iter().find_map(|s| match &s.kind {
        HirStmtKind::EndProgram(Some(ss)) => Some(ss.clone()),
        _ => None,
    });
    let swan = swan.map(|mut ss| {
        remap_expr_into(&mut ss, let_to_field);
        ss
    });
    let mut full_body = body_stmts;
    if let Some(sw) = swan {
        full_body.push(HirStmt {
            id: stmt.id,
            span: stmt.span,
            kind: HirStmtKind::Expression(sw),
        });
    }
    full_body
}

pub(crate) fn remap_stmt_identifiers(s: &mut HirStmt, map: &HashMap<String, String>) {
    match &mut s.kind {
        HirStmtKind::Assign(_, expr) => remap_expr_into(expr, map),
        HirStmtKind::Expression(e) => remap_expr_into(e, map),
        HirStmtKind::EndProgram(Some(ss)) => remap_expr_into(ss, map),
        HirStmtKind::Guarded(condition, statements) => {
            remap_expr_into(condition, map);
            for stmt in statements.iter_mut() {
                remap_stmt_identifiers(stmt, map);
            }
        }
        HirStmtKind::Let { expr: Some(e), .. } => remap_expr_into(e, map),
        HirStmtKind::Let { expr: None, .. } => {}
        _ => {}
    }
}

fn remap_expr_into(e: &mut HirExpr, map: &HashMap<String, String>) {
    match &mut e.kind {
        HirExprKind::Identifier(name) => {
            if let Some(field) = map.get(name) {
                *name = field.clone();
            }
        }
        HirExprKind::Call { args, .. } => {
            for arg in args.iter_mut() {
                remap_expr_into(arg, map);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            remap_expr_into(left, map);
            remap_expr_into(right, map);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. } => {
            remap_expr_into(inner, map);
        }
        HirExprKind::Field { receiver, .. } | HirExprKind::Index(receiver, _) => {
            remap_expr_into(receiver, map);
        }
        HirExprKind::Block(stmts) => {
            for s in stmts.iter_mut() {
                remap_stmt_identifiers(s, map);
            }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            remap_expr_into(condition, map);
            remap_expr_into(then_branch, map);
            if let Some(eb) = else_branch {
                remap_expr_into(eb, map);
            }
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            for item in items.iter_mut() {
                remap_expr_into(item, map);
            }
        }
        HirExprKind::PluginIntercept { args, .. } => {
            for arg in args.iter_mut() {
                remap_expr_into(arg, map);
            }
        }
        _ => {}
    }
}
