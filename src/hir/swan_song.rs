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
    let mut stmts: Vec<&HirStmt> = body
        .iter()
        .filter(|s| {
            !matches!(
                s.kind,
                HirStmtKind::EndProgram(..) | HirStmtKind::Term(None)
            )
        })
        .collect();

    let mut let_to_field: HashMap<String, String> = HashMap::new();
    for s in body {
        if let HirStmtKind::Assign(lhs, HirExpr {
            kind: HirExprKind::Identifier(let_name),
            ..
        }) = &s.kind
        {
            if let HirExprKind::Identifier(field_name) = &lhs.kind {
                if state_fields.contains(field_name) {
                    let_to_field.insert(let_name.clone(), field_name.clone());
                }
            }
        }
    }

    let mut hoist: Vec<Vec<HirStmt>> = Vec::new();
    while let Some(last_idx) = stmts.len().checked_sub(1) {
        let HirStmtKind::Guarded(_, statements) = &stmts[last_idx].kind else {
            break;
        };
        if !statements
            .iter()
            .any(|s| matches!(s.kind, HirStmtKind::EndProgram(..)))
        {
            break;
        }
        let mut body_stmts: Vec<HirStmt> = statements
            .iter()
            .filter(|s| !matches!(s.kind, HirStmtKind::EndProgram(..)))
            .cloned()
            .collect();
        for s in &mut body_stmts {
            remap_stmt_identifiers(s, &let_to_field);
        }
        let swan_song_stmt = statements.iter().find_map(|s| {
            if let HirStmtKind::EndProgram(Some(ss)) = &s.kind {
                Some(ss.clone())
            } else {
                None
            }
        });
        let swan_song_stmt = swan_song_stmt.map(|mut ss| {
            remap_expr_into(&mut ss, &let_to_field);
            ss
        });
        if !body_stmts.is_empty() || swan_song_stmt.is_some() {
            let mut full_body = body_stmts;
            if let Some(sw) = swan_song_stmt {
                full_body.push(HirStmt {
                    id: stmts[last_idx].id,
                    span: stmts[last_idx].span,
                    kind: HirStmtKind::Expression(sw),
                });
            }
            hoist.push(full_body);
            stmts.pop();
        }
        break;
    }

    (stmts.into_iter().cloned().collect(), hoist)
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
        _ => {}
    }
}
