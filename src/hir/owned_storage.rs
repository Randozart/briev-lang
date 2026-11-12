// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! HIR-owned String locals (M2 first slice).
//!
//! Consumes body-derived facts only: a callable returns a *fresh* String when
//! every return path allocates (String concatenation, Data->String copy, or a
//! call to another fresh callable). A local initialized from such a value owns
//! its backing. The pass inserts `Free#` at a proven owner-loss point
//! (reassignment, function exit) when the local is neither aliased, stored,
//! returned by name, nor passed to an escaping/consuming parameter.
//!
//! This is deliberately conservative: unknown calls, field/list stores, and
//! aliases drop ownership (leak) rather than risk a use-after-free. AST
//! emission is unchanged until the HIR path proves parity; the optimization
//! never changes program output.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::ids::NodeId;
use super::program::{HirCallable, HirProgram};
use super::stmt::{HirStmt, HirStmtKind, StorageKind};
use crate::ast::{BinaryOpKind, Type};
use std::collections::{HashMap, HashSet};

pub fn insert_releases(program: &mut HirProgram, escapes: &HashMap<String, Vec<bool>>) {
    let fresh = fresh_callables(program, escapes);
    let param_return = param_return_summaries(program);
    let consumed = consumed_params(program);
    let mut next = program
        .callables
        .iter()
        .map(|c| max_node(&c.body.statements) + 1)
        .max()
        .unwrap_or(0);
    for callable in &mut program.callables {
        // Conservative: generic bodies are emitted through the specialization
        // site, not the body seam; leave them leak-conservative for now.
        if !callable.type_params.is_empty() { continue; }
        let mut locals = HashSet::new();
        collect_let_names(&callable.body.statements, &mut locals);
        let mut ctx = Pass { fresh: &fresh, escapes, consumed: &consumed, param_return: &param_return, locals: &locals, next };
        let mut owned: HashMap<String, OwnKind> = HashMap::new();
        let mut fresh_vars = HashMap::new();
        for param in &callable.params {
            fresh_vars.insert(param.name.clone(), false);
        }
        let body = rewrite_list(
            &callable.body.statements,
            &mut owned,
            &mut fresh_vars,
            &mut ctx,
            true,
        );
        callable.body.statements = body;
        next = ctx.next;
    }
}

fn returns_owned(c: &HirCallable) -> bool {
    c.result.as_ref().is_some_and(owned_ty) || c.outputs.as_ref().is_some_and(|o| {
        o.all_types().iter().any(owned_ty)
    })
}

fn is_string(ty: &Type) -> bool {
    matches!(ty, Type::Custom(n) if n == "String")
}

fn is_list(ty: &Type) -> bool {
    matches!(ty, Type::Generic(n, _) | Type::Applied(n, _) if n == "List")
}

fn owned_ty(ty: &Type) -> bool {
    is_string(ty) || is_list(ty)
}

/// What a released owner needs: a String frees its handle; a List goes
/// through stdlib `coll_destroy` (object free + arena-aware data free).
#[derive(Clone, Copy, PartialEq, Debug)]
enum OwnKind { Str, Lst }

fn kind_of(ty: &Type) -> OwnKind {
    if is_string(ty) { OwnKind::Str } else { OwnKind::Lst }
}

fn max_node(stmts: &[HirStmt]) -> u32 {
    let mut max = 0;
    for s in stmts {
        max = max.max(s.id.0);
        match &s.kind {
            HirStmtKind::Guarded(c, b) => {
                max = max.max(expr_max(c));
                max = max.max(max_node(b));
            }
            HirStmtKind::Block(b)
            | HirStmtKind::SyncBlock(b)
            | HirStmtKind::Mutex(b)
            | HirStmtKind::Defer(b) => {
                max = max.max(max_node(b));
            }
            HirStmtKind::Foreach { list, body, .. } => {
                max = max.max(expr_max(list));
                max = max.max(max_node(body));
            }
            HirStmtKind::Match { expr, arms } => {
                max = max.max(expr_max(expr));
                for a in arms { max = max.max(max_node(&a.body)); }
            }
            HirStmtKind::Let { expr, .. } | HirStmtKind::Term(expr) | HirStmtKind::EndProgram(expr) => {
                if let Some(e) = expr { max = max.max(expr_max(e)); }
            }
            HirStmtKind::TempTransfer { expr, .. } => {
                max = max.max(expr_max(expr));
            }
            HirStmtKind::Assign(l, r) | HirStmtKind::ArrowAssign { value: l, target: Some(r), .. } => {
                max = max.max(expr_max(l)).max(expr_max(r));
            }
            HirStmtKind::ArrowAssign { value, .. } | HirStmtKind::Check(value)
            | HirStmtKind::Expression(value) | HirStmtKind::Gate(value) => {
                max = max.max(expr_max(value));
            }
            HirStmtKind::Tail(e) => max = max.max(expr_max(e)),
            _ => {}
        }
    }
    max
}

fn expr_max(e: &HirExpr) -> u32 {
    let mut max = e.id.0;
    match &e.kind {
        HirExprKind::Call { args, .. } | HirExprKind::List(args) | HirExprKind::Tuple(args) => {
            for a in args { max = max.max(expr_max(a)); }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            max = max.max(expr_max(receiver));
            for a in args { max = max.max(expr_max(a)); }
        }
        HirExprKind::Binary { left, right, .. } => {
            max = max.max(expr_max(left)).max(expr_max(right));
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Reflect { receiver: inner, .. }
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Deref(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => max = max.max(expr_max(inner)),
        HirExprKind::Index(a, b) => max = max.max(expr_max(a)).max(expr_max(b)),
        HirExprKind::Slice { array, start, end, stride } => {
            max = max.max(expr_max(array));
            for e in [start, end, stride].into_iter().flatten() { max = max.max(expr_max(e)); }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            max = max.max(expr_max(condition)).max(expr_max(then_branch));
            if let Some(e) = else_branch { max = max.max(expr_max(e)); }
        }
        HirExprKind::Match { scrutinee, arms } => {
            max = max.max(expr_max(scrutinee));
            for a in arms { if let Some(g) = &a.guard { max = max.max(expr_max(g)); } max = max.max(expr_max(&a.body)); }
        }
        HirExprKind::Block(stmts) => max = max.max(max_node(stmts)),
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, e) in fields.iter().chain(specs.iter()) { max = max.max(expr_max(e)); }
        }
        HirExprKind::Spawn { args, .. } => { for a in args { max = max.max(expr_max(a)); } }
        HirExprKind::Range { start, end, .. } => max = max.max(expr_max(start)).max(expr_max(end)),
        _ => {}
    }
    max
}

/// Callables whose EVERY return path is `Identifier(param_i)` verbatim:
/// builders/filters that hand their argument back (e.g. list `nms`-style
/// filters). A call to one is fresh exactly when that argument is fresh —
/// ownership flows through the identity return.
fn param_return_summaries(program: &HirProgram) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for c in &program.callables {
        let mut returns = Vec::new();
        collect_returns(&c.body.statements, &mut returns);
        if returns.is_empty() { continue; }
        let mut idx: Option<usize> = None;
        let mut all_param = true;
        for e in &returns {
            match &e.kind {
                HirExprKind::Identifier(n) => {
                    let i = c.params.iter().position(|p| &p.name == n);
                    match (i, idx) {
                        (Some(i), None) => idx = Some(i),
                        (Some(i), Some(j)) if i == j => {}
                        _ => { all_param = false; break; }
                    }
                }
                _ => { all_param = false; break; }
            }
        }
        if all_param { if let Some(i) = idx { out.insert(c.name.clone(), i); } }
    }
    out
}

fn fresh_callables(
    program: &HirProgram,
    escapes: &HashMap<String, Vec<bool>>,
) -> HashSet<String> {
    let mut fresh = HashSet::new();
    let pr = param_return_summaries(program);
    for _ in 0..program.callables.len().max(1) {
        let mut changed = false;
        for c in &program.callables {
            if !returns_owned(c) || fresh.contains(&c.name) { continue; }
            // Fresh-set soundness gate: if ANY parameter escapes (stored into
            // the result/state/collection), the callable must not count as
            // fresh — a `wrap(s) -> [s]` stasher would make the caller free a
            // retained operand. Escape-summary facts are the gate.
            if escapes.get(&c.name).is_some_and(|v| v.iter().any(|e| *e)) {
                continue;
            }
            let mut vars: HashMap<String, bool> = c.params.iter().map(|p| (p.name.clone(), false)).collect();
            collect_fresh(&c.body.statements, &mut vars, &fresh, &pr);
            let mut returns = Vec::new();
            collect_returns(&c.body.statements, &mut returns);
            if !returns.is_empty() && returns.iter().all(|e| is_fresh(e, &vars, &pr, &fresh)) {
                fresh.insert(c.name.clone());
                changed = true;
            }
        }
        if !changed { break; }
    }
    fresh
}

fn collect_fresh(stmts: &[HirStmt], vars: &mut HashMap<String, bool>, fresh: &HashSet<String>, pr: &HashMap<String, usize>) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Let { name, expr: Some(e), ty, .. } => {
                let fresh = is_fresh_rhs(e, vars, pr, fresh);
                let own = ty.as_ref().is_some_and(owned_ty) || e.ty.as_ref().is_some_and(owned_ty);
                vars.insert(name.clone(), fresh && own);
            }
            HirStmtKind::Assign(l, r) => {
                if let HirExprKind::Identifier(n) = &l.kind {
                    let fresh = is_fresh_rhs(r, vars, pr, fresh);
                    vars.insert(n.clone(), fresh);
                }
            }
            HirStmtKind::Guarded(_, b) | HirStmtKind::Block(b)
            | HirStmtKind::SyncBlock(b) | HirStmtKind::Mutex(b) | HirStmtKind::Defer(b) => {
                collect_fresh(b, vars, fresh, pr);
            }
            HirStmtKind::Foreach { body, .. } => collect_fresh(body, vars, fresh, pr),
            HirStmtKind::Match { arms, .. } => {
                for a in arms { collect_fresh(&a.body, vars, fresh, pr); }
            }
            _ => {}
        }
    }
}

fn collect_returns(stmts: &[HirStmt], out: &mut Vec<HirExpr>) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e) => out.push(e.clone()),
            HirStmtKind::TempTransfer { expr, .. } => out.push((**expr).clone()),
            HirStmtKind::Guarded(_, b) | HirStmtKind::Block(b)
            | HirStmtKind::SyncBlock(b) | HirStmtKind::Mutex(b) | HirStmtKind::Defer(b) => {
                collect_returns(b, out);
            }
            HirStmtKind::Foreach { body, .. } => collect_returns(body, out),
            HirStmtKind::Match { arms, .. } => {
                for a in arms { collect_returns(&a.body, out); }
            }
            _ => {}
        }
    }
}

fn is_fresh(e: &HirExpr, vars: &HashMap<String, bool>, pr: &HashMap<String, usize>, fresh: &HashSet<String>) -> bool {
    match &e.kind {
        HirExprKind::List(_) => true,
        HirExprKind::Call { callee, args, .. } => {
            if fresh.contains(callee) {
                true
            } else if let Some(i) = pr.get(callee) {
                args.get(*i).is_some_and(|a| is_fresh(a, vars, pr, fresh))
            } else {
                false
            }
        }
        HirExprKind::Binary { op: BinaryOpKind::Add, left, right } => {
            string_expr(left) && string_expr(right)
        }
        HirExprKind::Cast { inner, to, .. } => is_string(to) && !string_expr(inner),
        HirExprKind::Identifier(n) => vars.get(n).copied().unwrap_or(false),
        HirExprKind::If { then_branch, else_branch, .. } => {
            else_branch.as_ref().is_some_and(|e| is_fresh(then_branch, vars, pr, fresh) && is_fresh(e, vars, pr, fresh))
        }
        HirExprKind::Match { arms, .. } => !arms.is_empty() && arms.iter().all(|a| is_fresh(&a.body, vars, pr, fresh)),
        _ => false,
    }
}

fn is_fresh_rhs(e: &HirExpr, vars: &HashMap<String, bool>, pr: &HashMap<String, usize>, fresh: &HashSet<String>) -> bool {
    !matches!(e.kind, HirExprKind::Identifier(_)) && is_fresh(e, vars, pr, fresh)
}

fn string_expr(e: &HirExpr) -> bool {
    e.ty.as_ref().is_some_and(is_string) || match &e.kind {
        HirExprKind::Literal(HirLiteral::Bytes(_)) => true,
        HirExprKind::Identifier(_) => true,
        HirExprKind::Binary { op: BinaryOpKind::Add, left, right } => string_expr(left) && string_expr(right),
        _ => false,
    }
}

fn consumed_params(program: &HirProgram) -> HashMap<String, HashSet<usize>> {
    let mut consumed: HashMap<String, HashSet<usize>> = HashMap::new();
    for c in &program.callables {
        let mut names = HashSet::new();
        consumed_in(&c.body.statements, &mut names);
        let mut set = HashSet::new();
        for (i, p) in c.params.iter().enumerate() {
            if names.contains(&p.name) { set.insert(i); }
        }
        if !set.is_empty() { consumed.insert(c.name.clone(), set); }
    }
    consumed
}

fn consumed_in(stmts: &[HirStmt], out: &mut HashSet<String>) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Expression(e) | HirStmtKind::Check(e) | HirStmtKind::Gate(e) => consumed_expr(e, out),
            HirStmtKind::Let { expr: Some(e), .. } | HirStmtKind::Term(Some(e)) => consumed_expr(e, out),
            HirStmtKind::Assign(l, r) | HirStmtKind::ArrowAssign { value: l, target: Some(r), .. } => {
                consumed_expr(l, out); consumed_expr(r, out);
            }
            HirStmtKind::FreeHint(n) | HirStmtKind::KeepHint(n) => { out.insert(n.clone()); }
            HirStmtKind::ReleaseOwned { name, .. } => { out.insert(name.clone()); }
            HirStmtKind::TempTransfer { expr, releases, .. } => {
                consumed_expr(expr, out);
                for (_, n) in releases { out.insert(n.clone()); }
            }
            HirStmtKind::Guarded(c, b) => {
                consumed_expr(c, out); consumed_in(b, out);
            }
            HirStmtKind::Block(b) | HirStmtKind::SyncBlock(b)
            | HirStmtKind::Mutex(b) | HirStmtKind::Defer(b) => {
                consumed_in(b, out);
            }
            HirStmtKind::Foreach { list, body, .. } => { consumed_expr(list, out); consumed_in(body, out); }
            HirStmtKind::Match { expr, arms } => {
                consumed_expr(expr, out);
                for a in arms { consumed_in(&a.body, out); }
            }
            _ => {}
        }
    }
}

fn consumed_expr(e: &HirExpr, out: &mut HashSet<String>) {
    match &e.kind {
        HirExprKind::Call { callee, args, .. } => {
            if callee == "Free#" {
                for a in args { if let HirExprKind::Identifier(n) = &a.kind { out.insert(n.clone()); } }
            }
            for a in args { consumed_expr(a, out); }
        }
        HirExprKind::Consume(inner) => {
            if let HirExprKind::Identifier(n) = &inner.kind { out.insert(n.clone()); }
            consumed_expr(inner, out);
        }
        HirExprKind::Binary { left, right, .. } => { consumed_expr(left, out); consumed_expr(right, out); }
        HirExprKind::Unary { inner, .. } | HirExprKind::Cast { inner, .. }
        | HirExprKind::Field { receiver: inner, .. } | HirExprKind::AddrOf(inner)
        | HirExprKind::Deref(inner) | HirExprKind::Await(inner) => consumed_expr(inner, out),
        HirExprKind::Index(a, b) => { consumed_expr(a, out); consumed_expr(b, out); }
        HirExprKind::If { condition, then_branch, else_branch } => {
            consumed_expr(condition, out); consumed_expr(then_branch, out);
            if let Some(e) = else_branch { consumed_expr(e, out); }
        }
        HirExprKind::Block(stmts) => consumed_in(stmts, out),
        HirExprKind::List(items) | HirExprKind::Tuple(items) => { for e in items { consumed_expr(e, out); } }
        HirExprKind::Match { scrutinee, arms } => {
            consumed_expr(scrutinee, out);
            for a in arms { consumed_expr(&a.body, out); }
        }
        _ => {}
    }
}

struct Pass<'a> {
    fresh: &'a HashSet<String>,
    escapes: &'a HashMap<String, Vec<bool>>,
    consumed: &'a HashMap<String, HashSet<usize>>,
    param_return: &'a HashMap<String, usize>,
    locals: &'a HashSet<String>,
    next: u32,
}

fn collect_let_names(stmts: &[HirStmt], out: &mut HashSet<String>) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Let { name, .. } => { out.insert(name.clone()); }
            HirStmtKind::Guarded(_, b) | HirStmtKind::Block(b) | HirStmtKind::SyncBlock(b)
            | HirStmtKind::Mutex(b) | HirStmtKind::Defer(b) => collect_let_names(b, out),
            HirStmtKind::Foreach { body, .. } => collect_let_names(body, out),
            HirStmtKind::Match { arms, .. } => for a in arms { collect_let_names(&a.body, out); },
            _ => {}
        }
    }
}

fn rewrite_list(
    stmts: &[HirStmt],
    owned: &mut HashMap<String, OwnKind>,
    fresh_vars: &mut HashMap<String, bool>,
    ctx: &mut Pass<'_>,
    top: bool,
) -> Vec<HirStmt> {
    let entry: HashSet<String> = owned.keys().cloned().collect();
    let declared: HashSet<String> = stmts.iter().filter_map(|s| match &s.kind {
        HirStmtKind::Let { name, .. } => Some(name.clone()),
        _ => None,
    }).collect();
    let mut out = Vec::with_capacity(stmts.len());
    for stmt in stmts {
        let mut s = stmt.clone();
        let stmt_kind_tag = stmt.kind.clone();
        match &mut s.kind {
            HirStmtKind::Let { name, expr: Some(e), ty, .. } => {
                uses(e, owned, ctx);
                let own_flag = ty.as_ref().is_some_and(owned_ty) || e.ty.as_ref().is_some_and(owned_ty);
                let kind = ty.as_ref().or(e.ty.as_ref()).map(kind_of).unwrap_or(OwnKind::Str);
                let fresh = own_flag && is_fresh_rhs(e, fresh_vars, ctx.param_return, ctx.fresh);
                if own_flag {
                    fresh_vars.insert(name.clone(), fresh);
                }
                if fresh {
                    owned.insert(name.clone(), kind);
                } else {
                    owned.remove(name);
                    if matches!(e.kind, HirExprKind::Identifier(_)) {
                        let mut ids = HashSet::new();
                        mention(e, &mut ids);
                        for id in ids { owned.remove(&id); }
                    }
                }
                out.push(s);
            }
            HirStmtKind::Assign(lhs, rhs) => {
                uses(rhs, owned, ctx);
                let target = if let HirExprKind::Identifier(n) = &lhs.kind { Some(n.clone()) } else { None };
                let mut rhs_ids = HashSet::new();
                mention(rhs, &mut rhs_ids);
                if let Some(n) = &target {
                    if !ctx.locals.contains(n) {
                        for id in rhs_ids { owned.remove(&id); }
                        out.push(s);
                        continue;
                    }
                    if let Some(kind) = owned.get(n).copied() {
                        if !rhs_ids.contains(n) {
                            out.push(release(n, kind, ctx));
                            owned.remove(n);
                        }
                    }
                    let own_flag = lhs.ty.as_ref().is_some_and(owned_ty) || rhs.ty.as_ref().is_some_and(owned_ty);
                    let kind = lhs.ty.as_ref().or(rhs.ty.as_ref()).map(kind_of).unwrap_or(OwnKind::Str);
                    let fresh = own_flag && is_fresh_rhs(rhs, fresh_vars, ctx.param_return, ctx.fresh);
                    fresh_vars.insert(n.clone(), fresh);
                    if fresh { owned.insert(n.clone(), kind); } else { owned.remove(n); }
                    if matches!(rhs.kind, HirExprKind::Identifier(_)) {
                        for id in rhs_ids { owned.remove(&id); }
                    }
                } else {
                    for id in rhs_ids { owned.remove(&id); }
                }
                out.push(s);
            }
            HirStmtKind::Expression(e) | HirStmtKind::Check(e) | HirStmtKind::Gate(e) => {
                uses(e, owned, ctx);
                out.push(s);
            }
            HirStmtKind::FreeHint(n) => { owned.remove(n); out.push(s); }
            HirStmtKind::KeepHint(n) => { owned.remove(n); out.push(s); }
            HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e) | HirStmtKind::EndProgram(Some(e)) => {
                uses(e, owned, ctx);
                let transferred = if let HirExprKind::Identifier(n) = &e.kind { Some(n.clone()) } else { None };
                let mut ids = HashSet::new();
                mention(e, &mut ids);
                if let Some(n) = &transferred { owned.remove(n); }
                // Temp-transfer (2026-11-12): a FRESH return expression that
                // borrows owned locals evaluates the allocation first, so the
                // borrowed operands can be released right after — bind the
                // result to a synthetic local, release the operands, return
                // the binding. Not-fresh returns keep the conservative drop.
                let ret_fresh = is_fresh(e, fresh_vars, ctx.param_return, ctx.fresh);
                let borrows_owned = transferred.is_none()
                    && ids.iter().any(|n| owned.contains_key(n));
                if ret_fresh && borrows_owned {
                    let mut tname = format!("__ot{}", ctx.next);
                    ctx.next += 1;
                    while ctx.locals.contains(&tname) {
                        tname = format!("{}x", tname);
                        ctx.next += 1;
                    }
                    let names: Vec<String> = owned.keys().cloned().collect();
                    let releases: Vec<(StorageKind, String)> = names
                        .iter()
                        .map(|n| {
                            let k = owned.get(n).copied().unwrap_or(OwnKind::Str);
                            (match k { OwnKind::Str => StorageKind::Str, OwnKind::Lst => StorageKind::List }, n.clone())
                        })
                        .collect();
                    let ret = match stmt_kind_tag {
                        HirStmtKind::Tail(_) => crate::hir::stmt::RetKind::Tail,
                        HirStmtKind::EndProgram(_) => crate::hir::stmt::RetKind::EndProgram,
                        _ => crate::hir::stmt::RetKind::Term,
                    };
                    let stmt = HirStmt {
                        id: NodeId(ctx.next),
                        span: None,
                        kind: HirStmtKind::TempTransfer {
                            name: tname,
                            ty: e.ty.clone(),
                            expr: Box::new(e.clone()),
                            releases,
                            ret,
                        },
                    };
                    ctx.next += 1;
                    out.push(stmt);
                    owned.clear();
                    continue;
                }
                let names: Vec<String> = owned.keys().cloned().collect();
                for n in names {
                    let k = owned.get(&n).copied();
                    if !ids.contains(&n) {
                        if let Some(k) = k { out.push(release(&n, k, ctx)); }
                        owned.remove(&n);
                    } else {
                        owned.remove(&n);
                    }
                }
                out.push(s);
            }
            HirStmtKind::Guarded(cond, body) => {
                uses(cond, owned, ctx);
                let before: HashSet<String> = owned.keys().cloned().collect();
                let mut inner = owned.clone();
                let mut inner_fresh = fresh_vars.clone();
                let new_body = rewrite_list(body, &mut inner, &mut inner_fresh, ctx, false);
                for n in before { if !inner.contains_key(&n) { owned.remove(&n); } }
                owned.extend(inner);
                *fresh_vars = inner_fresh;
                *body = new_body;
                out.push(s);
            }
            HirStmtKind::Block(body) | HirStmtKind::SyncBlock(body)
            | HirStmtKind::Mutex(body) | HirStmtKind::Defer(body) => {
                let before: HashSet<String> = owned.keys().cloned().collect();
                let mut inner = owned.clone();
                let mut inner_fresh = fresh_vars.clone();
                let new_body = rewrite_list(body, &mut inner, &mut inner_fresh, ctx, false);
                for n in before { if !inner.contains_key(&n) { owned.remove(&n); } }
                owned.extend(inner);
                *fresh_vars = inner_fresh;
                *body = new_body;
                out.push(s);
            }
            HirStmtKind::Foreach { list, body, .. } => {
                uses(list, owned, ctx);
                let before: HashSet<String> = owned.keys().cloned().collect();
                let mut inner = owned.clone();
                let mut inner_fresh = fresh_vars.clone();
                let new_body = rewrite_list(body, &mut inner, &mut inner_fresh, ctx, false);
                for n in before { if !inner.contains_key(&n) { owned.remove(&n); } }
                owned.extend(inner);
                *fresh_vars = inner_fresh;
                *body = new_body;
                out.push(s);
            }
            HirStmtKind::Match { expr, arms } => {
                uses(expr, owned, ctx);
                let before: HashSet<String> = owned.keys().cloned().collect();
                let mut kept: Option<HashMap<String, OwnKind>> = None;
                for arm in arms.iter_mut() {
                    let mut inner = owned.clone();
                    let mut inner_fresh = fresh_vars.clone();
                    let new_body = rewrite_list(&arm.body, &mut inner, &mut inner_fresh, ctx, false);
                    for n in &before { if !inner.contains_key(n) { owned.remove(n); } }
                    kept = Some(match kept { Some(k) => k.into_iter().filter(|(n, _)| inner.contains_key(n)).collect(), None => inner });
                    arm.body = new_body;
                }
                if let Some(k) = kept { owned.extend(k); }
                out.push(s);
            }
            _ => out.push(s),
        }
    }
    if top {
        let names: Vec<(String, OwnKind)> = owned.iter().map(|(n, k)| (n.clone(), *k)).collect();
        for (n, k) in names { out.push(release(&n, k, ctx)); }
        owned.clear();
    } else {
        let added: Vec<(String, OwnKind)> = owned.iter()
            .filter(|(n, _)| !entry.contains(*n) && declared.contains(*n))
            .map(|(n, k)| (n.clone(), *k)).collect();
        for (n, k) in added { out.push(release(&n, k, ctx)); owned.remove(&n); }
    }
    out
}

fn release(name: &str, kind: OwnKind, ctx: &mut Pass<'_>) -> HirStmt {
    let id = NodeId(ctx.next);
    ctx.next += 1;
    HirStmt {
        id,
        span: None,
        kind: HirStmtKind::ReleaseOwned {
            storage: match kind { OwnKind::Str => StorageKind::Str, OwnKind::Lst => StorageKind::List },
            name: name.to_string(),
        },
    }
}

fn mention(e: &HirExpr, out: &mut HashSet<String>) {
    match &e.kind {
        HirExprKind::Identifier(n) => { out.insert(n.clone()); }
        HirExprKind::Call { args, .. } | HirExprKind::List(args) | HirExprKind::Tuple(args) => {
            for a in args { mention(a, out); }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            mention(receiver, out);
            for a in args { mention(a, out); }
        }
        HirExprKind::Binary { left, right, .. } => { mention(left, out); mention(right, out); }
        HirExprKind::Unary { inner, .. } | HirExprKind::Cast { inner, .. }
        | HirExprKind::Field { receiver: inner, .. } | HirExprKind::Reflect { receiver: inner, .. }
        | HirExprKind::AddrOf(inner) | HirExprKind::Deref(inner) | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => mention(inner, out),
        HirExprKind::Index(a, b) => { mention(a, out); mention(b, out); }
        HirExprKind::If { condition, then_branch, else_branch } => {
            mention(condition, out); mention(then_branch, out);
            if let Some(e) = else_branch { mention(e, out); }
        }
        HirExprKind::Block(stmts) => for s in stmts { mention_stmt(s, out); },
        HirExprKind::Match { scrutinee, arms } => {
            mention(scrutinee, out);
            for a in arms { mention(&a.body, out); }
        }
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, e) in fields.iter().chain(specs.iter()) { mention(e, out); }
        }
        _ => {}
    }
}

fn mention_stmt(s: &HirStmt, out: &mut HashSet<String>) {
    match &s.kind {
        HirStmtKind::Let { expr: Some(e), .. } | HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e)
        | HirStmtKind::Check(e) | HirStmtKind::Expression(e) | HirStmtKind::Gate(e) => mention(e, out),
        HirStmtKind::Assign(l, r) => { mention(l, out); mention(r, out); }
        HirStmtKind::TempTransfer { expr, releases, .. } => {
            mention(expr, out);
            for (_, n) in releases { out.insert(n.clone()); }
        }
        HirStmtKind::ReleaseOwned { name, .. } => { out.insert(name.clone()); }
        HirStmtKind::Guarded(c, b) => { mention(c, out); for s in b { mention_stmt(s, out); } }
        HirStmtKind::Block(b) | HirStmtKind::SyncBlock(b)
        | HirStmtKind::Mutex(b) | HirStmtKind::Defer(b) => { for s in b { mention_stmt(s, out); } }
        HirStmtKind::Foreach { list, body, .. } => { mention(list, out); for s in body { mention_stmt(s, out); } }
        HirStmtKind::Match { expr, arms } => { mention(expr, out); for a in arms { for s in &a.body { mention_stmt(s, out); } } }
        _ => {}
    }
}

fn uses(e: &HirExpr, owned: &mut HashMap<String, OwnKind>, ctx: &Pass<'_>) {
    match &e.kind {
        HirExprKind::Call { callee, args, .. } => {
            // A fresh-set callee provably allocates its own result and (by
            // the escape gate above) lets no argument escape: its args are
            // reads. Identity-return callees (param_return) still hit the
            // Escape path so the caller drops the handle the result now owns.
            let fresh_callee = ctx.fresh.contains(callee);
            for (i, a) in args.iter().enumerate() {
                let mode = if callee == "Free#" { Mode::Consume }
                    else if fresh_callee { Mode::Borrow }
                    else if ctx.consumed.get(callee).is_some_and(|s| s.contains(&i)) { Mode::Consume }
                    else if let Some(summary) = ctx.escapes.get(callee) {
                        if summary.get(i).copied().unwrap_or(true) { Mode::Escape } else { Mode::Borrow }
                    } else if callee.ends_with('#') { Mode::Borrow }
                    else { Mode::Escape };
                use_arg(a, mode, owned, ctx);
            }
        }
        HirExprKind::Consume(inner) => use_arg(inner, Mode::Consume, owned, ctx),
        HirExprKind::AddrOf(inner) => use_arg(inner, Mode::Escape, owned, ctx),
        // 2026-11-12: MethodCall receivers borrow, like Field reads. Same
        // trust class: a user method that STASHES its receiver escapes it
        // silently — recorded as the known limitation of this slice; stdlib
        // methods (Count#, iteration) are pure.
        HirExprKind::MethodCall { receiver, args, .. } => {
            use_arg(receiver, Mode::Borrow, owned, ctx);
            for a in args { use_arg(a, Mode::Borrow, owned, ctx); }
        }
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, v) in fields.iter().chain(specs.iter()) { use_arg(v, Mode::Escape, owned, ctx); }
        }
        HirExprKind::List(items) | HirExprKind::Tuple(items) => {
            for v in items { use_arg(v, Mode::Escape, owned, ctx); }
        }
        HirExprKind::Binary { left, right, .. } => { use_arg(left, Mode::Borrow, owned, ctx); use_arg(right, Mode::Borrow, owned, ctx); }
        HirExprKind::Unary { inner, .. } | HirExprKind::Cast { inner, .. }
        | HirExprKind::Field { receiver: inner, .. } | HirExprKind::Reflect { receiver: inner, .. }
        | HirExprKind::Deref(inner) | HirExprKind::Await(inner) => use_arg(inner, Mode::Borrow, owned, ctx),
        HirExprKind::Index(a, b) => { use_arg(a, Mode::Borrow, owned, ctx); use_arg(b, Mode::Borrow, owned, ctx); }
        HirExprKind::If { condition, then_branch, else_branch } => {
            use_arg(condition, Mode::Borrow, owned, ctx);
            use_arg(then_branch, Mode::Borrow, owned, ctx);
            if let Some(e) = else_branch { use_arg(e, Mode::Borrow, owned, ctx); }
        }
        HirExprKind::Block(stmts) => {
            for s in stmts { use_stmt(s, owned, ctx); }
        }
        HirExprKind::Match { scrutinee, arms } => {
            use_arg(scrutinee, Mode::Borrow, owned, ctx);
            for a in arms { use_arg(&a.body, Mode::Borrow, owned, ctx); }
        }
        _ => {}
    }
}

fn use_stmt(s: &HirStmt, owned: &mut HashMap<String, OwnKind>, ctx: &Pass<'_>) {
    match &s.kind {
        HirStmtKind::Let { expr: Some(e), .. } | HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e)
        | HirStmtKind::Check(e) | HirStmtKind::Expression(e) | HirStmtKind::Gate(e) => uses(e, owned, ctx),
        HirStmtKind::Assign(l, r) => { uses(l, owned, ctx); uses(r, owned, ctx); }
        HirStmtKind::TempTransfer { expr, .. } => uses(expr, owned, ctx),
        HirStmtKind::ReleaseOwned { .. } => {}
        HirStmtKind::Guarded(c, b) => { uses(c, owned, ctx); for s in b { use_stmt(s, owned, ctx); } }
        HirStmtKind::Block(b) | HirStmtKind::SyncBlock(b)
        | HirStmtKind::Mutex(b) | HirStmtKind::Defer(b) => { for s in b { use_stmt(s, owned, ctx); } }
        HirStmtKind::Foreach { list, body, .. } => { uses(list, owned, ctx); for s in body { use_stmt(s, owned, ctx); } }
        HirStmtKind::Match { expr, arms } => { uses(expr, owned, ctx); for a in arms { for s in &a.body { use_stmt(s, owned, ctx); } } }
        _ => {}
    }
}

#[derive(Clone, Copy)]
enum Mode { Borrow, Consume, Escape }

fn use_arg(e: &HirExpr, mode: Mode, owned: &mut HashMap<String, OwnKind>, ctx: &Pass<'_>) {
    match mode {
        Mode::Borrow => uses(e, owned, ctx),
        Mode::Consume | Mode::Escape => {
            let mut ids = HashSet::new();
            mention(e, &mut ids);
            for id in ids { owned.remove(&id); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casting::graph::CastingGraph;
    use crate::hir::lower_program;
    use crate::type_universe::TypeUniverse;

    fn parse(src: &str) -> Vec<crate::ast::TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    fn annotated(src: &str) -> HirProgram {
        let mut program = lower_program(&parse(src));
        let universe = TypeUniverse::new();
        let graph = CastingGraph::new();
        crate::hir::typing::annotate_types(&mut program, &universe, &graph);
        program
    }

    /// Collect every release the pass inserted (or authored Free# calls on
    /// plain identifiers), so tests assert on a flat list.
    fn releases_of(c: &HirCallable) -> Vec<(StorageKind, String)> {
        fn walk(stmts: &[HirStmt], out: &mut Vec<(StorageKind, String)>) {
            for st in stmts {
                match &st.kind {
                    HirStmtKind::ReleaseOwned { storage, name } => {
                        out.push((*storage, name.clone()));
                    }
                    HirStmtKind::TempTransfer { releases, .. } => {
                        out.extend(releases.iter().cloned());
                    }
                    HirStmtKind::Expression(HirExpr { kind: HirExprKind::Call { callee, args, .. }, .. }) => {
                        if callee == "Free#" {
                            for a in args {
                                if let HirExprKind::Identifier(n) = &a.kind {
                                    out.push((StorageKind::Str, n.clone()));
                                }
                            }
                        }
                    }
                    HirStmtKind::Guarded(_, b) | HirStmtKind::Block(b)
                    | HirStmtKind::SyncBlock(b) | HirStmtKind::Mutex(b) | HirStmtKind::Defer(b) => walk(b, out),
                    HirStmtKind::Foreach { body, .. } => walk(body, out),
                    HirStmtKind::Match { arms, .. } => for a in arms { walk(&a.body, out); },
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&c.body.statements, &mut out);
        out
    }

    fn has_free(c: &HirCallable, name: &str) -> bool {
        releases_of(c).iter().any(|(k, n)| *k == StorageKind::Str && n == name)
    }

    fn has_destroy(c: &HirCallable, name: &str) -> bool {
        releases_of(c).iter().any(|(k, n)| *k == StorageKind::List && n == name)
    }

    #[test]
    fn list_literal_owner_is_destroyed_at_exit() {
        let mut program = annotated(
            r#"
defn use() -> Int { let xs: List<Int> = [1, 2]; let n: Int = xs.Count#(); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(has_destroy(use_fn, "xs"), "dead owned List must be destroyed");
        assert!(!has_free(use_fn, "xs"), "List must not go through Free#");
    }

    #[test]
    fn param_identity_return_transfers_list_ownership() {
        let mut program = annotated(
            r#"
defn passthrough(xs: List<Int>) -> List<Int> { term xs; };
defn use() -> Int { let xs: List<Int> = [7]; let ys: List<Int> = passthrough(xs); let n: Int = ys.Count#(); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(has_destroy(use_fn, "ys"), "the transferred owner must be destroyed");
        assert!(!has_destroy(use_fn, "xs"), "the pre-transfer handle must not double-free");
    }

    #[test]
    fn fresh_return_borrowing_owned_local_temp_transfers() {
        let mut program = annotated(
            r#"
defn mk() -> String { term "a" + "b"; };
defn cat(a: String, b: String) -> String { term a + b; };
defn use() -> String { let s: String = mk(); term cat(s, "!"); };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(has_free(use_fn, "s"), "the borrowed operand must free after the fresh call evaluates");
        let last = use_fn.body.statements.last().unwrap();
        match &last.kind {
            HirStmtKind::TempTransfer { name, .. } => {
                assert!(name.starts_with("__ot"), "expected temp-transfer binding, got {name}");
            }
            other => panic!("expected temp-transfer tail, got {other:?}"),
        }
    }

    #[test]
    fn infers_fresh_string_call_and_releases_dead_local() {
        let mut program = annotated(
            r#"
defn make() -> String { term "a" + "b"; };
defn use() -> Int { let s: String = make(); let n: Int = CharCount#(s); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(has_free(use_fn, "s"), "dead owned String must be released");
    }

    #[test]
    fn alias_keeps_ownership_conservative() {
        let mut program = annotated(
            r#"
defn make() -> String { term "a" + "b"; };
defn use() -> Int { let s: String = make(); let t: String = s; let n: Int = CharCount#(t); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(!has_free(use_fn, "s"), "aliased String must not be freed");
        assert!(!has_free(use_fn, "t"), "alias is not an owner");
    }

    #[test]
    fn top_level_state_string_is_not_a_local_owner() {
        let mut program = annotated(
            r#"
let cache: String = "x";
defn store() -> Void { cache = "a" + "b"; term; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let store = program.callables.iter().find(|c| c.name == "store").unwrap();
        assert!(!has_free(store, "cache"), "global/state storage is not a local owner");
    }

    #[test]
    fn keep_suppresses_release() {
        let mut program = annotated(
            r#"
defn make() -> String { term "a" + "b"; };
defn use() -> Int { let s: String = make(); keep s; let n: Int = CharCount#(s); term n; };
"#,
        );
        insert_releases(&mut program, &HashMap::new());
        let use_fn = program.callables.iter().find(|c| c.name == "use").unwrap();
        assert!(!has_free(use_fn, "s"), "keep must suppress auto-release");
    }
}
