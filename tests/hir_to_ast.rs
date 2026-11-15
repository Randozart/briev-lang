// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H4.1 boundary: the HIR→AST reconstruction is lossless for supported nodes
//! (it is the shim that lets AST-emitting consumers take HIR-driven analysis
//! results without changing IR).

use briev_compiler::ast::TopLevel;
use briev_compiler::hir::{HirCallableKind, lower_program, to_ast};

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

#[test]
fn txn_body_round_trips() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(root.join("tests/tier1/iter_chain_multi.bv")).unwrap();
    let items = parse(&src);
    let program = lower_program(&items);
    let ast_txn = items
        .iter()
        .find_map(|i| match i {
            TopLevel::Transaction(t) if t.name == "main" => Some(t),
            _ => None,
        })
        .expect("main txn");
    let hir_txn = program
        .callables
        .iter()
        .find(|c| matches!(c.kind, HirCallableKind::Transaction) && c.name == "main")
        .expect("hir main txn");
    let round = to_ast::stmt_body(&hir_txn.body.statements).expect("supported body");
    assert_eq!(round, ast_txn.body, "HIR→AST body must be lossless");
}

#[test]
fn nested_control_flow_round_trips() {
    let src = "node n [true][true] {\n                 let a: Int = 1;\n                 let b: Int = a + 1;\n                 when b > 1 { a = b; };\n                 foreach i in 0..2 { a = a + i; };\n                 term a;\n               };";
    let items = parse(src);
    let program = lower_program(&items);
    let ast_txn = items
        .iter()
        .find_map(|i| match i {
            TopLevel::Transaction(t) => Some(t),
            _ => None,
        })
        .expect("txn");
    let hir_txn = program
        .callables
        .iter()
        .find(|c| matches!(c.kind, HirCallableKind::Transaction))
        .expect("hir txn");
    assert_eq!(
        to_ast::stmt_body(&hir_txn.body.statements).expect("supported"),
        ast_txn.body
    );
}

#[test]
fn destructuring_names_survive() {
    let src = "defn g() -> (Int, Int) { term (1, 2); };\n\
               defn f() -> Int { let (a, kk): (Int, Int) = g(); term a + kk; };";
    let items = parse(src);
    let program = lower_program(&items);
    let hir_f = program
        .callables
        .iter()
        .find(|c| c.name == "f" && matches!(c.kind, HirCallableKind::Defn))
        .expect("hir f");
    let round = to_ast::stmt_body(&hir_f.body.statements).expect("supported");
    let names = round
        .iter()
        .find_map(|st| match st {
            briev_compiler::ast::Statement::Let { names, .. } => Some(names.clone()),
            _ => None,
        })
        .expect("let");
    assert_eq!(names, vec!["a".to_string(), "kk".to_string()]);
}

// 2026-11-15 (HIR-fidelity): `fn?` (Exists) and formatting annotations are
// lossless — they lower into HIR and reconstruct to the identical AST node,
// so `unsupported_counts` no longer flags them and `to_ast` no longer bails.
#[test]
fn exists_check_round_trips_losslessly() {
    // `g?` is a compile-time function-existence check (Expr::Exists).
    let src = "defn g() -> Int { term 1; };\n\
               defn f() -> Int { let present: Bool = g?; when present { term 1; }; term 0; };";
    let items = parse(src);
    let program = lower_program(&items);
    // The lowered program must carry zero unsupported nodes: `Exists` is a
    // first-class HIR node now, not an `Other("exists")` gap.
    assert_eq!(
        briev_compiler::hir::unsupported_counts(&program).total(),
        0,
        "fn? (Exists) must lower losslessly into HIR"
    );
    // And the body must round-trip to the identical AST statements.
    let hir_f = program
        .callables
        .iter()
        .find(|c| c.name == "f" && matches!(c.kind, HirCallableKind::Defn))
        .expect("hir f");
    let ast_f = items
        .iter()
        .find_map(|i| match i {
            TopLevel::Definition(d) if d.name == "f" => Some(d),
            _ => None,
        })
        .expect("ast f");
    let round =
        to_ast::stmt_body(&hir_f.body.statements).expect("f body must be lossless (no Other node)");
    assert_eq!(round, ast_f.body, "fn? body must round-trip losslessly");
}
