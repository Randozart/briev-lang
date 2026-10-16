// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! 2026-10-16 (term-semantics plan step 7): the state-purity probe —
//! observability as liveness, applied to callables.
//!
//! An effective-Void defn/txn whose body has NO observable effect — no
//! FFI or intrinsic call, no state write, no call to anything that has
//! one — is dead weight the reactor still schedules: its firing changes
//! nothing the program can observe, and the compiler's liveness rules
//! would be right to eliminate it. That silence is almost never what the
//! author meant, so this pass says so — a WARNING (born composite via
//! `diagnostics::Block`: what / why / fix), never an error.
//!
//! Conservative by design (false NEGATIVES, never false positives):
//! - ANY intrinsic call counts as an effect (`Sqrt#` too — intrinsics
//!   are the FFI family; "a value is live if an FFI call consumes it").
//! - ANY Assign/ArrowAssign counts (a pure local shuffle goes unwarned).
//! - Effects propagate through the call graph to a fixpoint: a defn
//!   that only prints via a callee is effectful.
//! - Uncalled callables stay silent — dead-defn liveness already
//!   eliminates those before emission (defn_liveness.rs).
//! - Scope: top-level defns/txns (+ `export` wrappers) that are
//!   actually referenced. Nodes are entry points — untouched.

use crate::ast::top::{OutputType, TopLevel};
use crate::ast::{Expr, Statement, Type};
use crate::diagnostics::Block;
use std::collections::{HashMap, HashSet};

/// One callable's effect scan result.
struct Purity {
    /// Effective output is void: declared `-> Void`, or arrowless after
    /// step-6 inference resolved every value site away.
    effective_void: bool,
    /// Raw effect seen in this body (intrinsic/foreign call, state
    /// write, spawn, asm, …).
    raw_effect: bool,
    /// User-defined callables this body calls (edges for the fixpoint).
    edges: HashSet<String>,
}

/// Warn for every called, effective-Void defn/txn with no observable
/// effect. Insertion order (AST order) — deterministic output.
pub fn analyze(items: &[TopLevel]) -> Vec<Block> {
    let callables = collect_callables(items);
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

    // Effects propagate through calls to a fixpoint: a defn that only
    // prints via a callee is effectful. Terminates: each round either
    // marks at least one new callable effectful or stops.
    loop {
        let newly_effectful = propagate_effects(&purity);
        if newly_effectful.is_empty() {
            break;
        }
        mark_effectful(&mut purity, newly_effectful);
    }

    let called = called_names(items);
    let mut warns = Vec::new();
    for item in items {
        let Some((kind, name)) = callable_ref(item) else {
            continue;
        };
        let Some(p) = purity.get(name) else {
            continue;
        };
        if !p.effective_void || p.raw_effect || !called.contains(name) {
            continue;
        }
        warns.push(
            Block::warning(format!(
                "{kind} '{name}' has no observable effect"
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

/// (kind, name) for the callables this pass warns about — `export`
/// wrappers unwrap to their inner callable.
fn callable_ref(item: &TopLevel) -> Option<(&'static str, &String)> {
    match item {
        TopLevel::Definition(d) => Some(("defn", &d.name)),
        TopLevel::Transaction(t) => Some(("txn", &t.name)),
        TopLevel::Export(e) => callable_ref(&e.inner),
        _ => None,
    }
}

/// Apply one fixpoint round's promotions.
fn mark_effectful(purity: &mut HashMap<String, Purity>, names: Vec<String>) {
    for name in names {
        if let Some(p) = purity.get_mut(&name) {
            p.raw_effect = true;
        }
    }
}

/// One fixpoint round: the callables that become effectful because some
/// callee they call is effectful.
fn propagate_effects(purity: &HashMap<String, Purity>) -> Vec<String> {
    purity
        .iter()
        .filter(|(_, p)| !p.raw_effect)
        .filter(|(_, p)| {
            p.edges
                .iter()
                .any(|callee| purity.get(callee).is_some_and(|c| c.raw_effect))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// name → (effective void?, body) for every top-level callable.
fn collect_callables(items: &[TopLevel]) -> HashMap<String, (bool, &[Statement])> {
    let mut map = HashMap::new();
    for item in items {
        match item {
            TopLevel::Definition(d) => {
                map.insert(d.name.clone(), (is_effective_void(&d.output_type), d.body.as_slice()));
            }
            TopLevel::Transaction(t) => {
                map.insert(t.name.clone(), (is_effective_void(&t.output_type), t.body.as_slice()));
            }
            TopLevel::Export(e) => match e.inner.as_ref() {
                TopLevel::Definition(d) => {
                    map.insert(d.name.clone(), (is_effective_void(&d.output_type), d.body.as_slice()));
                }
                TopLevel::Transaction(t) => {
                    map.insert(t.name.clone(), (is_effective_void(&t.output_type), t.body.as_slice()));
                }
                _ => {}
            },
            _ => {}
        }
    }
    map
}

/// Declared `-> Void`, or arrowless (post-inference None = every value
/// site resolved away — the backend emits void either way).
fn is_effective_void(ot: &Option<OutputType>) -> bool {
    match ot {
        None => true,
        Some(OutputType::Single(Type::Void)) => true,
        Some(other) => {
            let tys = other.all_types();
            tys.iter().all(|t| matches!(t, Type::Void))
        }
    }
}

/// Scan one body: effect-bearing statement forms + every Call/Spawn
/// expression (via the shared walker, which descends nested blocks).
fn scan_body(
    stmts: &[Statement],
    user_names: &HashSet<String>,
    effect: &mut bool,
    edges: &mut HashSet<String>,
) {
    scan_stmts(stmts, user_names, effect, edges);
    for stmt in stmts {
        crate::ast::visit::walk_stmts(std::slice::from_ref(stmt), &mut |e| match e {
            Expr::Call(name, _, _) => record_call(name, user_names, effect, edges),
            Expr::Spawn { .. } => *effect = true,
            _ => {}
        });
    }
}

fn scan_stmts(
    stmts: &[Statement],
    user_names: &HashSet<String>,
    effect: &mut bool,
    edges: &mut HashSet<String>,
) {
    for s in stmts {
        scan_one(s, user_names, effect, edges);
    }
}

/// One statement's effect contribution: raw-effect forms flag directly;
/// block forms recurse; everything else arrives via the expr walk.
fn scan_one(
    s: &Statement,
    user_names: &HashSet<String>,
    effect: &mut bool,
    edges: &mut HashSet<String>,
) {
    match s {
        Statement::Assign(_, _)
        | Statement::ArrowAssign { .. }
        | Statement::EndProgram(_)
        | Statement::Trap
        | Statement::Halt
        | Statement::Rollback(_)
        | Statement::TrgBinding { .. }
        | Statement::InlineAsm { .. }
        | Statement::MetadataAssignment(_, _) => *effect = true,
        Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Mutex(body)
        | Statement::Defer(body)
        | Statement::Guarded(_, body)
        | Statement::Foreach { body, .. } => {
            scan_stmts(body, user_names, effect, edges);
        }
        Statement::Match { arms, .. } => {
            for arm in arms {
                scan_stmts(&arm.body, user_names, effect, edges);
            }
        }
        // Check/Gate are compile-time facts; Break/Yield/hints have no
        // observable runtime effect; Let/Expression/Term/Tail effects
        // arrive through the expression walk.
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
        // Intrinsics are the FFI family; an unknown name is an import
        // (frgn/stdlib) — both observable by definition.
        *effect = true;
    } else {
        edges.insert(name.to_string());
    }
}

/// Every user callable referenced anywhere in the program — calls and
/// task spawns. Anything unreferenced stays silent (defn liveness
/// already drops it before emission).
fn called_names(items: &[TopLevel]) -> HashSet<String> {
    let mut called = HashSet::new();
    let user_names: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            TopLevel::Definition(d) => Some(d.name.clone()),
            TopLevel::Transaction(t) => Some(t.name.clone()),
            _ => None,
        })
        .collect();
    let mut walk_item = |stmts: &[Statement], called: &mut HashSet<String>| {
        crate::ast::visit::walk_stmts(stmts, &mut |e| match e {
            Expr::Call(name, _, _) if user_names.contains(name) => {
                called.insert(name.clone());
            }
            Expr::Spawn { type_name, .. } if user_names.contains(type_name) => {
                called.insert(type_name.clone());
            }
            _ => {}
        });
    };
    for item in items {
        match item {
            TopLevel::Definition(d) => walk_item(&d.body, &mut called),
            TopLevel::Transaction(t) => walk_item(&t.body, &mut called),
            TopLevel::Export(e) => match e.inner.as_ref() {
                TopLevel::Definition(d) => walk_item(&d.body, &mut called),
                TopLevel::Transaction(t) => walk_item(&t.body, &mut called),
                _ => {}
            },
            TopLevel::Statement(s) => {
                crate::ast::visit::walk_stmts(std::slice::from_ref(s), &mut |e| match e {
                    Expr::Call(name, _, _) if user_names.contains(name) => {
                        called.insert(name.clone());
                    }
                    Expr::Spawn { type_name, .. } if user_names.contains(type_name) => {
                        called.insert(type_name.clone());
                    }
                    _ => {}
                });
            }
            _ => {}
        }
    }
    called
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warns(src: &str) -> Vec<String> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        let items = p.parse_program().unwrap();
        analyze(&items)
            .into_iter()
            .map(|b| b.render())
            .collect()
    }

    const HARNESS: &str = r#"
let n: Int = 0;
node main [n == 0][n == 1] {
    CALL;
    n = 1;
    endprogram;
};
"#;

    /// Effectless + void + called = warned.
    #[test]
    fn effectless_void_defn_warns() {
        let src = format!(
            "defn cleanup() -> Void {{ let y: Int = 1; }};\n{}",
            HARNESS.replace("CALL", "cleanup();")
        );
        let ws = warns(&src);
        assert_eq!(ws.len(), 1, "one warning, got: {ws:?}");
        assert!(ws[0].contains("defn 'cleanup' has no observable effect"));
    }

    /// A printing body is observably alive.
    #[test]
    fn printing_defn_no_warn() {
        let src = format!(
            "defn cleanup() -> Void {{ Print#(1); }};\n{}",
            HARNESS.replace("CALL", "cleanup();")
        );
        assert!(warns(&src).is_empty());
    }

    /// Effects propagate through the call graph.
    #[test]
    fn transitive_effect_no_warn() {
        let src = format!(
            "defn log() -> Void {{ Print#(1); }};\ndefn cleanup() -> Void {{ log(); }};\n{}",
            HARNESS.replace("CALL", "cleanup();")
        );
        assert!(warns(&src).is_empty(), "effect flows through the callee");
    }

    /// Uncalled callables stay silent — defn liveness owns those.
    #[test]
    fn uncalled_effectless_no_warn() {
        let src = "defn cleanup() -> Void { let y: Int = 1; };\n\
                   let n: Int = 0;\n\
                   node main [n == 0][n == 1] { n = 1; endprogram; };\n";
        assert!(warns(&src).is_empty());
    }

    /// A value-output defn is live by construction — even a pure one.
    #[test]
    fn value_output_no_warn() {
        let src = format!(
            "defn one() -> Int {{ term 1; }};\n{}",
            HARNESS.replace("CALL", "let r: Int = one();")
        );
        assert!(warns(&src).is_empty());
    }

    /// A state write is an observable effect (txn case).
    #[test]
    fn state_write_no_warn() {
        let src = format!(
            "let n: Int = 0;\ntxn bump() [n >= 0][n >= 0] {{ n <- n + 1; }};\n{}",
            HARNESS.replace("CALL", "bump();")
        );
        assert!(warns(&src).is_empty());
    }

    /// Spawn makes the spawner observably alive.
    #[test]
    fn spawn_is_an_effect() {
        let src = format!(
            "defn work() -> Int {{ term 1; }};\ndefn launch() -> Void {{ let t = spawn work(); }};\n{}",
            HARNESS.replace("CALL", "launch();")
        );
        assert!(warns(&src).is_empty());
    }
}
