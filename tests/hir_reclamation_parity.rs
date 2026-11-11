// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! R0 (2026-11-11): reclamation HIR-port parity foundation.
//!
//! The full port needs the AST analyzer's graph/solver translated to HIR. The
//! first gate is that HIR lowering preserves exactly the callable/body/global
//! surfaces the reclamation walker reads, and that the HIR→AST bridge returns
//! each body structurally unchanged. Anything lost here would silently change
//! reclamation facts, so pin it before porting the solver.

use briev_compiler::ast::{Statement, TopLevel};
use briev_compiler::hir::{lower_program, to_ast, HirProgram};

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn ast_body_map(items: &[TopLevel]) -> std::collections::HashMap<String, Vec<Statement>> {
    let mut out = std::collections::HashMap::new();
    for item in items {
        match item {
            TopLevel::Definition(d) => {
                out.insert(d.name.clone(), d.body.clone());
            }
            TopLevel::Transaction(t) => {
                out.insert(t.name.clone(), t.body.clone());
            }
            _ => {}
        }
    }
    out
}

fn ast_bodies(items: &[TopLevel]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for item in items {
        if let TopLevel::Definition(d) = item {
            out.push((d.name.clone(), d.body.len()));
        }
        if let TopLevel::Transaction(t) = item {
            out.push((t.name.clone(), t.body.len()));
        }
    }
    out
}

fn hir_bodies(program: &HirProgram) -> Vec<(String, usize)> {
    program
        .callables
        .iter()
        .filter(|c| {
            c.owner.is_none()
                && !c.is_isr
                && matches!(
                    c.kind,
                    briev_compiler::hir::HirCallableKind::Defn
                        | briev_compiler::hir::HirCallableKind::Transaction
                )
        })
        .map(|c| (c.name.clone(), c.body.statements.len()))
        .collect()
}

const FIXTURES: &[(&str, &str)] = &[
    (
        "alloc_escape",
        r#"
defn make(n: Int) -> Int {
    let p: Int = Alloc#(n) as Int;
    Store#(p, 42, 8);
    term p;
};
txn read_only(n: Int) [n > 0][n > 0] -> Int {
    let p: Int = Alloc#(n) as Int;
    Store#(p, 42, 8);
    term Load#(p, 8);
};
"#,
    ),
    (
        "publish_into_parameter",
        r#"
defn publish(holder: List<Int>) -> Int {
    holder <- 7;
    term 0;
};
txn run(n: Int) [n > 0][n > 0] -> Int {
    let local: List<Int> = [];
    term publish(local);
};
"#,
    ),
    (
        "guards_and_keeps",
        r#"
defn choose(a: Int) -> Int {
    let xs: List<Int> = [1, 2, 3];
    let v: Int = 0;
    when a > 0 { v = xs[0]; };
    when a < 0 { v = xs[1]; };
    keep xs;
    term v;
};
"#,
    ),
];

#[test]
fn hir_preserves_reclamation_inputs_and_round_trips_bodies() {
    for (name, src) in FIXTURES {
        let items = parse(src);
        for item in &items {
            if let TopLevel::Statement(s) = item {
                assert!(matches!(s.as_ref(), Statement::Let { .. }), "{name}: fixture shape");
            }
        }
        let program = lower_program(&items);
        let ast_map = ast_body_map(&items);
        assert_eq!(
            hir_bodies(&program),
            ast_bodies(&items),
            "{name}: callable/body surface drift"
        );

        // Every HIR callable body must reconstruct to an AST body of the same
        // length and statement kinds (the reclamation walker reads AST shapes).
        for callable in &program.callables {
            if callable.owner.is_some() {
                continue;
            }
            let Some(body) = to_ast::stmt_body(&callable.body.statements) else {
                panic!("{name}: body for '{}' is not reconstructible", callable.name);
            };
            let original = ast_map
                .get(&callable.name)
                .unwrap_or_else(|| panic!("{name}: no AST body for '{}'", callable.name));
            assert_eq!(
                &body, original,
                "{name}: HIR→AST body drift in '{}'",
                callable.name
            );
        }

        // Globals/constants the reclamation registration reads.
        let ast_lets: Vec<&String> = items
            .iter()
            .filter_map(|i| match i {
                TopLevel::Statement(s) => match s.as_ref() {
                    Statement::Let { name, .. } => Some(name),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let hir_lets: Vec<&String> = program.globals.iter().map(|g| &g.name).collect();
        assert_eq!(ast_lets, hir_lets, "{name}: global names drift");
    }
}

#[test]
fn reclamation_round_trip_is_stable_under_specialization_inputs() {
    // Generic bodies are registered separately by the AST walker; HIR keeps
    // them callable-shaped. Pin that they exist and round-trip.
    let src = r#"
defn wrap<T>(x: T) -> List<T> {
    let result: List<T> = [];
    result <- x;
    term result;
};
"#;
    let items = parse(src);
    let program = lower_program(&items);
    let generic = program
        .callables
        .iter()
        .find(|c| !c.type_params.is_empty())
        .expect("generic callable lowered");
    assert!(to_ast::stmt_body(&generic.body.statements).is_some());
}
