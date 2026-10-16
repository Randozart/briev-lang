// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! 2026-10-16 (term-semantics plan step 6): arrowless output inference.
//!
//! A defn or txn with no `-> T` is not "untyped": today it silently
//! compiles as Void and DROPS a value `term <e>;` / tail (probe: the call
//! site binds no result and the value vanishes). This pass records the
//! effective output — the type of the body's FIRST value-returning site —
//! back onto the AST item, so every downstream consumer (the
//! `fn_return_types` call-inference table, the `Spawn` Task<R>
//! registration in the state pre-collect, the body's own output check,
//! and the backend's signature emission) sees one type.
//!
//! Scope discipline:
//! - Only TOP-LEVEL defns/txns (+ `export` wrappers). Obj/cell member
//!   bodies resolve through a separate context path (`build_member_ctx`)
//!   and keep today's behaviour; widening is a later slice.
//! - A term/tail the MINIMAL inference context cannot resolve (state,
//!   params, defined calls, intrinsics only — no self/slots/op
//!   overloads) leaves the item arrowless = Void, the exact pre-existing
//!   behaviour. Never an error: existing programs keep compiling.
//! - No value site (`term;` bare, plain statements) → Void, unchanged.
//! - Nodes are always Void — untouched.
//!
//! Runs as a fixpoint (forward references between arrowless items
//! resolve once the callee is inferred) and BEFORE the state /
//! `fn_return_types` pre-collects in `check_program_with_target`, so
//! every consumer sees inferred outputs in the same pass.

use super::{infer_type_only, TypecheckContext};
use crate::ast::top::{OutputType, TopLevel};
use crate::ast::{Expr, Statement, Type};
use crate::type_universe::TypeUniverse;
use std::collections::{HashMap, HashSet};

/// Infer and record output types for every arrowless defn/txn in the
/// program. Mutates `items` in place (the item's `output_type` becomes
/// the single source of truth for the rest of the pipeline).
pub fn infer_arrowless_outputs(items: &mut [TopLevel], universe: &TypeUniverse) {
    let defined_fns: HashSet<String> = items
        .iter()
        .filter_map(|item| match item {
            TopLevel::Definition(d) => Some(d.name.clone()),
            TopLevel::Transaction(t) => Some(t.name.clone()),
            _ => None,
        })
        .collect();
    let mut returns = declared_returns(items);
    let state = visible_state(items);

    // Fixpoint: each pass can resolve chains one link longer (a term
    // calling an arrowless callee defined later in the file). Terminates:
    // every pass either rewrites at least one item from arrowless to
    // typed (strictly shrinking the arrowless set) or stops.
    loop {
        let rewrites: Vec<(usize, String, OutputType)> = items
            .iter()
            .enumerate()
            .filter_map(|(idx, item)| {
                try_infer_item(item, universe, &returns, &state, &defined_fns)
                    .map(|(name, ty)| (idx, name, type_to_output(ty)))
            })
            .collect();
        if rewrites.is_empty() {
            break;
        }
        apply_rewrites(items, &mut returns, rewrites);
    }
}

/// Record one pass's inferences: on the items (the single source of
/// truth for everything downstream) and in the fixpoint's returns table
/// (so later passes see the new callees).
fn apply_rewrites(
    items: &mut [TopLevel],
    returns: &mut HashMap<String, Type>,
    rewrites: Vec<(usize, String, OutputType)>,
) {
    for (idx, name, out) in rewrites {
        if let Some(ty) = output_first_type(&out) {
            returns.insert(name, ty);
        }
        apply_output(&mut items[idx], out);
    }
}

/// Declared return types of all typed callables — the inference
/// fixpoint's base table (calls into typed fns resolve immediately).
fn declared_returns(items: &[TopLevel]) -> HashMap<String, Type> {
    let mut map = HashMap::new();
    for item in items {
        match item {
            TopLevel::Definition(d) => {
                insert_first(&mut map, &d.name, d.output_type.as_ref())
            }
            TopLevel::Transaction(t) => {
                insert_first(&mut map, &t.name, t.output_type.as_ref())
            }
            _ => {}
        }
    }
    map
}

/// A declared `Single(Tuple)` record's table entry is its first element
/// (mirrors the `fn_return_types` collect's 4714 unwrap); tuples as
/// `OutputType::Tuple` flow through `output_all_types` element-wise.
fn insert_first(map: &mut HashMap<String, Type>, name: &str, ot: Option<&OutputType>) {
    if let Some(ty) = ot.and_then(|o| output_all_types(o).into_iter().next()) {
        map.insert(name.to_string(), ty);
    }
}

/// Names + types visible to a term/tail expression in any body: typed
/// top-level lets, constants, data tables, trigger pins (mirror of the
/// state pre-collect's read set, read-only).
fn visible_state(items: &[TopLevel]) -> Vec<(String, Type)> {
    let mut out = Vec::new();
    for item in items {
        match item {
            TopLevel::Statement(s) => {
                if let Statement::Let {
                    name,
                    ty: Some(ty),
                    ..
                } = s.as_ref()
                {
                    out.push((name.clone(), ty.clone()));
                }
            }
            TopLevel::Constant(c) => out.push((c.name.clone(), c.ty.clone())),
            TopLevel::Data(t) => out.push((t.name.clone(), t.ty.clone())),
            TopLevel::Trigger(trg) => out.push((trg.name.clone(), Type::int())),
            TopLevel::Export(e) => {
                if let TopLevel::Constant(c) = e.inner.as_ref() {
                    out.push((c.name.clone(), c.ty.clone()));
                }
            }
            _ => {}
        }
    }
    out
}

/// Scan for the first value site, accumulating the TYPED local `let`
/// bindings in scope at that site (`locals`, mutated in place). A branch
/// that contains the site contributes its own bindings (cloned per
/// branch — a sibling branch's locals are not in scope). Typeless
/// `let`s are skipped: a site referencing one stays unresolved (the
/// item keeps today's arrowless behaviour — never an error).
fn first_value_site<'a>(
    stmts: &'a [Statement],
    locals: &mut Vec<(String, Type)>,
) -> Option<&'a Expr> {
    for s in stmts {
        let found = match s {
            Statement::Let {
                name,
                names,
                ty: Some(ty),
                ..
            } => {
                push_let_binding(locals, name, names, ty);
                None
            }
            Statement::Term(Some(e)) => Some(e),
            Statement::Tail(e) => Some(e),
            Statement::Guarded(_, body) => descend_branch(body, locals),
            Statement::Block(body)
            | Statement::SyncBlock(body)
            | Statement::Mutex(body)
            | Statement::Defer(body)
            | Statement::Foreach { body, .. } => descend_branch(body, locals),
            Statement::Match { arms, .. } => {
                arms.iter().find_map(|a| descend_branch(&a.body, locals))
            }
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// A typed `let` (single or tuple-destructured) binds every name it
/// declares at the site's scope.
fn push_let_binding(locals: &mut Vec<(String, Type)>, name: &str, names: &[String], ty: &Type) {
    locals.push((name.to_string(), ty.clone()));
    for n in names {
        locals.push((n.clone(), ty.clone()));
    }
}

/// Descend into a nested block with a fresh locals layer; on a hit, the
/// block's accumulated bindings merge back into the caller's scope.
fn descend_branch<'a>(body: &'a [Statement], locals: &mut Vec<(String, Type)>) -> Option<&'a Expr> {
    let mut branch_locals = locals.clone();
    let base = branch_locals.len();
    let found = first_value_site(body, &mut branch_locals);
    if found.is_some() {
        locals.extend(branch_locals.drain(base..));
    }
    found
}

/// Try to infer one item's output: arrowless + a first value site whose
/// type resolves in the minimal context. Any resolution failure = None
/// (the item keeps today's arrowless/Void behaviour).
fn try_infer_item(
    item: &TopLevel,
    universe: &TypeUniverse,
    returns: &HashMap<String, Type>,
    state: &[(String, Type)],
    defined_fns: &HashSet<String>,
) -> Option<(String, Type)> {
    let (name, params, body, arrowless) = match item {
        TopLevel::Definition(d) => (
            &d.name,
            &d.parameters,
            &d.body,
            d.output_type.is_none(),
        ),
        TopLevel::Transaction(t) => (
            &t.name,
            &t.parameters,
            &t.body,
            t.output_type.is_none(),
        ),
        _ => return None,
    };
    if !arrowless {
        return None;
    }
    let mut locals = Vec::new();
    let expr = first_value_site(body, &mut locals)?;
    let mut ctx = TypecheckContext::new(universe);
    for (pname, pty) in params {
        ctx.bindings.insert(pname.clone(), pty.clone());
        ctx.state_keys.insert(pname.clone());
    }
    for (sname, sty) in state {
        ctx.bindings.insert(sname.clone(), sty.clone());
    }
    for (lname, lty) in locals {
        ctx.bindings.insert(lname, lty);
    }
    ctx.defined_fns = defined_fns.clone();
    ctx.fn_return_types = returns.clone();
    infer_type_only(expr, &mut ctx).ok().map(|ty| (name.clone(), ty))
}

/// Type → OutputType: tuples map element-wise (a `term (a, b);` records
/// `OutputType::Tuple`, so call sites see the FULL tuple — a
/// `Single(Tuple)` would be unwrapped to its first element by the
/// `fn_return_types` collect).
fn type_to_output(ty: Type) -> OutputType {
    match ty {
        Type::Tuple(elems) => {
            OutputType::Tuple(elems.into_iter().map(type_to_output).collect())
        }
        other => OutputType::Single(other),
    }
}

fn output_first_type(ot: &OutputType) -> Option<Type> {
    match ot {
        OutputType::Single(t) => Some(t.clone()),
        OutputType::Tuple(ts) => ts.first().and_then(output_first_type),
        _ => None,
    }
}

fn output_all_types(ot: &OutputType) -> Vec<Type> {
    match ot {
        OutputType::Single(t) => vec![t.clone()],
        OutputType::Tuple(ts) => ts.iter().flat_map(output_all_types).collect(),
        _ => vec![],
    }
}

/// Record the inferred output on the item itself — the one rewrite this
/// pass performs; everything downstream reads `output_type` as usual.
fn apply_output(item: &mut TopLevel, out: OutputType) {
    match item {
        TopLevel::Definition(d) => d.output_type = Some(out),
        TopLevel::Transaction(t) => t.output_type = Some(out),
        TopLevel::Export(e) => apply_output(&mut e.inner, out),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::TypeError;

    fn check(src: &str) -> Result<(), Vec<TypeError>> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        let mut items = p.parse_program().unwrap();
        let universe = crate::type_universe::TypeUniverse::new();
        crate::typechecker::check_program(&mut items, &universe)
    }

    const HARNESS: &str = r#"
let n: Int = 0;
node main [n == 0][n == 1] {
    let r: TYPE = f(1);
    n = 1;
    endprogram;
};
"#;

    fn bound(binding: &str) -> String {
        HARNESS.replace("TYPE", binding)
    }

    /// The discriminator: an arrowless defn's value term infers Int, so
    /// binding the result as Bool is a type error. (Before this pass the
    /// call resolved "unknown" and silently accepted ANY annotation.)
    #[test]
    fn inferred_term_result_is_typed_at_call_site() {
        let src = format!("defn f(x: Int) {{ term x * 2; }};\n{}", bound("Bool"));
        assert!(check(&src).is_err(), "Int result bound as Bool must error");
        let src = format!("defn f(x: Int) {{ term x * 2; }};\n{}", bound("Int"));
        check(&src).expect("Int result bound as Int typechecks");
    }

    /// Same through a tail.
    #[test]
    fn inferred_tail_result_is_typed_at_call_site() {
        let src = format!("defn f(x: Int) {{ x * 2 }};\n{}", bound("Bool"));
        assert!(check(&src).is_err(), "tail Int result bound as Bool must error");
        let src = format!("defn f(x: Int) {{ x * 2 }};\n{}", bound("Int"));
        check(&src).expect("tail Int result bound as Int typechecks");
    }

    /// No value site → the item stays arrowless and the call site keeps
    /// the pre-existing `fn_return_types` Int fallback (4732-era wart) —
    /// a Bool binding errors with "found Int" exactly as before this
    /// pass; an Int binding compiles (regression guard).
    #[test]
    fn arrowless_without_value_site_keeps_int_fallback() {
        let src = format!(
            "defn f(x: Int) {{ let y: Int = x + 1; }};\n{}",
            bound("Bool")
        );
        let errs = check(&src).expect_err("Int fallback: Bool binding errors as before");
        assert!(
            errs.iter().any(|e| format!("{e:?}").contains("found: \"Int\"")),
            "expected the Int fallback at the call site, got: {errs:?}"
        );
        let src = format!(
            "defn f(x: Int) {{ let y: Int = x + 1; }};\n{}",
            bound("Int")
        );
        check(&src).expect("Int fallback binding still compiles");
    }

    /// Forward reference: the caller precedes the callee; the fixpoint
    /// still infers.
    #[test]
    fn forward_reference_resolves_via_fixpoint() {
        let src = format!(
            "defn later(x: Int) {{ term x + 1; }};\n{}",
            bound("Bool")
        );
        assert!(check(&src).is_err(), "forward-inferred Int bound as Bool must error");
        let src = format!(
            "defn later(x: Int) {{ term x + 1; }};\n{}",
            bound("Int")
        );
        check(&src).expect("forward-inferred Int bound as Int typechecks");
    }

    /// Txns mirror defns (with the mandatory non-trivial contract).
    #[test]
    fn txn_value_term_infers_output() {
        let good = format!(
            "txn f(x: Int) [x >= 0][x >= 0] {{ term x * 2; }};\n{}",
            bound("Int")
        );
        check(&good).expect("inferred txn Int bound as Int typechecks");
        let bad = format!(
            "txn f(x: Int) [x >= 0][x >= 0] {{ term x * 2; }};\n{}",
            bound("Bool")
        );
        assert!(check(&bad).is_err(), "inferred txn Int bound as Bool must error");
    }

    /// A typed local `let` before the term IS in scope at the site — the
    /// common `let y: Int = ...; term y + 1;` shape infers normally.
    #[test]
    fn typed_local_visible_to_term() {
        let src = format!(
            "defn f(x: Int) {{ let y: Int = x * 2; term y + 1; }};\n{}",
            bound("Bool")
        );
        assert!(check(&src).is_err(), "local-fed Int result bound as Bool must error");
        let src = format!(
            "defn f(x: Int) {{ let y: Int = x * 2; term y + 1; }};\n{}",
            bound("Int")
        );
        check(&src).expect("local-fed Int result bound as Int typechecks");
    }

    /// A term the minimal context cannot resolve (typeless local here)
    /// leaves the item arrowless — inference never invents an error, and
    /// the call site keeps the pre-existing Int fallback.
    #[test]
    fn unresolvable_term_stays_arrowless() {
        let src = format!(
            "defn f(x: Int) {{ let y = x * 2; term y + 1; }};\n{}",
            bound("Bool")
        );
        let errs = check(&src).expect_err("unresolved term: Int fallback as before");
        assert!(
            errs.iter().any(|e| format!("{e:?}").contains("found: \"Int\"")),
            "expected the Int fallback at the call site, got: {errs:?}"
        );
        let src = format!(
            "defn f(x: Int) {{ let y = x * 2; term y + 1; }};\n{}",
            bound("Int")
        );
        check(&src).expect("Int fallback binding still compiles");
    }
}
