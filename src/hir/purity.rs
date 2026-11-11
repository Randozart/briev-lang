// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-10 (H5 port): state-purity advisories over typed HIR.
//!
//! Mirror of `analysis::purity` (AST remains the fallback/oracle): an
//! effective-Void, called callable whose body has no observable effect is
//! warned about (never rejected). Conservative by design — any intrinsic/FFI
//! call, state write, spawn, asm, terminator, or call to such a callable
//! counts as an effect.

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::ast::Type;
use crate::diagnostics::Block;
use std::collections::{HashMap, HashSet};

struct Purity {
    effective_void: bool,
    raw_effect: bool,
    edges: HashSet<String>,
}

pub fn analyze(program: &HirProgram) -> Vec<Block> {
    let callables = collect_callables(program);
    let user_names: HashSet<String> = callables.keys().cloned().collect();
    let mut purity: HashMap<String, Purity> = callables
        .into_iter()
        .map(|(name, (effective_void, body))| {
            let mut p = Purity {
                effective_void,
                raw_effect: false,
                edges: HashSet::new(),
            };
            scan_body(body, &user_names, &mut p.raw_effect, &mut p.edges);
            (name, p)
        })
        .collect();

    loop {
        let newly_effectful = propagate_effects(&purity);
        if newly_effectful.is_empty() {
            break;
        }
        mark_effectful(&mut purity, newly_effectful);
    }

    let called = called_names(program);
    let mut warns = Vec::new();
    for callable in program.callables.iter().filter(|c| is_top_item(c)) {
        let Some(p) = purity.get(&callable.name) else {
            continue;
        };
        if !p.effective_void || p.raw_effect || !called.contains(&callable.name) {
            continue;
        }
        let kind = if callable.kind == HirCallableKind::Transaction {
            "txn"
        } else {
            "defn"
        };
        warns.push(
            Block::warning(format!(
                "{kind} '{}' has no observable effect",
                callable.name
            ))
            .why(
                "its body performs no FFI or intrinsic call, no state write, and no call to anything that does — its firing changes nothing the program can observe, so the compiler's liveness rules treat it as eliminable",
            )
            .fix(
                "if calling it is the point, make the effect observable (Print#, an frgn call, a state write) or give it a value output; otherwise delete it",
            ),
        );
    }
    warns
}

/// AST parity: only top-level defn/txn items (export wrappers unwrap in the
/// AST; HIR marks them exported but they are still top-level callables).
fn is_top_item(c: &HirCallable) -> bool {
    c.owner.is_none()
        && !c.is_isr
        && matches!(
            c.kind,
            HirCallableKind::Defn | HirCallableKind::Transaction
        )
}

fn collect_callables<'a>(
    program: &'a HirProgram,
) -> HashMap<String, (bool, &'a [HirStmt])> {
    let mut map = HashMap::new();
    for callable in program.callables.iter().filter(|c| is_top_item(c)) {
        map.insert(
            callable.name.clone(),
            (is_effective_void(callable), callable.body.statements.as_slice()),
        );
    }
    map
}

fn is_effective_void(callable: &HirCallable) -> bool {
    let arrow_void = callable.result.as_ref().map_or(true, |t| *t == Type::Void);
    arrow_void && callable.output_list.iter().all(|t| *t == Type::Void)
}

fn propagate_effects(purity: &HashMap<String, Purity>) -> Vec<String> {
    let mut names: Vec<String> = purity
        .iter()
        .filter(|(_, p)| !p.raw_effect)
        .filter(|(_, p)| {
            p.edges
                .iter()
                .any(|callee| purity.get(callee).is_some_and(|c| c.raw_effect))
        })
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

fn mark_effectful(purity: &mut HashMap<String, Purity>, names: Vec<String>) {
    for name in names {
        if let Some(p) = purity.get_mut(&name) {
            p.raw_effect = true;
        }
    }
}

fn scan_body(
    stmts: &[HirStmt],
    user_names: &HashSet<String>,
    effect: &mut bool,
    edges: &mut HashSet<String>,
) {
    scan_stmts(stmts, user_names, effect, edges);
    crate::hir::expr_walk::walk_stmts(stmts, &mut |e| match &e.kind {
        HirExprKind::Call { callee, .. } => record_call(callee, user_names, effect, edges),
        HirExprKind::Spawn { .. } => *effect = true,
        _ => {}
    });
}

fn scan_stmts(
    stmts: &[HirStmt],
    user_names: &HashSet<String>,
    effect: &mut bool,
    edges: &mut HashSet<String>,
) {
    for stmt in stmts {
        scan_one(stmt, user_names, effect, edges);
    }
}

fn scan_one(
    stmt: &HirStmt,
    user_names: &HashSet<String>,
    effect: &mut bool,
    edges: &mut HashSet<String>,
) {
    match &stmt.kind {
        HirStmtKind::Assign(..)
        | HirStmtKind::ArrowAssign { .. }
        | HirStmtKind::EndProgram(_)
        | HirStmtKind::Trap
        | HirStmtKind::Halt
        | HirStmtKind::Rollback(_)
        | HirStmtKind::TrgBinding { .. }
        | HirStmtKind::InlineAsm { .. }
        | HirStmtKind::MetadataAssignment(..) => *effect = true,
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Mutex(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Guarded(_, body)
        | HirStmtKind::Foreach { body, .. } => scan_stmts(body, user_names, effect, edges),
        HirStmtKind::Match { arms, .. } => {
            for arm in arms {
                scan_stmts(&arm.body, user_names, effect, edges);
            }
        }
        // Check/Gate are compile-time facts; Break/Yield/hints have no
        // observable runtime effect; Let/Expression/Term/Tail effects arrive
        // through the expression walk.
        _ => {}
    }
}

fn record_call(
    name: &str,
    user_names: &HashSet<String>,
    effect: &mut bool,
    edges: &mut HashSet<String>,
) {
    if name.ends_with('#') || !user_names.contains(name) {
        *effect = true;
    } else {
        edges.insert(name.to_string());
    }
}

fn called_names(program: &HirProgram) -> HashSet<String> {
    let user_names: HashSet<String> = program
        .callables
        .iter()
        .filter(|c| is_top_item(c))
        .map(|c| c.name.clone())
        .collect();
    let mut called = HashSet::new();
    let mut record = |e: &HirExpr, called: &mut HashSet<String>| match &e.kind {
        HirExprKind::Call { callee, .. } if user_names.contains(callee) => {
            called.insert(callee.clone());
        }
        HirExprKind::Spawn { type_name, .. } if user_names.contains(type_name) => {
            called.insert(type_name.clone());
        }
        _ => {}
    };
    for callable in program.callables.iter().filter(|c| is_top_item(c)) {
        crate::hir::expr_walk::walk_stmts(&callable.body.statements, &mut |e| {
            record(e, &mut called)
        });
    }
    // AST parity: TopLevel::Statement globals walk their initializer exprs.
    for global in &program.globals {
        if let Some(expr) = &global.expr {
            crate::hir::expr_walk::walk_expr_pub(expr, &mut |e| record(e, &mut called));
        }
    }
    called
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warns(program: &HirProgram) -> Vec<String> {
        analyze(program).into_iter().map(|b| b.render()).collect()
    }

    fn both(src: &str) -> (Vec<String>, Vec<String>) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let program = crate::hir::lower_program(&items);
        (
            warns(&program),
            crate::analysis::purity::analyze(&items)
                .into_iter()
                .map(|b| b.render())
                .collect(),
        )
    }

    const HARNESS: &str = "let n: Int = 0;\nnode main [n == 0][n == 1] {\n    CALL;\n    n = 1;\n    endprogram;\n};\n";

    fn source(prefix: &str, call: &str) -> String {
        format!("{prefix}{}", HARNESS.replace("CALL", call))
    }

    #[test]
    fn effectless_void_defn_matches_ast() {
        let src = source(
            "defn cleanup() -> Void { let y: Int = 1; };\n",
            "cleanup();",
        );
        let (h, a) = both(&src);
        assert_eq!(h, a);
        assert_eq!(h.len(), 1, "{h:?}");
        assert!(h[0].contains("defn 'cleanup' has no observable effect"));
    }

    #[test]
    fn printing_and_transitive_effects_match_ast() {
        let src = source(
            "defn log() -> Void { Print#(1); };\ndefn cleanup() -> Void { log(); };\n",
            "cleanup();",
        );
        let (h, a) = both(&src);
        assert_eq!(h, a);
        assert!(h.is_empty(), "{h:?}");
    }

    #[test]
    fn uncalled_and_value_output_match_ast() {
        let src = "defn cleanup() -> Void { let y: Int = 1; };\n\
                   defn one() -> Int { term 1; };\n\
                   let n: Int = 0;\n\
                   node main [n == 0][n == 1] { n = one(); endprogram; };\n";
        let (h, a) = both(src);
        assert_eq!(h, a);
        assert!(h.is_empty(), "{h:?}");
    }

    #[test]
    fn state_write_and_spawn_match_ast() {
        let state = source(
            "defn work() -> Int { term 1; };\ndefn launch() -> Void { let t = spawn work(); };\n",
            "launch();",
        );
        let (h, a) = both(&state);
        assert_eq!(h, a);
        assert!(h.is_empty(), "{h:?}");
    }
}
