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
            | HirCallableKind::MemberTransaction => {}
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

fn fixpoint(
    regular: &HashMap<String, &HirCallable>,
    txns: &HashSet<String>,
    tracked_fields: &HashSet<String>,
    always: &HashSet<String>,
    with_locals: bool,
) -> HashMap<String, bool> {
    let mut needs: HashMap<String, bool> =
        regular.keys().map(|k| (k.clone(), false)).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for (name, c) in regular {
            if *needs.get(name).unwrap_or(&false) {
                continue;
            }
            let mut locals = HashSet::new();
            if with_locals {
                for p in &c.params {
                    locals.insert(p.name.clone());
                }
                collect_stmt_locals(&c.body.statements, &mut locals);
            }
            if body_needs_state(&c.body.statements, regular, txns, tracked_fields, always, &locals, &needs) {
                needs.insert(name.clone(), true);
                changed = true;
            }
        }
    }
    needs
}

fn body_needs_state(
    stmts: &[HirStmt],
    regular: &HashMap<String, &HirCallable>,
    txns: &HashSet<String>,
    state_fields: &HashSet<String>,
    always: &HashSet<String>,
    locals: &HashSet<String>,
    needs: &HashMap<String, bool>,
) -> bool {
    stmts.iter().any(|s| {
        stmt_needs_state(s, regular, txns, state_fields, always, locals, needs)
    })
}

fn stmt_needs_state(
    stmt: &HirStmt,
    regular: &HashMap<String, &HirCallable>,
    txns: &HashSet<String>,
    state_fields: &HashSet<String>,
    always: &HashSet<String>,
    locals: &HashSet<String>,
    needs: &HashMap<String, bool>,
) -> bool {
    let rec = |e: &HirExpr| {
        expr_needs_state(e, regular, txns, state_fields, always, locals, needs)
    };
    let rec_stmts = |ss: &[HirStmt]| {
        body_needs_state(ss, regular, txns, state_fields, always, locals, needs)
    };
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

fn expr_needs_state(
    expr: &HirExpr,
    regular: &HashMap<String, &HirCallable>,
    txns: &HashSet<String>,
    state_fields: &HashSet<String>,
    always: &HashSet<String>,
    locals: &HashSet<String>,
    needs: &HashMap<String, bool>,
) -> bool {
    let rec = |e: &HirExpr| {
        expr_needs_state(e, regular, txns, state_fields, always, locals, needs)
    };
    let rec_stmts = |ss: &[HirStmt]| {
        body_needs_state(ss, regular, txns, state_fields, always, locals, needs)
    };
    match &expr.kind {
        HirExprKind::Literal(_) => false,
        HirExprKind::Identifier(name) => {
            state_fields.contains(name) && !locals.contains(name)
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            receiver.as_ref().map_or(false, |r| rec(r)) || args.iter().any(|a| rec(a))
        }
        HirExprKind::Call { callee, args, .. } => {
            let leaf = STATE_INTRINSICS.contains(&callee.as_str())
                || txns.contains(callee)
                || always.contains(callee)
                || regular
                    .contains_key(callee)
                    .then(|| *needs.get(callee).unwrap_or(&true))
                    .unwrap_or(false);
            leaf || args.iter().any(|a| rec(a))
        }
        HirExprKind::MethodCall { receiver, name, args, .. } => {
            rec(receiver)
                || args.iter().any(|a| rec(a))
                || method_stateful_on_any_type(name, regular, needs)
        }
        HirExprKind::Binary { left, right, .. } => rec(left) || rec(right),
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Reflect { receiver: inner, .. }
        | HirExprKind::Lambda { body: inner, .. } => rec(inner),
        HirExprKind::Index(a, b) => rec(a) || rec(b),
        HirExprKind::Slice { array, start, end, stride } => {
            rec(array)
                || start.as_ref().is_some_and(|e| rec(e))
                || end.as_ref().is_some_and(|e| rec(e))
                || stride.as_ref().is_some_and(|e| rec(e))
        }
        HirExprKind::Range { start, end, .. } => rec(start) || rec(end),
        // spawn/await lower with `%state` directly.
        HirExprKind::Spawn { .. } | HirExprKind::Await(_) => true,
        HirExprKind::Block(stmts) => rec_stmts(stmts),
        HirExprKind::If { condition, then_branch, else_branch } => {
            rec(condition)
                || rec(then_branch)
                || else_branch.as_ref().is_some_and(|e| rec(e))
        }
        HirExprKind::Match { scrutinee, arms } => {
            rec(scrutinee) || arms.iter().any(|a| rec(&a.body))
        }
        HirExprKind::StructLiteral { fields, .. } => fields.iter().any(|(_, e)| rec(e)),
        HirExprKind::Tuple(items) | HirExprKind::List(items) => items.iter().any(|e| rec(e)),
        HirExprKind::UnitLiteral { .. } => false,
        HirExprKind::Other(_) => true,
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
