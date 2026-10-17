// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Storage-class classification for top-level `let`s (2026-10-17).
//!
//! A top-level `let` belongs to one of two storage classes:
//!
//! - **state** — machine state: lives in `%State`, referenced by the proof
//!   surface (contracts, watchdogs, `#!exit`, result expressions, view
//!   bindings), governed by the transition graph, liveness, and dead-field
//!   elimination.
//! - **plain** — an ordinary variable: identical name/type/init/read/write
//!   semantics, but the machine does not reason about it. Lowered to a
//!   module-scope `@global`, so defns that touch only plain lets do not
//!   carry `ptr %state` and the `%State` alloca never escapes through them.
//!
//! A bare `let` is classified by inference: **state iff the proof surface
//! references it, else plain.** `state let` / `plain let` are checked
//! assertions — `storage_assertion_errors` reports a declaration whose
//! category contradicts the inference, quoting the contradicting reference.
//! See docs/plans/2026-10-17-state-plain-storage-class.md.
//!
//! Why this exists: the buffered-stdout lane (01c92189, 2026-09-12) stored
//! its buffer in library state fields, putting `ptr %state` on the whole
//! print chain. Any loop containing a print made the `%State` alloca
//! escape; SROA refused promotion and every loop-carried field dropped to
//! memory round-trips (interval_step 848ms → 414ms/500M in the
//! counterfactual). Plain storage makes that demotion structurally
//! impossible: plumbing never becomes machine state in the first place.

use crate::ast::{Expr, Statement, TopLevel, Type};
use std::collections::HashSet;

/// Collect EVERY identifier an expression mentions (unfiltered — unlike
/// `collect_state_identifiers`, which keeps only known state fields).
/// Mirrors that function's variant coverage; a missed variant here would be
/// a liveness-hole-style bug for the classification (a proof surface
/// reference hidden in an unhandled form would wrongly demote a let).
fn collect_raw_idents(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Identifier(name) => {
            out.insert(name.clone());
        }
        Expr::BinaryOp(_, l, r) => {
            collect_raw_idents(l, out);
            collect_raw_idents(r, out);
        }
        Expr::UnaryOp(_, inner) => collect_raw_idents(inner, out),
        Expr::Index(obj, idx) => {
            collect_raw_idents(obj, out);
            collect_raw_idents(idx, out);
        }
        Expr::Field(obj, _) => collect_raw_idents(obj, out),
        Expr::AddrOf(inner) | Expr::Deref(inner) | Expr::Consume(inner) => {
            collect_raw_idents(inner, out)
        }
        Expr::Cast(inner, _) => collect_raw_idents(inner, out),
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_raw_idents(arg, out);
            }
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            collect_raw_idents(recv, out);
            for arg in args {
                collect_raw_idents(arg, out);
            }
        }
        Expr::Reflect(recv, _, _) => collect_raw_idents(recv, out),
        Expr::Block(stmts) => collect_stmts_surface(stmts, out),
        Expr::Tuple(exprs) | Expr::List(exprs) => {
            for e in exprs {
                collect_raw_idents(e, out);
            }
        }
        Expr::Match(scrutinee, arms) => {
            collect_raw_idents(scrutinee, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_raw_idents(guard, out);
                }
                collect_raw_idents(&arm.body, out);
            }
        }
        Expr::Within(inner, _) => collect_raw_idents(inner, out),
        Expr::If(cond, then, else_) => {
            collect_raw_idents(cond, out);
            collect_raw_idents(then, out);
            if let Some(else_) = else_ {
                collect_raw_idents(else_, out);
            }
        }
        Expr::Lambda(_, body) => collect_raw_idents(body, out),
        Expr::Range { start, end, inclusive: _ } => {
            collect_raw_idents(start, out);
            collect_raw_idents(end, out);
        }
        _ => {}
    }
}

/// Statement walk that collects ONLY proof-surface expressions. Bodies do
/// NOT count toward the surface (a node body reading a let does not make it
/// machine state — body access compiles to plain storage access) with ONE
/// exception: `endprogram <expr>` is a terminal observation — the program's
/// observable output — so its expression is surface. This mirrors
/// swan-song hoisting, which exists precisely because those guards are the
/// terminal observable side effect. Guard conditions and assignment RHS are
/// body control flow — never surface (a `when count % 5000000 == 0` print
/// guard must not pin `count` to machine state).
fn collect_stmts_surface(stmts: &[Statement], out: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Statement::EndProgram(Some(e)) => collect_raw_idents(e, out),
            // 2026-10-17: a term WITH A VALUE is a terminal observation too
            // (`term Print#(count);` — the value is the program's output).
            // `term;` (None) is bare termination — no observation, no surface.
            Statement::Term(Some(e)) => collect_raw_idents(e, out),
            Statement::Guarded(_, body) => collect_stmts_surface(body, out),
            Statement::Block(body) | Statement::SyncBlock(body) | Statement::Mutex(body) => {
                collect_stmts_surface(body, out)
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    collect_stmts_surface(&arm.body, out);
                }
            }
            Statement::Foreach { body, .. } => collect_stmts_surface(body, out),
            _ => {}
        }
    }
}

/// The set of top-level let names the PROOF SURFACE references: contract
/// pre/post conditions, watchdog conditions + fallbacks + on-fire
/// arguments, `#!exit` conditions, and `endprogram` observations. Plus any
/// caller-supplied extra surface (view bindings — the web shim consumes
/// them; the classifier has no view context).
fn proof_surface(
    items: &[TopLevel],
    exit_condition: Option<&Expr>,
    extra: &HashSet<String>,
) -> HashSet<String> {
    let mut surface: HashSet<String> = extra.clone();
    if let Some(e) = exit_condition {
        collect_raw_idents(e, &mut surface);
    }
    for item in items {
        if let TopLevel::Transaction(t) = item {
            collect_raw_idents(&t.contract.pre_condition, &mut surface);
            collect_raw_idents(&t.contract.post_condition, &mut surface);
            if let Some(wd) = &t.contract.watchdog {
                collect_raw_idents(&wd.condition, &mut surface);
                if let Some(fb) = &wd.fallback {
                    collect_raw_idents(fb, &mut surface);
                }
                if let Some(arg) = &wd.on_fire.as_ref().and_then(|f| f.arg.as_deref()) {
                    surface.insert(arg.to_string());
                }
            }
            collect_stmts_surface(&t.body, &mut surface);
        }
    }
    surface
}

/// The declared storage assertion on a top-level let, if any: `"state"` or
/// `"plain"`. `None` = bare (inferred).
pub fn declared_storage(items: &[TopLevel], target: &str) -> Option<&'static str> {
    let stmt = items.iter().find_map(|item| match item {
        TopLevel::Statement(s) => match s.as_ref() {
            Statement::Let { name, .. } if name == target => Some(s.as_ref()),
            _ => None,
        },
        _ => None,
    })?;
    let Statement::Let { modifiers, .. } = stmt else {
        return None;
    };
    let tag = modifiers.iter().find_map(|m| match m.name.as_str() {
        "state" => Some("state"),
        "plain" => Some("plain"),
        _ => None,
    });
    tag
}

fn let_annotation(name: &str, items: &[TopLevel]) -> Option<String> {
    declared_storage(items, name).map(str::to_string)
}

/// Names WRITTEN by any reactive node/txn body (`x = ...`, arrow stores).
/// A body write is a state transition — such a let is always machine state
/// (the loop engines enumerate state fields by index; a demoted body-written
/// field would lose its per-field store/read plumbing). Defn-body writes do
/// NOT count: plumbing like `stdout_fill` is written only by defns, and
/// that is precisely the demotion the fix exists for.
fn body_written_names(items: &[TopLevel]) -> HashSet<String> {
    fn walk(stmts: &[Statement], out: &mut HashSet<String>) {
        for stmt in stmts {
            match stmt {
                Statement::Assign(Expr::Identifier(name), _) => {
                    out.insert(name.clone());
                }
                Statement::ArrowAssign { target: Some(t), .. } => {
                    if let Expr::Identifier(name) = t.as_ref() {
                        out.insert(name.clone());
                    }
                }
                Statement::Guarded(_, body)
                | Statement::Block(body)
                | Statement::SyncBlock(body)
                | Statement::Mutex(body)
                | Statement::Foreach { body, .. } => walk(body, out),
                Statement::Match { arms, .. } => {
                    for arm in arms {
                        walk(&arm.body, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut written = HashSet::new();
    for item in items {
        if let TopLevel::Transaction(t) = item {
            walk(&t.body, &mut written);
        }
    }
    written
}

/// A plain let's TYPE must be scalar — the primitives (Int/Float/Float64/
/// Bool, or untyped = Int). Obj/coll/compound types unpack into machine
/// instance columns or build heap values through op machinery; such a let
/// stays state on inference (`let q: Q = [1,2,3]` constructs through the
/// collection's ops; `let p: Pool = 0` unpacks into columns).
fn scalar_type(ty: &Option<Type>) -> bool {
    match ty {
        None => true,
        Some(Type::Custom(n)) => {
            matches!(n.as_str(), "Int" | "Float" | "Float64" | "Bool")
        }
        _ => false,
    }
}

/// A plain let's initializer must be SCALAR — `None` or a literal/copy
/// expression seedable in the pre-reactor phase by a single `store`.
/// Constructions (list/tuple literals, struct literals, calls) build
/// heap/compound values through op machinery that belongs to machine state.
fn scalar_init(expr: &Option<Expr>) -> bool {
    match expr {
        None => true,
        Some(e) => !matches!(
            e,
            Expr::List(_)
                | Expr::Tuple(_)
                | Expr::StructLiteral { .. }
                | Expr::Call(..)
                | Expr::MethodCall(..)
        ),
    }
}

/// The per-let decision: does THIS let lower to plain storage? Declared
/// `state` is never plain here (a contradiction is the caller's error to
/// report); declared `plain` always is; bare infers.
fn let_lowers_plain(
    declared: Option<&str>,
    surface_has: bool,
    body_writes: bool,
    ty: &Option<Type>,
    expr: &Option<Expr>,
) -> bool {
    match declared {
        Some("plain") => true,
        Some(_) => false,
        None => !surface_has && !body_writes && scalar_type(ty) && scalar_init(expr),
    }
}

/// The set of top-level lets classified PLAIN: declared `plain`, or bare
/// and unreferenced by the proof surface AND unwritten by any node body
/// AND scalar-initialized. Declared `state` lets are never plain here (a
/// contradiction is the caller's error to report).
pub fn classify_plain_lets(
    items: &[TopLevel],
    exit_condition: Option<&Expr>,
    extra: &HashSet<String>,
) -> HashSet<String> {
    let surface = proof_surface(items, exit_condition, extra);
    let written = body_written_names(items);
    let mut plain = HashSet::new();
    for item in items {
        if let TopLevel::Statement(stmt) = item {
            if let Statement::Let { name, ty, expr, .. } = stmt.as_ref() {
                let declared = let_annotation(name, items);
                if let_lowers_plain(
                    declared.as_deref(),
                    surface.contains(name),
                    written.contains(name),
                    ty,
                    expr,
                ) {
                    plain.insert(name.clone());
                }
            }
        }
    }
    plain
}

/// Checked-assertion violations: a declaration whose category contradicts
/// the inference. Error grade — the declaration states a fact about the
/// program that is false. Each message quotes the contradicting reference.
pub fn storage_assertion_errors(
    items: &[TopLevel],
    exit_condition: Option<&Expr>,
    extra: &HashSet<String>,
) -> Vec<String> {
    let surface = proof_surface(items, exit_condition, extra);
    let mut errors = Vec::new();
    for item in items {
        if let TopLevel::Statement(stmt) = item {
            if let Statement::Let { name, expr, .. } = stmt.as_ref() {
                let tag = let_annotation(name, items);
                let referenced = surface.contains(name);
                match tag.as_deref() {
                    Some("plain") if referenced => errors.push(format!(
                        "error: '{}' is declared `plain`, but the proof surface references it — \
a contract, watchdog, exit condition, result expression, or view binding makes a \
variable machine state. Remove the reference or declare it `state`.",
                        name
                    )),
                    Some("plain") if !scalar_init(expr) => errors.push(format!(
                        "error: '{}' is declared `plain`, but its initializer constructs a \
compound value — plain storage is for scalar plumbing seeded by a plain value. \
Remove the `plain` declaration or initialize it with a scalar.",
                        name
                    )),
                    Some("state") if !referenced => errors.push(format!(
                        "error: '{}' is declared `state`, but no contract, watchdog, exit \
condition, or result expression references it — the machine never reasons about it. \
Contract over it or declare it `plain`.",
                        name
                    )),
                    _ => {}
                }
            }
        }
    }
    errors
}
