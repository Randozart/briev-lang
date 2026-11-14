// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4a — direct self-tail-recursion analysis over typed HIR.
//!
//! Mirror of `analysis::tail_recursion` (AST, still authoritative until
//! parity). Reuses the AST plan types (`TailRecursionClass`,
//! `TailRecursionPlan`, `TailLoopPlan`) so consumers cannot drift.

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use super::expr_walk::walk_stmts;
use crate::analysis::tail_recursion::{TailLoopPlan, TailRecursionClass, TailRecursionPlan};
use std::collections::{BTreeMap, BTreeSet};

/// Top-level, non-exported defns — the AST plan's exact index
/// (`TopLevel::Definition`, which excludes `Export` wrappers and members).
fn is_top_defn(c: &HirCallable) -> bool {
    matches!(c.kind, HirCallableKind::Defn) && c.owner.is_none() && !c.trait_default && !c.exported
}

pub fn build_tail_recursion_plan(program: &HirProgram) -> TailRecursionPlan {
    let mut defns = BTreeMap::new();
    for c in program.callables.iter().filter(|c| is_top_defn(c)) {
        let total = count_self_calls_stmts(&c.body.statements, &c.name);
        let class = if total == 0 {
            TailRecursionClass::NotRecursive
        } else if tail_calls_in_body(&c.body.statements, &c.name) == total {
            TailRecursionClass::TailRecursive
        } else {
            TailRecursionClass::NonTailRecursive
        };
        defns.insert(c.name.clone(), class);
    }
    TailRecursionPlan { defns }
}

pub fn build_tail_loop_plan(program: &HirProgram) -> TailLoopPlan {
    let tail = build_tail_recursion_plan(program);
    let mut defns = BTreeSet::new();
    for c in program.callables.iter().filter(|c| is_top_defn(c)) {
        if tail.class(&c.name) != TailRecursionClass::TailRecursive {
            continue;
        }
        if !loop_lowerable_signature(c) || has_cleanup_or_boundary(&c.body.statements) {
            continue;
        }
        let calls = tail_self_calls_in_body(&c.body.statements, &c.name);
        if !tail_calls_match_arity(&calls, c.params.len()) {
            continue;
        }
        defns.insert(c.name.clone());
    }
    TailLoopPlan { defns }
}

fn loop_lowerable_signature(c: &HirCallable) -> bool {
    c.type_params.is_empty()
        && !c.variadic
        && c.params.iter().all(|p| loop_storable_type(&p.ty))
}

fn tail_calls_match_arity(calls: &[Vec<HirExpr>], arity: usize) -> bool {
    !calls.is_empty() && calls.iter().all(|args| args.len() == arity)
}

fn loop_storable_type(ty: &crate::ast::Type) -> bool {
    use crate::ast::Type;
    !matches!(
        ty,
        Type::Vector(_, _)
            | Type::Dyn(_)
            | Type::Task(_)
            | Type::TypeVar(_)
            | Type::Width(_)
            | Type::Number(_)
            | Type::LayoutPtr(_)
    ) && !matches!(ty, Type::Custom(name) if name == "Slice")
}

fn has_cleanup_or_boundary(stmts: &[HirStmt]) -> bool {
    let mut bad = stmts.iter().any(|stmt| match &stmt.kind {
        HirStmtKind::Defer(_) | HirStmtKind::Rollback(_) | HirStmtKind::EndProgram(_) => true,
        HirStmtKind::Guarded(_, body)
        | HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Mutex(body)
        | HirStmtKind::Foreach { body, .. } => has_cleanup_or_boundary(body),
        _ => false,
    });
    if bad {
        return true;
    }
    walk_stmts(stmts, &mut |e| {
        if let HirExprKind::Block(arm_stmts) = &e.kind {
            bad |= has_cleanup_or_boundary(arm_stmts);
        }
    });
    bad
}

fn tail_self_calls_in_body(body: &[HirStmt], name: &str) -> Vec<Vec<HirExpr>> {
    let mut out = Vec::new();
    collect_tail_self_calls(body, name, &mut out);
    out
}

fn collect_tail_self_calls(body: &[HirStmt], name: &str, out: &mut Vec<Vec<HirExpr>>) {
    for stmt in body {
        match &stmt.kind {
            HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e) => {
                collect_tail_self_calls_expr(e, name, out);
            }
            // 2026-11-13 (owned-storage seam): the transfer REPLACED the
            // original Term/Tail — collect through its wrapped expression.
            HirStmtKind::TempTransfer { expr, ret, .. } if !matches!(ret, crate::hir::stmt::RetKind::EndProgram) => {
                collect_tail_self_calls_expr(expr, name, out);
            }
            HirStmtKind::Guarded(_, inner) | HirStmtKind::Block(inner) => {
                collect_tail_self_calls(inner, name, out);
            }
            _ => {}
        }
    }
}

fn collect_tail_self_calls_expr(expr: &HirExpr, name: &str, out: &mut Vec<Vec<HirExpr>>) {
    match &expr.kind {
        HirExprKind::Call { callee, args, .. } if callee == name => out.push(args.clone()),
        HirExprKind::Match { arms, .. } => {
            for arm in arms {
                collect_tail_self_calls_expr(&arm.body, name, out);
            }
        }
        HirExprKind::Block(stmts) => {
            for stmt in stmts.iter().rev() {
                match &stmt.kind {
                    HirStmtKind::Term(Some(e))
                    | HirStmtKind::Tail(e)
                    | HirStmtKind::Expression(e) => {
                        collect_tail_self_calls_expr(e, name, out);
                        break;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn tail_calls_in_body(body: &[HirStmt], name: &str) -> usize {
    let mut total = 0;
    for stmt in body {
        match &stmt.kind {
            HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e) => {
                total += tail_calls_in_expr(e, name)
            }
            // 2026-11-13 (owned-storage seam): the transfer REPLACED the
            // original Term/Tail — its wrapped expression is the tail.
            HirStmtKind::TempTransfer { expr, ret, .. } if !matches!(ret, crate::hir::stmt::RetKind::EndProgram) => {
                total += tail_calls_in_expr(expr, name)
            }
            HirStmtKind::Guarded(_, inner) | HirStmtKind::Block(inner) => {
                total += tail_calls_in_body(inner, name)
            }
            _ => {}
        }
    }
    total
}

fn tail_calls_in_expr(expr: &HirExpr, name: &str) -> usize {
    match &expr.kind {
        HirExprKind::Call { callee, .. } if callee == name => 1,
        HirExprKind::Match { arms, .. } => {
            arms.iter().map(|a| tail_calls_in_expr(&a.body, name)).sum()
        }
        HirExprKind::Block(stmts) => tail_calls_in_block(stmts, name),
        _ => 0,
    }
}

fn tail_calls_in_block(stmts: &[HirStmt], name: &str) -> usize {
    for stmt in stmts.iter().rev() {
        match &stmt.kind {
            // 2026-11-13 (owned-storage seam): a TempTransfer REPLACED the
            // original tail expression (bind → releases → return the
            // binding). Its wrapped expression occupies the same tail
            // position — count it, or AST/HIR tail classes drift whenever
            // the owned-storage pass converts a self-recursive tail.
            HirStmtKind::TempTransfer { expr, ret, .. } if !matches!(ret, crate::hir::stmt::RetKind::EndProgram) => {
                return tail_calls_in_expr(expr, name);
            }
            HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e) | HirStmtKind::Expression(e) => {
                return tail_calls_in_expr(e, name);
            }
            _ => {}
        }
    }
    0
}
// ── Full expression walk (total self-call count) ────────────────────────

fn count_self_calls_stmts(stmts: &[HirStmt], name: &str) -> usize {
    stmts.iter().map(|s| count_self_calls_stmt(s, name)).sum()
}

fn count_self_calls_stmt(stmt: &HirStmt, name: &str) -> usize {
    let expr = |e: &HirExpr| count_self_calls_expr(e, name);
    match &stmt.kind {
        HirStmtKind::Let { expr: Some(e), .. } => expr(e),
        HirStmtKind::Assign(l, r) => expr(l) + expr(r),
        HirStmtKind::ArrowAssign { target, value, .. } => {
            target.as_ref().map(|t| expr(t)).unwrap_or(0) + expr(value)
        }
        HirStmtKind::Term(Some(e))
        | HirStmtKind::EndProgram(Some(e))
        | HirStmtKind::Rollback(Some(e))
        | HirStmtKind::Tail(e)
        | HirStmtKind::Check(e)
        | HirStmtKind::Gate(e)
        | HirStmtKind::Expression(e) => expr(e),
        // 2026-11-13 (owned-storage seam): count the wrapped tail
        // expression — the transfer REPLACED the original Term/Tail.
        HirStmtKind::TempTransfer { expr: boxed, .. } => expr(boxed.as_ref()),
        HirStmtKind::Guarded(c, b) => expr(c) + count_self_calls_stmts(b, name),
        HirStmtKind::Block(b)
        | HirStmtKind::SyncBlock(b)
        | HirStmtKind::Defer(b)
        | HirStmtKind::Mutex(b)
        | HirStmtKind::Foreach { body: b, .. } => count_self_calls_stmts(b, name),
        HirStmtKind::Match { expr: e, arms } => {
            expr(e) + arms.iter().map(|a| count_self_calls_stmts(&a.body, name)).sum::<usize>()
        }
        HirStmtKind::TrgBinding { instance, .. } => expr(instance),
        HirStmtKind::Open(l, r) => expr(l) + expr(r),
        _ => 0,
    }
}

fn count_self_calls_expr(expr: &HirExpr, name: &str) -> usize {
    let rec = |e: &HirExpr| count_self_calls_expr(e, name);
    match &expr.kind {
        HirExprKind::Call { callee, args, .. } => {
            (callee == name) as usize + args.iter().map(rec).sum::<usize>()
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            rec(receiver) + args.iter().map(rec).sum::<usize>()
        }
        HirExprKind::Binary { left, right, .. } => rec(left) + rec(right),
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Reflect { receiver: inner, .. }
        | HirExprKind::Lambda { body: inner, .. } => rec(inner),
        HirExprKind::Index(a, b) => rec(a) + rec(b),
        HirExprKind::Slice { array, start, end, stride } => {
            rec(array)
                + start.as_ref().map(|e| rec(e)).unwrap_or(0)
                + end.as_ref().map(|e| rec(e)).unwrap_or(0)
                + stride.as_ref().map(|e| rec(e)).unwrap_or(0)
        }
        HirExprKind::Range { start, end, .. } => rec(start) + rec(end),
        HirExprKind::Spawn { args, .. } => args.iter().map(rec).sum(),
        HirExprKind::Block(stmts) => count_self_calls_stmts(stmts, name),
        HirExprKind::If { condition, then_branch, else_branch } => {
            rec(condition) + rec(then_branch) + else_branch.as_ref().map(|e| rec(e)).unwrap_or(0)
        }
        HirExprKind::Match { scrutinee, arms } => {
            rec(scrutinee) + arms.iter().map(|a| rec(&a.body)).sum::<usize>()
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => items.iter().map(rec).sum(),
        HirExprKind::StructLiteral { fields, .. } => fields.iter().map(|(_, e)| rec(e)).sum(),
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            args.iter().map(rec).sum::<usize>()
                + receiver.as_ref().map(|r| rec(r)).unwrap_or(0)
        }
        _ => 0,
    }
}
