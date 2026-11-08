// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.3a — top-level `let` storage classification (state vs plain) over HIR.
//!
//! Mirror of `analysis::storage_class::classify_plain_lets` (AST, still
//! authoritative until corpus parity). Same rules: a bare let is plain iff
//! the proof surface does not reference it, no reactive txn body writes it,
//! its type is scalar, and its initializer is scalar; `plain let` always,
//! `state let` never.

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::ast::Type;
use std::collections::HashSet;

/// Top-level lets classified PLAIN (declared or inferred).
pub fn classify_plain_lets(
    program: &HirProgram,
    exit_condition: Option<&HirExpr>,
    extra: &HashSet<String>,
) -> HashSet<String> {
    let surface = proof_surface(program, exit_condition, extra);
    let written = body_written_names(program);
    let mut plain = HashSet::new();
    for g in &program.globals {
        // `state name: T;` is never a plain let (AST iterates top-level lets).
        if g.is_state_decl {
            continue;
        }
        if let_lowers_plain(
            g.declared_storage.as_deref(),
            surface.contains(&g.name),
            written.contains(&g.name),
            &g.ty,
            g.expr.as_ref(),
        ) {
            plain.insert(g.name.clone());
        }
    }
    plain
}

/// 2026-11-05 (H4 intercept coverage): the AST proof surface iterates raw
/// `TopLevel::Transaction` items — BARE top-level transactions only.
/// Sync-wrapped and export-wrapped copies are not raw Transaction items, so
/// their bodies are outside the surface (mixed_effect_shape's all-sync nodes
/// depend on this).
fn is_txn(c: &super::program::HirCallable) -> bool {
    matches!(c.kind, HirCallableKind::Transaction)
        && c.owner.is_none()
        && !c.exported
        && c.sync_domains.is_empty()
}

fn proof_surface(
    program: &HirProgram,
    exit_condition: Option<&HirExpr>,
    extra: &HashSet<String>,
) -> HashSet<String> {
    let mut surface: HashSet<String> = extra.clone();
    if let Some(e) = exit_condition {
        collect_raw_idents(e, &mut surface);
    }
    for c in program.callables.iter().filter(|c| is_txn(c)) {
        if let Some(ct) = &c.contract {
            collect_raw_idents(&ct.pre, &mut surface);
            collect_raw_idents(&ct.post, &mut surface);
            if let Some(wd) = &ct.watchdog {
                collect_raw_idents(&wd.condition, &mut surface);
                if let Some(fb) = &wd.fallback {
                    collect_raw_idents(fb, &mut surface);
                }
                if let Some(arg) = wd.on_fire.as_ref().and_then(|f| f.arg.as_deref()) {
                    surface.insert(arg.to_string());
                }
            }
        }
        collect_stmts_surface(&c.body.statements, &mut surface);
    }
    surface
}

fn body_written_names(program: &HirProgram) -> HashSet<String> {
    let mut written = HashSet::new();
    for c in program.callables.iter().filter(|c| is_txn(c)) {
        walk_written(&c.body.statements, &mut written);
    }
    written
}

fn walk_written(stmts: &[HirStmt], out: &mut HashSet<String>) {
    for stmt in stmts {
        written_of_stmt(stmt, out);
    }
}

fn written_of_stmt(stmt: &HirStmt, out: &mut HashSet<String>) {
    match &stmt.kind {
        HirStmtKind::Assign(lhs, _) => insert_ident_name(lhs, out),
        HirStmtKind::ArrowAssign { target: Some(t), .. } => insert_ident_name(t, out),
        HirStmtKind::Guarded(_, body)
        | HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Mutex(body)
        | HirStmtKind::Foreach { body, .. } => walk_written(body, out),
        HirStmtKind::Match { arms, .. } => {
            for arm in arms {
                walk_written(&arm.body, out);
            }
        }
        _ => {}
    }
}

fn insert_ident_name(expr: &HirExpr, out: &mut HashSet<String>) {
    if let HirExprKind::Identifier(name) = &expr.kind {
        out.insert(name.clone());
    }
}

fn scalar_type(ty: &Option<Type>) -> bool {
    match ty {
        None => true,
        Some(Type::Custom(n)) => matches!(n.as_str(), "Int" | "Float" | "Float64" | "Bool"),
        _ => false,
    }
}

fn scalar_init(expr: Option<&HirExpr>) -> bool {
    match expr {
        None => true,
        Some(e) => !matches!(
            &e.kind,
            HirExprKind::List(_)
                | HirExprKind::Tuple(_)
                | HirExprKind::StructLiteral { .. }
                | HirExprKind::Call { .. }
                | HirExprKind::MethodCall { .. }
        ),
    }
}

fn let_lowers_plain(
    declared: Option<&str>,
    surface_has: bool,
    body_writes: bool,
    ty: &Option<Type>,
    expr: Option<&HirExpr>,
) -> bool {
    match declared {
        Some("plain") => true,
        Some(_) => false,
        None => !surface_has && !body_writes && scalar_type(ty) && scalar_init(expr),
    }
}

/// Collect every identifier an expression mentions (mirrors the AST walker's
/// variant coverage exactly — a missed variant would wrongly demote a let).
pub(crate) fn collect_raw_idents(expr: &HirExpr, out: &mut HashSet<String>) {
    match &expr.kind {
        HirExprKind::Identifier(name) => {
            out.insert(name.clone());
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_raw_idents(left, out);
            collect_raw_idents(right, out);
        }
        HirExprKind::Unary { inner, .. } => collect_raw_idents(inner, out),
        HirExprKind::Index(obj, idx) => {
            collect_raw_idents(obj, out);
            collect_raw_idents(idx, out);
        }
        HirExprKind::Field { receiver, .. } => collect_raw_idents(receiver, out),
        HirExprKind::AddrOf(inner)
        | HirExprKind::Deref(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Cast { inner, .. } => collect_raw_idents(inner, out),
        HirExprKind::Call { args, .. } => {
            for arg in args {
                collect_raw_idents(arg, out);
            }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            collect_raw_idents(receiver, out);
            for arg in args {
                collect_raw_idents(arg, out);
            }
        }
        HirExprKind::Reflect { receiver, .. } => collect_raw_idents(receiver, out),
        HirExprKind::Block(stmts) => collect_stmts_surface(stmts, out),
        HirExprKind::Tuple(exprs) | HirExprKind::List(exprs) => {
            for e in exprs {
                collect_raw_idents(e, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_raw_idents(scrutinee, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_raw_idents(guard, out);
                }
                collect_raw_idents(&arm.body, out);
            }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            collect_raw_idents(condition, out);
            collect_raw_idents(then_branch, out);
            if let Some(e) = else_branch {
                collect_raw_idents(e, out);
            }
        }
        HirExprKind::Lambda { body, .. } => collect_raw_idents(body, out),
        HirExprKind::Range { start, end, .. } => {
            collect_raw_idents(start, out);
            collect_raw_idents(end, out);
        }
        _ => {}
    }
}

fn collect_stmts_surface(stmts: &[HirStmt], out: &mut HashSet<String>) {
    for stmt in stmts {
        collect_stmt_surface(stmt, out);
    }
}

fn collect_stmt_surface(stmt: &HirStmt, out: &mut HashSet<String>) {
    match &stmt.kind {
        HirStmtKind::EndProgram(Some(e)) | HirStmtKind::Term(Some(e)) => {
            collect_raw_idents(e, out)
        }
        HirStmtKind::Guarded(_, body)
        | HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Mutex(body)
        | HirStmtKind::Foreach { body, .. } => collect_stmts_surface(body, out),
        HirStmtKind::Match { arms, .. } => {
            for arm in arms {
                collect_stmts_surface(&arm.body, out);
            }
        }
        _ => {}
    }
}
