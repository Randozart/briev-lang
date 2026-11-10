// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.1 — `needs_state` / `needs_plain` over typed HIR.
//!
//! Mirror of `analysis::export_abi` (the AST implementation, still
//! authoritative until corpus parity is proven). Same least-fixpoint semantics:
//! a defn needs state iff its body (directly or transitively) touches a state
//! field, a transaction, a state-carrying intrinsic, an `asm`/`bad` fn, an
//! arrow assign, a spawn/await, or a stateful member reached by method call.
//!
//! Keys match the AST index: bare defn names, `Type::member` for obj/impl
//! members, bare names for trait defaults. Top-level `let`/`state` globals are
//! the state-leaf set (minus plain); constants are excluded at lowering.
//!
//! No consumer uses this yet; `tests/hir_needs_state_parity.rs` compares it to
//! the AST result over the corpus.

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use std::collections::{HashMap, HashSet};

/// Intrinsics whose lowering emits `ptr %state` (mirrors the AST list).
const STATE_INTRINSICS: &[&str] = &[
    "Malloc#", "Memcpy#", "Memmove#", "Memset#", "Print#",
    "FileRead#", "FileWrite#", "ShellCmd#", "SysQuery#", "EnvGet#", "HttpFetch#",
    "AllocArray#", "AllocInitArray#", "StringNew#", "StringFromPtr#", "StringConcat#",
    "TaskCall#", "GetCwd#", "ChDir#",
];

fn member_key(owner: &str, name: &str) -> String {
    format!("{owner}::{name}")
}

/// Index the program's callables exactly as the AST analysis does.
fn index<'a>(
    program: &'a HirProgram,
    plain: &HashSet<String>,
) -> (
    HashMap<String, &'a HirCallable>,
    HashSet<String>,
    HashSet<String>,
) {
    let mut regular: HashMap<String, &HirCallable> = HashMap::new();
    let mut txns: HashSet<String> = HashSet::new();
    for c in &program.callables {
        match c.kind {
            // Member transactions are reactor-threaded but are NOT indexed as
            // fixpoint nodes by the AST (index_type_members indexes Definition
            // members only), so mirror that.
            HirCallableKind::Transaction | HirCallableKind::CompileTimeTxn
                if c.owner.is_none() =>
            {
                txns.insert(c.name.clone());
            }
            // Compile-time callables are not runtime emitters and are not
            // indexed by the AST analysis; member txns are reactor-threaded
            // but likewise not fixpoint nodes.
            HirCallableKind::Transaction
            | HirCallableKind::CompileTimeTxn
            | HirCallableKind::CompileTimeDefn
            | HirCallableKind::MemberTransaction
            | HirCallableKind::IsrHandler => {}
            HirCallableKind::Defn | HirCallableKind::MemberDefn => {
                let key = if c.trait_default {
                    c.name.clone()
                } else if let Some(owner) = &c.owner {
                    member_key(owner, &c.name)
                } else {
                    c.name.clone()
                };
                regular.insert(key, c);
            }
        }
    }
    let state_fields: HashSet<String> = program
        .globals
        .iter()
        .filter(|g| !plain.contains(&g.name))
        .map(|g| g.name.clone())
        .collect();
    (regular, txns, state_fields)
}

/// `needs_state` for every defn (exported and regular).
pub fn compute_defn_needs_state(
    program: &HirProgram,
    plain: &HashSet<String>,
) -> HashMap<String, bool> {
    let (regular, txns, state_fields) = index(program, plain);
    let always: HashSet<String> = program.always_stateful.iter().cloned().collect();
    fixpoint(&regular, &txns, &state_fields, &always, true)
}

/// `needs_plain` — separate least fixpoint over the plain set, with no txn /
/// always-stateful leaves and no local shadowing (mirrors the AST).
pub fn compute_defn_needs_plain(
    program: &HirProgram,
    plain: &HashSet<String>,
) -> HashMap<String, bool> {
    let (regular, _txns, _state_fields) = index(program, plain);
    let none: HashSet<String> = HashSet::new();
    fixpoint(&regular, &none, plain, &none, false)
}

/// `needs_state` for exported defns (GLUE ABI subset).
pub fn compute_export_needs_state(
    program: &HirProgram,
    plain: &HashSet<String>,
) -> HashMap<String, bool> {
    let all = compute_defn_needs_state(program, plain);
    program
        .callables
        .iter()
        .filter(|c| c.exported)
        .map(|c| (c.name.clone(), *all.get(&c.name).unwrap_or(&true)))
        .collect()
}

/// Shared read context for the needs-state walkers (state/txn sets fixed per
/// fixpoint, locals+needs per callable).
struct Ctx<'a> {
    regular: &'a HashMap<String, &'a HirCallable>,
    txns: &'a HashSet<String>,
    state_fields: &'a HashSet<String>,
    always: &'a HashSet<String>,
    locals: &'a HashSet<String>,
    needs: &'a HashMap<String, bool>,
}

/// Per-run context (locals/needs vary per callable).
struct FixCtx<'a> {
    regular: &'a HashMap<String, &'a HirCallable>,
    txns: &'a HashSet<String>,
    tracked_fields: &'a HashSet<String>,
    always: &'a HashSet<String>,
}

fn fixpoint(
    regular: &HashMap<String, &HirCallable>,
    txns: &HashSet<String>,
    tracked_fields: &HashSet<String>,
    always: &HashSet<String>,
    with_locals: bool,
) -> HashMap<String, bool> {
    let ctx = FixCtx { regular, txns, tracked_fields, always };
    let mut needs: HashMap<String, bool> =
        regular.keys().map(|k| (k.clone(), false)).collect();
    while fixpoint_round(&ctx, with_locals, &mut needs) {}
    needs
}

/// One least-fixpoint round. Returns whether any callable became stateful.
fn fixpoint_round(ctx: &FixCtx<'_>, with_locals: bool, needs: &mut HashMap<String, bool>) -> bool {
    let mut changed = false;
    for (name, c) in ctx.regular {
        if *needs.get(name).unwrap_or(&false) {
            continue;
        }
        let locals = callable_locals(c, with_locals);
        let cctx = Ctx {
            regular: ctx.regular,
            txns: ctx.txns,
            state_fields: ctx.tracked_fields,
            always: ctx.always,
            locals: &locals,
            needs,
        };
        if body_needs_state(&c.body.statements, &cctx) {
            needs.insert(name.clone(), true);
            changed = true;
        }
    }
    changed
}

/// Parameter names plus body let-bindings (when the pass tracks locals).
fn callable_locals(c: &HirCallable, with_locals: bool) -> HashSet<String> {
    let mut locals = HashSet::new();
    if !with_locals {
        return locals;
    }
    for p in &c.params {
        locals.insert(p.name.clone());
    }
    collect_stmt_locals(&c.body.statements, &mut locals);
    locals
}

fn body_needs_state(stmts: &[HirStmt], ctx: &Ctx<'_>) -> bool {
    stmts.iter().any(|s| stmt_needs_state(s, ctx))
}

fn stmt_needs_state(stmt: &HirStmt, ctx: &Ctx<'_>) -> bool {
    let rec = |e: &HirExpr| expr_needs_state(e, ctx);
    let rec_stmts = |ss: &[HirStmt]| body_needs_state(ss, ctx);
    match &stmt.kind {
        HirStmtKind::Let { expr, .. } => expr.as_ref().is_some_and(|e| rec(e)),
        HirStmtKind::Assign(lhs, rhs) => rec(lhs) || rec(rhs),
        // `<-` on an Event lowers to a hardcoded state-carrying call.
        HirStmtKind::ArrowAssign { .. } => true,
        HirStmtKind::Term(opt) | HirStmtKind::EndProgram(opt) | HirStmtKind::Rollback(opt) => {
            opt.as_ref().is_some_and(|e| rec(e))
        }
        HirStmtKind::Tail(e) | HirStmtKind::Check(e) | HirStmtKind::Gate(e)
        | HirStmtKind::Expression(e) => rec(e),
        HirStmtKind::Guarded(cond, body) => rec(cond) || rec_stmts(body),
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => rec_stmts(body),
        HirStmtKind::Foreach { list, body, .. } => rec(list) || rec_stmts(body),
        HirStmtKind::Match { expr, arms } => {
            rec(expr) || arms.iter().any(|a| rec_stmts(&a.body))
        }
        HirStmtKind::TrgBinding { instance, .. } => rec(instance),
        HirStmtKind::Open(l, r) => rec(l) || rec(r),
        HirStmtKind::Break
        | HirStmtKind::Trap
        | HirStmtKind::Halt
        | HirStmtKind::Yield
        | HirStmtKind::FreeHint(_)
        | HirStmtKind::KeepHint(_)
        | HirStmtKind::MetadataAssignment(..)
        | HirStmtKind::InlineAsm { .. } => false,
        // Unsupported lowering: conservative (never mark a stateful defn
        // stateless).
        HirStmtKind::Other(_) => true,
    }
}

fn expr_needs_state(expr: &HirExpr, ctx: &Ctx<'_>) -> bool {
    match &expr.kind {
        HirExprKind::Literal(_) | HirExprKind::UnitLiteral { .. } => false,
        HirExprKind::Identifier(name) => {
            ctx.state_fields.contains(name) && !ctx.locals.contains(name)
        }
        HirExprKind::Other(_) | HirExprKind::Spawn { .. } | HirExprKind::Await(_) => true,
        HirExprKind::Call { callee, args, .. } => call_needs_state(callee, args, ctx),
        HirExprKind::MethodCall { receiver, name, args, .. } => {
            method_needs_state(receiver, name, args, ctx)
        }
        HirExprKind::Binary { left, right, .. } => {
            expr_needs_state(left, ctx) || expr_needs_state(right, ctx)
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Reflect { receiver: inner, .. }
        | HirExprKind::Lambda { body: inner, .. } => expr_needs_state(inner, ctx),
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            receiver.as_ref().map_or(false, |r| expr_needs_state(r, ctx))
                || args.iter().any(|a| expr_needs_state(a, ctx))
        }
        HirExprKind::Index(..) | HirExprKind::Slice { .. } | HirExprKind::Range { .. } => {
            subexprs_need_state(expr, ctx)
        }
        HirExprKind::StructLiteral { .. }
        | HirExprKind::Tuple(_)
        | HirExprKind::List(_) => elements_need_state(expr, ctx),
        HirExprKind::Block(stmts) => body_needs_state(stmts, ctx),
        HirExprKind::If { condition, then_branch, else_branch } => {
            expr_needs_state(condition, ctx)
                || expr_needs_state(then_branch, ctx)
                || else_branch.as_ref().is_some_and(|e| expr_needs_state(e, ctx))
        }
        HirExprKind::Match { scrutinee, arms } => {
            expr_needs_state(scrutinee, ctx)
                || arms.iter().any(|a| expr_needs_state(&a.body, ctx))
        }
    }
}

/// Call leaf: state intrinsic, txn, always-stateful, or a stateful callee.
fn call_needs_state(callee: &str, args: &[HirExpr], ctx: &Ctx<'_>) -> bool {
    let leaf = STATE_INTRINSICS.contains(&callee)
        || ctx.txns.contains(callee)
        || ctx.always.contains(callee)
        || ctx
            .regular
            .contains_key(callee)
            .then(|| *ctx.needs.get(callee).unwrap_or(&true))
            .unwrap_or(false);
    leaf || args.iter().any(|a| expr_needs_state(a, ctx))
}

/// Method call: any stateful same-named member on any owner type makes it
/// stateful (mirrors the AST's conservative cross-type reach).
fn method_needs_state(
    receiver: &HirExpr,
    name: &str,
    args: &[HirExpr],
    ctx: &Ctx<'_>,
) -> bool {
    expr_needs_state(receiver, ctx)
        || args.iter().any(|a| expr_needs_state(a, ctx))
        || method_stateful_on_any_type(name, ctx.regular, ctx.needs)
}

/// Index/Slice/Range operand recursion.
fn subexprs_need_state(expr: &HirExpr, ctx: &Ctx<'_>) -> bool {
    match &expr.kind {
        HirExprKind::Index(a, b) => {
            expr_needs_state(a, ctx) || expr_needs_state(b, ctx)
        }
        HirExprKind::Slice { array, start, end, stride } => {
            expr_needs_state(array, ctx)
                || start.as_ref().is_some_and(|e| expr_needs_state(e, ctx))
                || end.as_ref().is_some_and(|e| expr_needs_state(e, ctx))
                || stride.as_ref().is_some_and(|e| expr_needs_state(e, ctx))
        }
        HirExprKind::Range { start, end, .. } => {
            expr_needs_state(start, ctx) || expr_needs_state(end, ctx)
        }
        _ => false,
    }
}

/// Struct-literal / tuple / list element recursion.
fn elements_need_state(expr: &HirExpr, ctx: &Ctx<'_>) -> bool {
    match &expr.kind {
        HirExprKind::StructLiteral { fields, .. } => {
            fields.iter().any(|(_, e)| expr_needs_state(e, ctx))
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            items.iter().any(|e| expr_needs_state(e, ctx))
        }
        _ => false,
    }
}

fn method_stateful_on_any_type(
    method: &str,
    regular: &HashMap<String, &HirCallable>,
    needs: &HashMap<String, bool>,
) -> bool {
    let suffix = format!("::{method}");
    regular
        .keys()
        .any(|k| k.ends_with(&suffix) && *needs.get(k).unwrap_or(&false))
}

/// Collect names bound by `let` in a statement tree (mirrors the AST; foreach
/// items and lambda params are deliberately not locals there either).
fn collect_stmt_locals(stmts: &[HirStmt], out: &mut HashSet<String>) {
    for s in stmts {
        collect_stmt_local(s, out);
    }
}

fn collect_stmt_local(s: &HirStmt, out: &mut HashSet<String>) {
    match &s.kind {
        HirStmtKind::Let { name, expr, .. } => {
            out.insert(name.clone());
            if let Some(e) = expr {
                collect_expr_locals(e, out);
            }
        }
        HirStmtKind::Block(b)
        | HirStmtKind::SyncBlock(b)
        | HirStmtKind::Defer(b)
        | HirStmtKind::Mutex(b) => collect_stmt_locals(b, out),
        HirStmtKind::Guarded(c, b) => {
            collect_expr_locals(c, out);
            collect_stmt_locals(b, out);
        }
        HirStmtKind::Foreach { list, body, .. } => {
            collect_expr_locals(list, out);
            collect_stmt_locals(body, out);
        }
        HirStmtKind::Match { expr, arms } => {
            collect_expr_locals(expr, out);
            for a in arms {
                collect_stmt_locals(&a.body, out);
            }
        }
        HirStmtKind::Assign(l, r) => {
            collect_expr_locals(l, out);
            collect_expr_locals(r, out);
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                collect_expr_locals(t, out);
            }
            collect_expr_locals(value, out);
        }
        HirStmtKind::Term(Some(e))
        | HirStmtKind::EndProgram(Some(e))
        | HirStmtKind::Rollback(Some(e))
        | HirStmtKind::Tail(e)
        | HirStmtKind::Check(e)
        | HirStmtKind::Gate(e)
        | HirStmtKind::Expression(e) => collect_expr_locals(e, out),
        _ => {}
    }
}

fn collect_expr_locals(expr: &HirExpr, out: &mut HashSet<String>) {
    match &expr.kind {
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            if let Some(r) = receiver {
                collect_expr_locals(r, out);
            }
            for a in args {
                collect_expr_locals(a, out);
            }
        }
        HirExprKind::Block(stmts) => collect_stmt_locals(stmts, out),
        HirExprKind::Call { args, .. }
        | HirExprKind::Tuple(args)
        | HirExprKind::List(args) => {
            for a in args {
                collect_expr_locals(a, out);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_expr_locals(left, out);
            collect_expr_locals(right, out);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Reflect { receiver: inner, .. }
        | HirExprKind::Lambda { body: inner, .. } => collect_expr_locals(inner, out),
        HirExprKind::Index(a, b) => {
            collect_expr_locals(a, out);
            collect_expr_locals(b, out);
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            collect_expr_locals(receiver, out);
            for a in args {
                collect_expr_locals(a, out);
            }
        }
        HirExprKind::Slice { array, start, end, stride } => {
            collect_expr_locals(array, out);
            if let Some(s) = start {
                collect_expr_locals(s, out);
            }
            if let Some(e) = end {
                collect_expr_locals(e, out);
            }
            if let Some(s) = stride {
                collect_expr_locals(s, out);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            collect_expr_locals(start, out);
            collect_expr_locals(end, out);
        }
        HirExprKind::Spawn { args, .. } => {
            for a in args {
                collect_expr_locals(a, out);
            }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            collect_expr_locals(condition, out);
            collect_expr_locals(then_branch, out);
            if let Some(e) = else_branch {
                collect_expr_locals(e, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_expr_locals(scrutinee, out);
            for a in arms {
                collect_expr_locals(&a.body, out);
            }
        }
        HirExprKind::StructLiteral { fields, .. } => {
            for (_, e) in fields {
                collect_expr_locals(e, out);
            }
        }
        _ => {}
    }
}
