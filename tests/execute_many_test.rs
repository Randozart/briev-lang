//! execute_many! expansion tests (plan 2026-09-18-execute-many-macro).
//!
//! The macro's contract: `execute_many!(callee, (a), (b, c), …)` expands at
//! the Parsed stage to sequential `callee(…)` statements — one per block,
//! in order; ≥1 block required; statement-only. Verified at the AST level
//! (the plugin rewrites before the typechecker sees it), with the misplace
//! and no-block errors pinned as compile diagnostics.

use briev_compiler::ast::{Expr, Statement, TopLevel};
use briev_compiler::plugin::execute_many_plugin::ExecuteManyPlugin;
use briev_compiler::plugin::Plugin;
use briev_compiler::type_universe::TypeUniverse;

fn parse_prog(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("lex");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn expand(items: Vec<TopLevel>) -> Result<Vec<TopLevel>, String> {
    let mut items = items;
    ExecuteManyPlugin
        .on_ast(&mut items, &mut TypeUniverse::new())
        .map(|_| items)
}

fn expanded_calls(body: &[Statement]) -> Vec<(String, usize)> {
    body.iter()
        .filter_map(|s| match s {
            Statement::Expression(Expr::Call(name, args, _)) => {
                Some((name.clone(), args.len()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn expansion_produces_sequential_calls_in_order() {
    let items = parse_prog(
        "defn f(x: Int, y: Int) { };\n\
         node run [i < 1][i == 1] {\n\
         execute_many!(f, (1, 2), (3, 4), (5, 6));\n\
         };\n",
    );
    let out = expand(items).expect("expansion ok");
    let TopLevel::Transaction(t) = &out[1] else {
        panic!("txn expected");
    };
    let calls = expanded_calls(&t.body);
    assert_eq!(calls.len(), 3, "three blocks -> three calls");
    assert!(calls.iter().all(|(n, a)| n == "f" && *a == 2));
}

#[test]
fn single_and_empty_blocks_shape_correctly() {
    let items = parse_prog(
        "defn g(a: Int) { };\n\
         defn h() { };\n\
         node run [i < 1][i == 1] {\n\
         execute_many!(g, (7));\n\
         execute_many!(h, ());\n\
         };\n",
    );
    let out = expand(items).expect("expansion ok");
    let TopLevel::Transaction(t) = &out[2] else {
        panic!("txn expected");
    };
    // g block: one call, one arg (grouping unwrapped to the bare expr)
    // h block: one call, zero args (empty tuple)
    let calls = expanded_calls(&t.body);
    assert_eq!(
        calls,
        vec![("g".to_string(), 1), ("h".to_string(), 0)],
        "(7) is a single-arg block; () is the empty block"
    );
}

#[test]
fn no_blocks_is_an_error() {
    let items = parse_prog(
        "defn f(x: Int) { };\n\
         node run [i < 1][i == 1] {\n\
         execute_many!(f);\n\
         };\n",
    );
    let err = expand(items).err().expect("zero blocks must error");
    assert!(err.contains("at least one invocation block"), "{err}");
    assert!(err.contains("fix:"), "house style names the fix: {err}");
}

#[test]
fn non_identifier_callee_is_an_error() {
    let items = parse_prog(
        "defn f(x: Int) { };\n\
         node run [i < 1][i == 1] {\n\
         execute_many!(f(1), (2));\n\
         };\n",
    );
    let err = expand(items).err().expect("bad callee must error");
    assert!(err.contains("callee name as its first argument"), "{err}");
}

#[test]
fn let_position_is_rejected_with_fix() {
    let items = parse_prog(
        "defn f(x: Int) -> Int { term x; };\n\
         node run [i < 1][i == 1] {\n\
         let v: Int = execute_many!(f, (1));\n\
         };\n",
    );
    let err = expand(items).err().expect("let position must error");
    assert!(err.contains("statement construct"), "{err}");
    assert!(err.contains("bind calls explicitly"), "{err}");
}

#[test]
fn expansion_reaches_nested_foreach_bodies() {
    // The kernel-nesting case: the composition kernels put work inside
    // foreach bodies — the expansion must reach them.
    let items = parse_prog(
        "defn s(a: Int) { };\n\
         node run [i < 1][i == 1] {\n\
         foreach j in 0..3 {\n\
         execute_many!(s, (1), (2));\n\
         };\n\
         };\n",
    );
    let out = expand(items).expect("expansion ok");
    let TopLevel::Transaction(t) = &out[1] else {
        panic!("txn expected");
    };
    let mut in_foreach = Vec::new();
    for s in &t.body {
        if let Statement::Foreach { body, .. } = s {
            in_foreach = expanded_calls(body);
        }
    }
    assert_eq!(in_foreach.len(), 2, "nested foreach bodies expand too");
}
