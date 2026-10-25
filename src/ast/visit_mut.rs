//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// Authored by Otto Westerlund (@odecode); adopted with attribution, see
// docs/contributors/otto-westerlund.md.
//! Exhaustive mutable postorder AST traversal.
//! 2026-10-22: print expansion missed foreach and operand/container positions.
//! One rewrite walker covers those payloads without partial per-plugin trees.
//! Children are visited before their parent, exactly once in source order.
//! Replacements are not revisited; callbacks must produce already-lowered nodes.
//! Exhaustive matches keep new AST variants from silently escaping expansion.

use super::expr::{ChainSegment, DerivationBlock, DerivationExample, Pattern};
use super::top::{Contract, Definition, Statement, StmtMatchArm, TopLevel, Transaction};
use super::Expr;

/// Visit every `Expr` node in `e`, including `e` itself.
pub fn walk_expr<F: FnMut(&mut Expr)>(e: &mut Expr, f: &mut F) {
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
                walk_pattern(&mut arm.pattern, f);
                if let Some(g) = &mut arm.guard {
                    walk_expr(g, f);
                }
                walk_expr(&mut arm.body, f);
            }
        }
        Expr::Tuple(elems) | Expr::List(elems) => walk_all(elems, f),
        Expr::StructLiteral { fields, specs, .. } => {
            for (_, val) in fields.iter_mut().chain(specs.iter_mut()) {
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
    f(e);
}

/// Visit every `Expr` reachable from a statement list: initializer
/// expressions, guards, nested bodies, match arms, contract halves reached
/// through `Definition`/`InlineDefn`, and so on.
pub fn walk_stmts<F: FnMut(&mut Expr)>(stmts: &mut [Statement], f: &mut F) {
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

/// Visit every `Expr` inside a pattern (literal and range patterns carry
/// expressions; bindings do not).
pub fn walk_pattern<F: FnMut(&mut Expr)>(p: &mut Pattern, f: &mut F) {
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
pub fn walk_contract<F: FnMut(&mut Expr)>(c: &mut Contract, f: &mut F) {
    walk_expr(&mut c.pre_condition, f);
    walk_expr(&mut c.post_condition, f);
    if let Some(spec) = &mut c.watchdog {
        walk_expr(&mut spec.condition, f);
    }
}

/// Visit a transaction's contract halves, derivation block, and body.
/// Mirrors `walk_definition` (a txn is a defn with a mandatory contract).
pub fn walk_transaction<F: FnMut(&mut Expr)>(t: &mut Transaction, f: &mut F) {
    walk_contract(&mut t.contract, f);
    if let Some(block) = &mut t.derivation {
        walk_derivation(block, f);
    }
    walk_stmts(&mut t.body, f);
}

/// Visit every `Expr` reachable from a top-level item. 2026-10-18 (fixture
/// campaign): the import splice's three hand-rolled closure walkers each
/// missed `Statement::Match` arms (a call inside a match arm was dropped
/// from the transitive import closure and fell back to the raw-Int
/// fallback at re-typecheck) and never descended into obj/type member
/// bodies (`GrowBuf.put_u8` → same-module `grow_to_fit` — any importer of
/// GrowBuf failed). This is the one walker all three now build on; the
/// exhaustive match keeps coverage from rotting (Rule 17).
pub fn walk_toplevel<F: FnMut(&mut Expr)>(item: &mut TopLevel, f: &mut F) {
    match item {
        TopLevel::Definition(d) | TopLevel::CompileTimeDefn(d) => walk_definition(d, f),
        TopLevel::TypeDefOperator(d) => walk_definition(d, f),
        TopLevel::Transaction(t) | TopLevel::CompileTimeTxn(t) => walk_transaction(t, f),
        TopLevel::TypeDef(td) => walk_type_body(td, f),
        TopLevel::Obj(sd) => {
            for t in &mut sd.transactions {
                walk_transaction(t, f);
            }
        }
        TopLevel::Cell(cd) => {
            for t in &mut cd.transactions {
                walk_transaction(t, f);
            }
            for d in &mut cd.definitions {
                walk_definition(d, f);
            }
            for trig in &mut cd.internal_triggers {
                walk_expr(&mut trig.instance, f);
            }
        }
        TopLevel::Budget(b) => walk_expr(&mut b.contract, f),
        TopLevel::Unpop(_) | TopLevel::ShortCircuit(_) => {}
        TopLevel::WhenLaw(w) => {
            walk_expr(&mut w.guard, f);
            walk_stmts(&mut w.facts, f);
        }
        TopLevel::Constant(c) => walk_expr(&mut c.expr, f),
        TopLevel::Assertion { pre, .. } => walk_expr(pre, f),
        TopLevel::Statement(s) => walk_stmts(std::slice::from_mut(s), f),
        TopLevel::StageBlock(b) => walk_stmts(&mut b.body, f),
        TopLevel::Fuzzed { item, .. } => walk_toplevel(item, f),
        TopLevel::Trigger(t) => walk_expr(&mut t.instance, f),
        TopLevel::TriggerBinding { instance, .. } => walk_expr(instance, f),
        TopLevel::IsrHandler(h) => walk_expr(&mut h.vector, f),
        TopLevel::ProtocolDef(p) => {
            if let Some(c) = &mut p.contract {
                walk_contract(c, f);
            }
        }
        TopLevel::Trait(tr) => {
            for d in &mut tr.functions {
                walk_definition(d, f);
            }
            for b in &mut tr.op_bindings {
                walk_expr(&mut b.expr, f);
            }
        }
        TopLevel::Impl(im) => {
            for d in &mut im.functions {
                walk_definition(d, f);
            }
            for b in &mut im.op_bindings {
                walk_expr(&mut b.expr, f);
            }
        }
        TopLevel::Export(export) => walk_toplevel(&mut export.inner, f),
        TopLevel::SyncGroup { item, .. } => walk_toplevel(item, f),
        TopLevel::Cfg(guard) => {
            for it in &mut guard.items {
                walk_toplevel(it, f);
            }
        }
        TopLevel::CompileTimeLet(_, e) => walk_expr(e, f),
        TopLevel::CompileTimeConst(_, e) => walk_expr(e, f),
        TopLevel::Init(init) => {
            if let Some(v) = &mut init.value {
                walk_expr(v, f);
            }
            walk_stmts(&mut init.body, f);
        }
        // No Expr payloads relevant to the reference closure: imports and
        // exports are names; frgn/asm/bad bodies are non-Briev text or
        // type-only declarations; enums carry variant TYPES only; render/
        // fab/svg blocks are strings and placements; data tables and module
        // config are compile-time data.
        TopLevel::Import(_)
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
pub fn walk_definition<F: FnMut(&mut Expr)>(d: &mut Definition, f: &mut F) {
    walk_contract(&mut d.contract, f);
    if let Some(block) = &mut d.derivation {
        walk_derivation(block, f);
    }
    walk_stmts(&mut d.body, f);
}

fn walk_derivation<F: FnMut(&mut Expr)>(block: &mut DerivationBlock, f: &mut F) {
    for DerivationExample { inputs, output, .. } in &mut block.examples {
        walk_all(inputs, f);
        walk_expr(output, f);
    }
    if let Some(e) = &mut block.synthesized {
        walk_expr(e, f);
    }
    if let Some(e) = &mut block.postcondition {
        walk_expr(e, f);
    }
    if let Some(e) = &mut block.precondition {
        walk_expr(e, f);
    }
    for seg in &mut block.chain {
        match seg {
            ChainSegment::Ref(_) => {}
            ChainSegment::Derivation(inner) => walk_derivation(inner, f),
        }
    }
}

/// Statement-match arms: patterns first, then the arm body. Split out of
/// `walk_stmts` so the walker's loop nesting stays at depth 1 (Praetor).
fn walk_stmt_arms<F: FnMut(&mut Expr)>(arms: &mut [StmtMatchArm], f: &mut F) {
    for arm in arms {
        arm.patterns.iter_mut().for_each(|p| walk_pattern(p, f));
        walk_stmts(&mut arm.body, f);
    }
}

/// A type/obj body's behavioral payload: member defs (whose bodies reference
/// same-module fns — `obj GrowBuf { put_u8 → grow_to_fit }`), op-binding
/// exprs, constraints, and when-laws. Slots/bindings/operators/ports carry
/// Types, not Exprs. Split out of `walk_toplevel` (Praetor complexity).
fn walk_type_body<F: FnMut(&mut Expr)>(td: &mut super::top::TypeDef, f: &mut F) {
    // 2026-10-18 (splice closure): before this descent the closure never
    // walked member bodies and every cross-module importer of such a type
    // failed ("expected GrowBuf, found Int").
    for m in &mut td.body.members {
        walk_toplevel(m, f);
    }
    for b in &mut td.body.op_bindings {
        walk_expr(&mut b.expr, f);
    }
    for c in &mut td.body.constraints {
        walk_expr(c, f);
    }
    for law in &mut td.body.when_laws {
        walk_expr(&mut law.guard, f);
        walk_stmts(&mut law.facts, f);
    }
}

fn walk_all<F: FnMut(&mut Expr)>(exprs: &mut [Expr], f: &mut F) {
    for e in exprs {
        walk_expr(e, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_is_postorder_and_exactly_once() {
        let mut expression = Expr::BinaryOp(super::super::BinaryOpKind::Add,
            Box::new(Expr::Decimal(1)), Box::new(Expr::Decimal(2)));
        let mut order = Vec::new();
        walk_expr(&mut expression, &mut |expression| match expression {
            Expr::Decimal(value) => { order.push(*value); *value += 10; },
            Expr::BinaryOp(_, left, right) => {
                assert_eq!(**left, Expr::Decimal(11));
                assert_eq!(**right, Expr::Decimal(12));
                order.push(3);
            },
            _ => {}
        });
        assert_eq!(order, [1, 2, 3]);
    }

    #[test]
    fn mutable_and_readonly_walkers_cover_the_same_payloads() {
        let source = r#"
            defn walk(value: Int) [value >= 0][term >= 0] -> Int {
                foreach item in 0..2 { when item > 0 { value = value+item; }; };
                match value { 0 => { value = 1; }, _ => { value = 2; }, };
                let nested: Int = { let local: Int = 3; local; };
                term nested;
            };
        "#;
        let tokens = crate::lexer::tokenize(source).expect("visitor tokens");
        let mut program = crate::parser::Parser::new(tokens, source).parse_program().expect("visitor syntax");
        let original = program.clone();
        let mut before = std::collections::HashMap::new();
        let mut after = std::collections::HashMap::new();
        for item in &program {
            crate::ast::visit::walk_toplevel(item, &mut |expression| {
                *before.entry(std::mem::discriminant(expression)).or_insert(0) += 1;
            });
        }
        for item in &mut program {
            walk_toplevel(item, &mut |expression| {
                *after.entry(std::mem::discriminant(expression)).or_insert(0) += 1;
            });
        }
        assert_eq!(before, after);
        assert_eq!(format!("{program:?}"), format!("{original:?}"));
    }
}
