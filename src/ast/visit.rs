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
use super::top::{Contract, Definition, Statement, StmtMatchArm, TopLevel, Transaction};
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
            Statement::Tail(e) => walk_expr(e, f),
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

/// Visit every assignment as a (target, value) EXPRESSION pair, descending
/// every statement form. 2026-10-18 (fixture campaign): the goal-reachability
/// scans in the typechecker were top-level-statement-only, so a nested
/// `when passes == PASSES { phase = 2; }` completion store was invisible to
/// the beginprogram gate while the (fixed) causality harvester now sees it —
/// the two gates jointly refused compilable programs. One walker, both
/// consumers (Rule 17).
pub fn walk_assign_pairs<F: FnMut(&Expr, &Expr)>(stmts: &[Statement], f: &mut F) {
    for stmt in stmts {
        match stmt {
            Statement::Assign(lhs, rhs) => f(lhs, rhs),
            Statement::ArrowAssign {
                target: Some(t),
                value,
                ..
            } => f(t, value),
            Statement::ArrowAssign { target: None, .. } => {}
            Statement::Guarded(_, body)
            | Statement::Block(body)
            | Statement::SyncBlock(body)
            | Statement::Defer(body)
            | Statement::Mutex(body) => walk_assign_pairs(body, f),
            Statement::Foreach { body, .. } => walk_assign_pairs(body, f),
            Statement::Match { arms, .. } => {
                for arm in arms {
                    walk_assign_pairs(&arm.body, f);
                }
            }
            Statement::InlineDefn(d) => walk_assign_pairs(&d.body, f),
            // No assignment pairs.
            Statement::Let { .. }
            | Statement::Term(_)
            | Statement::Tail(_)
            | Statement::EndProgram(_)
            | Statement::Rollback(_)
            | Statement::Check(_)
            | Statement::Gate(_)
            | Statement::Expression(_)
            | Statement::TrgBinding { .. }
            | Statement::Open(_, _)
            | Statement::Break
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

/// Visit every assignment TARGET expression in a statement list: `Assign`
/// LHS and `ArrowAssign` targets. 2026-10-18 (fixture campaign, BUGS.md):
/// the write harvesters (causality, concurrency gate) previously saw only
/// top-level bare-identifier assigns — a phase transition inside a `when`
/// block was invisible to the causal graph, and `arr[i] = v` was not a
/// write at all, so shared-buffer writers looked disjoint to the Rule 22
/// classifier. Targets are yielded as EXPRESSIONS (`Identifier`,
/// `Index`, `Field`, `Deref` — whatever the grammar produces on the LHS);
/// callers decide what counts as a written name.
pub fn walk_assign_targets<F: FnMut(&Expr)>(stmts: &[Statement], f: &mut F) {
    walk_assign_pairs(stmts, &mut |lhs, _| f(lhs));
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

/// Visit a transaction's contract halves, derivation block, and body.
/// Mirrors `walk_definition` (a txn is a defn with a mandatory contract).
pub fn walk_transaction<F: FnMut(&Expr)>(t: &Transaction, f: &mut F) {
    walk_contract(&t.contract, f);
    if let Some(block) = &t.derivation {
        walk_derivation(block, f);
    }
    walk_stmts(&t.body, f);
}

/// Visit every `Expr` reachable from a top-level item. 2026-10-18 (fixture
/// campaign): the import splice's three hand-rolled closure walkers each
/// missed `Statement::Match` arms (a call inside a match arm was dropped
/// from the transitive import closure and fell back to the raw-Int
/// fallback at re-typecheck) and never descended into obj/type member
/// bodies (`GrowBuf.put_u8` → same-module `grow_to_fit` — any importer of
/// GrowBuf failed). This is the one walker all three now build on; the
/// exhaustive match keeps coverage from rotting (Rule 17).
pub fn walk_toplevel<F: FnMut(&Expr)>(item: &TopLevel, f: &mut F) {
    match item {
        TopLevel::Definition(d) | TopLevel::CompileTimeDefn(d) => walk_definition(d, f),
        TopLevel::TypeDefOperator(d) => walk_definition(d, f),
        TopLevel::Transaction(t) | TopLevel::CompileTimeTxn(t) => walk_transaction(t, f),
        TopLevel::TypeDef(td) => walk_type_body(td, f),
        TopLevel::Obj(sd) => {
            for t in &sd.transactions {
                walk_transaction(t, f);
            }
        }
        TopLevel::Cell(cd) => {
            for t in &cd.transactions {
                walk_transaction(t, f);
            }
            for d in &cd.definitions {
                walk_definition(d, f);
            }
            for trig in &cd.internal_triggers {
                walk_expr(&trig.instance, f);
            }
        }
        TopLevel::Budget(b) => walk_expr(&b.contract, f),
        TopLevel::Unpop(_) | TopLevel::ShortCircuit(_) => {}
        TopLevel::WhenLaw(w) => {
            walk_expr(&w.guard, f);
            walk_stmts(&w.facts, f);
        }
        TopLevel::Constant(c) => walk_expr(&c.expr, f),
        TopLevel::Assertion { pre, .. } => walk_expr(pre, f),
        TopLevel::Statement(s) => walk_stmts(std::slice::from_ref(s), f),
        TopLevel::StageBlock(b) => walk_stmts(&b.body, f),
        TopLevel::Fuzzed { item, .. } => walk_toplevel(item, f),
        TopLevel::Trigger(t) => walk_expr(&t.instance, f),
        TopLevel::TriggerBinding { instance, .. } => walk_expr(instance, f),
        TopLevel::IsrHandler(h) => walk_expr(&h.vector, f),
        TopLevel::ProtocolDef(p) => {
            if let Some(c) = &p.contract {
                walk_contract(c, f);
            }
        }
        TopLevel::Trait(tr) => {
            for d in &tr.functions {
                walk_definition(d, f);
            }
            for b in &tr.op_bindings {
                walk_expr(&b.expr, f);
            }
        }
        TopLevel::Impl(im) => {
            for d in &im.functions {
                walk_definition(d, f);
            }
            for b in &im.op_bindings {
                walk_expr(&b.expr, f);
            }
        }
        TopLevel::SyncGroup { item, .. } => walk_toplevel(item, f),
        TopLevel::Cfg(guard) => {
            for it in &guard.items {
                walk_toplevel(it, f);
            }
        }
        TopLevel::CompileTimeLet(_, e) => walk_expr(e, f),
        TopLevel::CompileTimeConst(_, e) => walk_expr(e, f),
        TopLevel::Init(init) => {
            if let Some(v) = &init.value {
                walk_expr(v, f);
            }
            walk_stmts(&init.body, f);
        }
        // No Expr payloads relevant to the reference closure: imports and
        // exports are names; frgn/asm/bad bodies are non-Briev text or
        // type-only declarations; enums carry variant TYPES only; render/
        // fab/svg blocks are strings and placements; data tables and module
        // config are compile-time data.
        TopLevel::Import(_)
        | TopLevel::Export(_)
        | TopLevel::ForeignBinding(_)
        | TopLevel::Enum(_)
        | TopLevel::StateDecl(_)
        | TopLevel::Signature(_)
        | TopLevel::LinkDependency(_)
        | TopLevel::ResourceDecl(_)
        | TopLevel::StaticStruct(_)
        | TopLevel::Codec(_)
        | TopLevel::AsmFn(_)
        | TopLevel::BadFn(_)
        | TopLevel::RenderBlock(_)
        | TopLevel::FabBlock(_)
        | TopLevel::Stylesheet(_)
        | TopLevel::SvgComponent { .. }
        | TopLevel::ModuleMetadata(_)
        | TopLevel::Data(_)
        | TopLevel::ModuleConfig(_) => {}
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

/// A type/obj body's behavioral payload: member defs (whose bodies reference
/// same-module fns — `obj GrowBuf { put_u8 → grow_to_fit }`), op-binding
/// exprs, constraints, and when-laws. Slots/bindings/operators/ports carry
/// Types, not Exprs. Split out of `walk_toplevel` (Praetor complexity).
fn walk_type_body<F: FnMut(&Expr)>(td: &super::top::TypeDef, f: &mut F) {
    // 2026-10-18 (splice closure): before this descent the closure never
    // walked member bodies and every cross-module importer of such a type
    // failed ("expected GrowBuf, found Int").
    for m in &td.body.members {
        walk_toplevel(m, f);
    }
    for b in &td.body.op_bindings {
        walk_expr(&b.expr, f);
    }
    for c in &td.body.constraints {
        walk_expr(c, f);
    }
    for law in &td.body.when_laws {
        walk_expr(&law.guard, f);
        walk_stmts(&law.facts, f);
    }
}

fn walk_all<F: FnMut(&Expr)>(exprs: &[Expr], f: &mut F) {
    for e in exprs {
        walk_expr(e, f);
    }
}
