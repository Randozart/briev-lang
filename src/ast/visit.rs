// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! The one exhaustive AST visitor (Rule 17 — centralized helper).
//!
//! 2026-10-05 (three-surfaces plan Phase 0.3): the intrinsic supported-set
//! gate in `backend::normalizer::validate_intrinsics` harvested only
//! statement-ROOT expressions and had no `Statement::Let` arm at all — a
//! `let x = Deref#()` or a nested `a[i] + GetGlobalId#(0)` call never
//! reached the gate, so unsupported programs sailed through and panicked
//! (or mis-lowered) in the emitter. Before this module the codebase carried
//! nine partial hand-rolled walkers (analysis/*, backend/normalizer.rs),
//! each covering a different subset — the exact drift that let the gate and
//! the emitters disagree.
//!
//! Contract: `walk_expr`/`walk_stmts` invoke `f` on EVERY `Expr` node in
//! the tree — statement roots, `let` initializers, call arguments, match
//! patterns, contract halves, derivation examples — in deterministic
//! source order. The matches are exhaustive (no `_` arm): adding a variant
//! to `Expr`, `Statement`, `Pattern`, `MatchArm`, or `StmtMatchArm` fails
//! to compile until it is visited, so coverage cannot rot silently.

use super::expr::{ChainSegment, DerivationBlock, DerivationExample, MatchArm, Pattern};
use super::top::{Contract, Definition, Statement, StmtMatchArm};
use super::Expr;

/// Visit every `Expr` node in `e`, including `e` itself.
pub fn walk_expr<F: FnMut(&Expr)>(e: &Expr, f: &mut F) {
    f(e);
    match e {
        // ── Leaves ──────────────────────────────────────────────────
        Expr::Quoted(_)
        | Expr::Decimal(_)
        | Expr::Char(_)
        | Expr::TaggedLiteral(_, _)
        | Expr::TaggedQuotedLiteral(_, _)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::BeginProgram
        | Expr::Identifier(_)
        | Expr::Wildcard
        | Expr::Exists(_)
        | Expr::FormattingAnnotation(_)
        | Expr::UnitLiteral { .. } => {}

        // ── Call-shaped ─────────────────────────────────────────────
        Expr::Call(_, args, _) => walk_all(args, f),
        Expr::MethodCall(recv, _, args, _, _) => {
            walk_expr(recv, f);
            walk_all(args, f);
        }
        Expr::Spawn { args, .. } => walk_all(args, f),
        Expr::PluginIntercept { args, receiver, .. } => {
            walk_all(args, f);
            if let Some(r) = receiver {
                walk_expr(r, f);
            }
        }

        // ── Operators / structure ───────────────────────────────────
        Expr::BinaryOp(_, a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        Expr::UnaryOp(_, inner) => walk_expr(inner, f),
        Expr::Field(obj, _) => walk_expr(obj, f),
        Expr::Reflect(inner, _, _) => walk_expr(inner, f),
        Expr::Index(a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        Expr::Slice { array, start, end, stride } => {
            walk_expr(array, f);
            for part in [start, end, stride].into_iter().flatten() {
                walk_expr(part, f);
            }
        }
        Expr::Range { start, end, .. } => {
            walk_expr(start, f);
            walk_expr(end, f);
        }
        Expr::If(cond, then_e, else_e) => {
            walk_expr(cond, f);
            walk_expr(then_e, f);
            if let Some(e) = else_e {
                walk_expr(e, f);
            }
        }
        Expr::Match(scrutinee, arms) => {
            walk_expr(scrutinee, f);
            for arm in arms {
                walk_pattern(&arm.pattern, f);
                if let Some(g) = &arm.guard {
                    walk_expr(g, f);
                }
                walk_expr(&arm.body, f);
            }
        }
        Expr::Tuple(elems) | Expr::List(elems) => walk_all(elems, f),
        Expr::StructLiteral { fields, specs, .. } => {
            for (_, val) in fields.iter().chain(specs.iter()) {
                walk_expr(val, f);
            }
        }
        Expr::Lambda(_, body) => walk_expr(body, f),
        Expr::Cast(inner, _) | Expr::IsType(inner, _) => walk_expr(inner, f),
        Expr::Within(a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        Expr::Deref(inner)
        | Expr::AddrOf(inner)
        | Expr::Consume(inner)
        | Expr::Await(inner) => walk_expr(inner, f),
        Expr::Capture { expr, .. } => walk_expr(expr, f),
        Expr::Block(stmts) => walk_stmts(stmts, f),
        Expr::DerivationBlock(block) => walk_derivation(block, f),
    }
}

/// Visit every `Expr` reachable from a statement list: initializer
/// expressions, guards, nested bodies, match arms, contract halves reached
/// through `Definition`/`InlineDefn`, and so on.
pub fn walk_stmts<F: FnMut(&Expr)>(stmts: &[Statement], f: &mut F) {
    for stmt in stmts {
        match stmt {
            Statement::Let { expr, .. } => {
                if let Some(e) = expr {
                    walk_expr(e, f);
                }
            }
            Statement::Assign(lhs, rhs) => {
                walk_expr(lhs, f);
                walk_expr(rhs, f);
            }
            Statement::ArrowAssign { target, value, consume: _ } => {
                if let Some(t) = target {
                    walk_expr(t, f);
                }
                walk_expr(value, f);
            }
            Statement::Term(e) | Statement::EndProgram(e) | Statement::Rollback(e) => {
                if let Some(e) = e {
                    walk_expr(e, f);
                }
            }
            Statement::Check(e) | Statement::Gate(e) => walk_expr(e, f),
            Statement::Guarded(cond, body) => {
                walk_expr(cond, f);
                walk_stmts(body, f);
            }
            Statement::Expression(e) => walk_expr(e, f),
            Statement::Block(body)
            | Statement::SyncBlock(body)
            | Statement::Defer(body)
            | Statement::Mutex(body) => walk_stmts(body, f),
            Statement::Foreach { list, body, .. } => {
                walk_expr(list, f);
                walk_stmts(body, f);
            }
            Statement::TrgBinding { instance, .. } => walk_expr(instance, f),
            Statement::Match { expr, arms } => {
                walk_expr(expr, f);
                walk_stmt_arms(arms, f);
            }
            Statement::Open(a, b) => {
                walk_expr(a, f);
                walk_expr(b, f);
            }
            Statement::InlineDefn(d) => walk_definition(d, f),
            // No Expr payload.
            Statement::Break
            | Statement::Trap
            | Statement::Halt
            | Statement::Yield
            | Statement::MetadataAssignment(_, _)
            | Statement::FreeHint(_)
            | Statement::KeepHint(_)
            | Statement::InlineAsm { .. } => {}
        }
    }
}

/// Visit every `Expr` inside a pattern (literal and range patterns carry
/// expressions; bindings do not).
pub fn walk_pattern<F: FnMut(&Expr)>(p: &Pattern, f: &mut F) {
    match p {
        Pattern::Wildcard | Pattern::Binding(_) | Pattern::TypedBinding(_, _) => {}
        Pattern::Literal(e) => walk_expr(e, f),
        Pattern::EnumVariant(_, subs) => {
            for s in subs {
                walk_pattern(s, f);
            }
        }
        Pattern::Tuple(subs) | Pattern::Multi(subs) => {
            for s in subs {
                walk_pattern(s, f);
            }
        }
        Pattern::Range(a, b) | Pattern::RangeInclusive(a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
    }
}

/// Visit a contract's precondition, postcondition, and watchdog condition.
pub fn walk_contract<F: FnMut(&Expr)>(c: &Contract, f: &mut F) {
    walk_expr(&c.pre_condition, f);
    walk_expr(&c.post_condition, f);
    if let Some(spec) = &c.watchdog {
        walk_expr(&spec.condition, f);
    }
}

/// Visit a definition's contract halves, derivation block, and body.
pub fn walk_definition<F: FnMut(&Expr)>(d: &Definition, f: &mut F) {
    walk_contract(&d.contract, f);
    if let Some(block) = &d.derivation {
        walk_derivation(block, f);
    }
    walk_stmts(&d.body, f);
}

fn walk_derivation<F: FnMut(&Expr)>(block: &DerivationBlock, f: &mut F) {
    for DerivationExample { inputs, output, .. } in &block.examples {
        walk_all(inputs, f);
        walk_expr(output, f);
    }
    if let Some(e) = &block.synthesized {
        walk_expr(e, f);
    }
    if let Some(e) = &block.postcondition {
        walk_expr(e, f);
    }
    if let Some(e) = &block.precondition {
        walk_expr(e, f);
    }
    for seg in &block.chain {
        match seg {
            ChainSegment::Ref(_) => {}
            ChainSegment::Derivation(inner) => walk_derivation(inner, f),
        }
    }
}

/// Statement-match arms: patterns first, then the arm body. Split out of
/// `walk_stmts` so the walker's loop nesting stays at depth 1 (Praetor).
fn walk_stmt_arms<F: FnMut(&Expr)>(arms: &[StmtMatchArm], f: &mut F) {
    for arm in arms {
        arm.patterns.iter().for_each(|p| walk_pattern(p, f));
        walk_stmts(&arm.body, f);
    }
}

fn walk_all<F: FnMut(&Expr)>(exprs: &[Expr], f: &mut F) {
    for e in exprs {
        walk_expr(e, f);
    }
}
