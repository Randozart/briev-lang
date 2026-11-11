// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! AST-vs-HIR parity for the reactive-region analysis.
//!
//! Each case builds one AST program, lowers it to HIR, runs both analyzers,
//! and asserts exact equality of the canonical dumps. Full cases also compare
//! after chain composition and budget allocation, and compare the final-value
//! precompute results.

use super::RegionAnalyzer as HirAnalyzer;
use crate::analysis::region::RegionAnalyzer as AstAnalyzer;
use crate::ast::{
    Annotation, BinaryOpKind, Constant, Contract, Expr, Statement, StateDecl, TopLevel,
    Transaction, Trigger, Type,
};
use crate::hir::program::HirProgram;
use std::collections::HashMap;

fn state(name: &str) -> TopLevel {
    TopLevel::StateDecl(StateDecl {
        name: name.to_string(),
        ty: Type::int(),
        span: None,
    })
}

fn trigger(name: &str) -> TopLevel {
    TopLevel::Trigger(Trigger {
        name: name.to_string(),
        instance: Expr::Decimal(0),
        span: None,
    })
}

fn constant(name: &str, value: i64) -> TopLevel {
    TopLevel::Constant(Constant {
        name: name.to_string(),
        ty: Type::int(),
        expr: int(value),
        section: None,
    })
}

fn txn(name: &str, pre: Expr, post: Expr, body: Vec<Statement>) -> TopLevel {
    TopLevel::Transaction(Transaction {
        variadic_param: None,
        name: name.to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        contract: Contract {
            pre_condition: pre,
            post_condition: post,
            watchdog: None,
            explicit: false,
            span: None,
            post_authority: false,
        },
        body,
        span: None,
        metadata: HashMap::new(),
        modifiers: vec![],
        outputs: Vec::new(),
        output_type: None,
        derivation: None,
        doc: None,
    })
}

fn assign(lhs: &str, expr: Expr) -> Statement {
    Statement::Assign(Expr::Identifier(lhs.to_string()), expr)
}

fn guarded(condition: Expr, body: Vec<Statement>) -> Statement {
    Statement::Guarded(condition, body)
}

fn foreach(item: &str, list: Expr, body: Vec<Statement>) -> Statement {
    Statement::Foreach {
        item: item.to_string(),
        list: Box::new(list),
        body,
        modifiers: vec![Annotation { name: "seq".to_string(), value: None }],
    }
}

fn int(n: i64) -> Expr {
    Expr::Decimal(n)
}

fn ident(name: &str) -> Expr {
    Expr::Identifier(name.to_string())
}

fn add(a: Expr, b: Expr) -> Expr {
    Expr::BinaryOp(BinaryOpKind::Add, Box::new(a), Box::new(b))
}

fn less(a: Expr, b: Expr) -> Expr {
    Expr::BinaryOp(BinaryOpKind::Lt, Box::new(a), Box::new(b))
}

fn lower(items: &[TopLevel]) -> HirProgram {
    crate::hir::lower_program(items)
}

fn assert_dump_parity(ast: &AstAnalyzer, hir: &HirAnalyzer, stage: &str) {
    let ast_dump = crate::analysis::region::canonical_dump(ast);
    let hir_dump = crate::hir::region::canonical_dump(hir);
    assert_eq!(hir_dump, ast_dump, "HIR/AST region drift ({stage})");
}

fn analyze_pair(items: &[TopLevel]) -> (AstAnalyzer, HirAnalyzer) {
    let ast = AstAnalyzer::analyze(items);
    let hir = HirAnalyzer::analyze(&lower(items));
    assert_dump_parity(&ast, &hir, "analyze");
    (ast, hir)
}

fn normalize_final(
    values: Option<Vec<(Vec<String>, HashMap<String, i64>)>>,
) -> Option<Vec<(Vec<String>, std::collections::BTreeMap<String, i64>)>> {
    let mut values = values?;
    values.sort_by(|a, b| a.0.cmp(&b.0));
    Some(
        values
            .into_iter()
            .map(|(chain, bindings)| {
                (chain, bindings.into_iter().collect::<std::collections::BTreeMap<_, _>>())
            })
            .collect(),
    )
}

fn assert_full_parity(items: &[TopLevel], budget: u64) {
    let (mut ast, mut hir) = analyze_pair(items);

    ast.compose_chains();
    hir.compose_chains();
    assert_dump_parity(&ast, &hir, "compose_chains");

    ast.build_budget_plan(budget);
    hir.build_budget_plan(budget);
    assert_dump_parity(&ast, &hir, "build_budget_plan");

    let program = lower(items);
    assert_eq!(
        normalize_final(hir.collect_final_values(&program)),
        normalize_final(ast.collect_final_values(items)),
        "final-value precompute drift"
    );

    assert_eq!(
        hir.is_fully_precomputable(budget),
        ast.is_fully_precomputable(budget),
        "precomputability drift"
    );
}

#[test]
fn empty_program_matches() {
    let items: Vec<TopLevel> = Vec::new();
    let (ast, hir) = analyze_pair(&items);
    assert!(ast.var_info.is_empty());
    assert!(hir.var_info.is_empty());
    assert_eq!(hir.region_of("missing"), None);
    assert_eq!(hir.classification_of("missing"), None);
    assert!(!hir.is_frontier_dependent("missing"));
    assert_eq!(hir.value_set_size_of("missing"), None);
}

#[test]
fn single_region_reads_and_writes_match() {
    let items = vec![
        trigger("btn"),
        state("x"),
        txn("bump", Expr::Bool(true), Expr::Bool(true), vec![assign("x", ident("btn"))]),
    ];
    let (ast, hir) = analyze_pair(&items);
    assert_eq!(hir.classification_of("x"), ast.classification_of("x"));
    assert_eq!(hir.region_of("x"), Some(1));
    assert_eq!(hir.regions.len(), 1);
    assert_full_parity(&items, 10);
}

#[test]
fn multiple_independent_regions_match() {
    let items = vec![
        trigger("ta"),
        trigger("tb"),
        state("x"),
        state("y"),
        txn("tx_a", Expr::Bool(true), Expr::Bool(true), vec![assign("x", ident("ta"))]),
        txn("tx_b", Expr::Bool(true), Expr::Bool(true), vec![assign("y", ident("tb"))]),
    ];
    let (_ast, hir) = analyze_pair(&items);
    assert_eq!(hir.regions.len(), 2);
    assert_ne!(hir.region_of("x"), hir.region_of("y"));
    assert_full_parity(&items, 10);
}

#[test]
fn chained_dependency_is_one_region() {
    let items = vec![
        trigger("trg"),
        state("x"),
        state("y"),
        txn("t1", Expr::Bool(true), Expr::Bool(true), vec![assign("x", ident("trg"))]),
        txn("t2", Expr::Bool(true), Expr::Bool(true), vec![assign("y", ident("x"))]),
    ];
    let (_ast, hir) = analyze_pair(&items);
    assert_eq!(hir.regions.len(), 1);
    assert_eq!(hir.region_of("trg"), hir.region_of("x"));
    assert_eq!(hir.region_of("x"), hir.region_of("y"));
    assert_full_parity(&items, 10);
}

#[test]
fn constant_interval_and_value_set_match() {
    let items = vec![constant("total", 100)];
    let (_ast, hir) = analyze_pair(&items);
    assert_eq!(hir.classification_of("total"), Some(crate::analysis::region::VarClass::Pure));
    let info = hir.var_info.get("total").unwrap();
    assert_eq!(info.interval, Some(crate::analysis::region::Interval { lo: 100, hi: 100 }));
    assert_eq!(info.value_set_size, Some(1));
    assert_full_parity(&items, 10);
}

#[test]
fn precondition_dependency_and_iteration_bound_match() {
    let items = vec![
        constant("total", 7),
        state("count"),
        txn(
            "step",
            less(ident("count"), ident("total")),
            Expr::Bool(true),
            vec![assign("count", add(ident("count"), int(1)))],
        ),
    ];
    let (ast, hir) = analyze_pair(&items);
    assert_eq!(hir.iteration_bound_of("step"), ast.iteration_bound_of("step"));
    assert_eq!(hir.iteration_bound_of("step"), Some(7));
    assert_full_parity(&items, 10);
}

#[test]
fn conditional_bodies_match() {
    let items = vec![
        trigger("btn"),
        state("x"),
        txn(
            "cond",
            less(ident("x"), int(10)),
            Expr::Bool(true),
            vec![guarded(ident("btn"), vec![assign("x", add(ident("x"), int(1)))])],
        ),
    ];
    assert_full_parity(&items, 10);
}

#[test]
fn loop_bodies_match() {
    let items = vec![
        trigger("btn"),
        state("x"),
        txn(
            "loop",
            Expr::Bool(true),
            Expr::Bool(true),
            vec![foreach("item", ident("btn"), vec![assign("x", ident("btn"))])],
        ),
    ];
    assert_full_parity(&items, 10);
}

#[test]
fn aliasing_multiple_writers_match() {
    let items = vec![
        trigger("trg"),
        state("x"),
        txn("a", Expr::Bool(true), Expr::Bool(true), vec![assign("x", ident("trg"))]),
        txn("b", Expr::Bool(true), Expr::Bool(true), vec![assign("x", add(ident("trg"), int(1)))]),
        txn("c", Expr::Bool(true), Expr::Bool(true), vec![assign("x", add(ident("x"), int(2)))]),
    ];
    let (_ast, hir) = analyze_pair(&items);
    assert_eq!(hir.regions.len(), 1);
    assert_full_parity(&items, 10);
}

#[test]
fn nested_regions_with_independent_chains_match() {
    let items = vec![
        trigger("ta"),
        trigger("tb"),
        state("x"),
        state("y"),
        txn("a1", Expr::Bool(true), Expr::Bool(true), vec![assign("x", ident("ta"))]),
        txn("a2", Expr::Bool(true), Expr::Bool(true), vec![assign("y", ident("x"))]),
        txn("b1", Expr::Bool(true), Expr::Bool(true), vec![assign("y", ident("tb"))]),
    ];
    assert_full_parity(&items, 10);
}

#[test]
fn terminator_region_is_ineligible_and_unbounded() {
    let items = vec![
        trigger("btn"),
        state("x"),
        txn(
            "exit",
            Expr::Bool(true),
            Expr::Bool(true),
            vec![assign("x", ident("btn")), Statement::Term(Some(int(0)))],
        ),
    ];
    let (_ast, hir) = analyze_pair(&items);
    let score = hir.region_scores.first().expect("region score");
    assert_eq!(score.complexity, crate::analysis::region::ComplexityClass::Unbounded);
    assert!(!score.gpu_eligible);
    assert_full_parity(&items, 10);
}

#[test]
fn all_internal_chain_precomputes_final_values() {
    let items = vec![
        constant("total", 100),
        state("count"),
        state("x"),
        state("y"),
        txn(
            "step_a",
            less(ident("count"), ident("total")),
            Expr::Bool(true),
            vec![assign("x", int(42)), assign("count", add(ident("count"), int(1)))],
        ),
        txn(
            "step_b",
            less(ident("count"), ident("total")),
            Expr::Bool(true),
            vec![assign("y", add(ident("x"), int(1))), assign("count", add(ident("count"), int(1)))],
        ),
    ];
    let (mut ast, mut hir) = analyze_pair(&items);
    ast.compose_chains();
    hir.compose_chains();
    assert_dump_parity(&ast, &hir, "compose_chains");
    assert_eq!(hir.is_fully_precomputable(10), ast.is_fully_precomputable(10));
    assert!(hir.is_fully_precomputable(10));
    assert!(!hir.is_fully_precomputable(0));

    let program = lower(&items);
    let values = normalize_final(hir.collect_final_values(&program)).expect("final values");
    assert!(
        values.iter().any(|(_, bindings)| bindings.get("count") == Some(&100)),
        "final counter must equal the declared bound: {values:?}"
    );
    assert_eq!(
        normalize_final(hir.collect_final_values(&program)),
        normalize_final(ast.collect_final_values(&items))
    );
}

#[test]
fn indexed_write_blocks_precomputation() {
    let items = vec![
        constant("total", 4),
        state("count"),
        state("a"),
        txn(
            "go",
            less(ident("count"), ident("total")),
            Expr::Bool(true),
            vec![
                Statement::Assign(
                    Expr::Index(Box::new(ident("a")), Box::new(ident("count"))),
                    Expr::Float(1.5),
                ),
                assign("count", add(ident("count"), int(1))),
                Statement::Term(None),
            ],
        ),
    ];
    let (mut ast, mut hir) = analyze_pair(&items);
    ast.compose_chains();
    hir.compose_chains();
    assert_dump_parity(&ast, &hir, "compose_chains");
    assert!(!hir.is_fully_precomputable(100));
    assert_eq!(hir.is_fully_precomputable(100), ast.is_fully_precomputable(100));
}

#[test]
fn observable_call_blocks_precomputation() {
    let print = Statement::Expression(Expr::Block(vec![Statement::Expression(Expr::Call(
        "Print#".into(),
        vec![Expr::Identifier("count".into())],
        None,
    ))]));
    let items = vec![
        constant("total", 4),
        state("count"),
        state("x"),
        txn(
            "go",
            less(ident("count"), ident("total")),
            Expr::Bool(true),
            vec![
                assign("x", int(1)),
                print,
                assign("count", add(ident("count"), int(1))),
                Statement::Term(None),
            ],
        ),
    ];
    let (mut ast, mut hir) = analyze_pair(&items);
    ast.compose_chains();
    hir.compose_chains();
    assert_dump_parity(&ast, &hir, "compose_chains");
    assert!(!hir.is_fully_precomputable(100));
    assert_eq!(hir.is_fully_precomputable(100), ast.is_fully_precomputable(100));
}

#[test]
fn budget_plan_parity_when_exhausted() {
    let items = vec![
        trigger("ta"),
        state("x"),
        txn("tx_a", Expr::Bool(true), Expr::Bool(true), vec![assign("x", ident("ta"))]),
    ];
    assert_full_parity(&items, 0);
}

#[test]
fn trigger_binding_and_plain_let_are_not_regions() {
    let items = vec![
        TopLevel::Statement(Box::new(Statement::Let {
            name: "plain".to_string(),
            names: vec![],
            ty: Some(Type::int()),
            expr: Some(int(0)),
            modifiers: vec![],
        })),
        TopLevel::TriggerBinding {
            name: "bound".to_string(),
            ty: None,
            instance: ident("x"),
            port: String::new(),
            modifiers: vec![],
        },
    ];
    let (ast, hir) = analyze_pair(&items);
    assert!(ast.var_info.is_empty());
    assert!(hir.var_info.is_empty());
}
