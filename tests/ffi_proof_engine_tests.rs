use briev_compiler::ast::{BinaryOpKind, Expr, Statement, TopLevel, Type};
use briev_compiler::lexer::tokenize;
use briev_compiler::parser::Parser;
use briev_compiler::proof_engine;

fn parse_program(src: &str) -> Vec<TopLevel> {
    let tokens = tokenize(src).unwrap();
    let mut p = Parser::new(tokens, src);
    p.parse_program().unwrap()
}

#[test]
fn test_frgn_binding_parses() {
    let program = parse_program(
        r#"frgn read_file(path: String) -> Int from "libc.so.6";"#,
    );
    let has_frgn = program
        .iter()
        .any(|tl| matches!(tl, TopLevel::ForeignBinding(_)));
    assert!(has_frgn, "expected ForeignBinding in program");
}

#[test]
fn test_frgn_contract_precondition_satisfiable() {
    let pre = Expr::BinaryOp(
        BinaryOpKind::Le,
        Box::new(Expr::Identifier("len".into())),
        Box::new(Expr::Decimal(4096)),
    );
    let post = Expr::Bool(true);

    let result = proof_engine::prove_contract(&pre, &post, &[("len".into(), Type::Custom("Int".into()))], false);
    assert!(result.is_ok(), "precondition should be satisfiable");
}

#[test]
fn test_frgn_contract_postcondition_satisfiable() {
    let pre = Expr::Bool(true);
    let post = Expr::BinaryOp(
        BinaryOpKind::Ge,
        Box::new(Expr::Identifier("result".into())),
        Box::new(Expr::Decimal(0)),
    );

    let result = proof_engine::prove_contract(&pre, &post, &[("result".into(), Type::Custom("Int".into()))], false);
    assert!(result.is_ok(), "postcondition should be satisfiable");
}

#[test]
fn test_frgn_contract_combined_pre_post() {
    let pre = Expr::BinaryOp(
        BinaryOpKind::Gt,
        Box::new(Expr::Identifier("count".into())),
        Box::new(Expr::Decimal(0)),
    );
    let post = Expr::BinaryOp(
        BinaryOpKind::Eq,
        Box::new(Expr::Identifier("result".into())),
        Box::new(Expr::Identifier("count".into())),
    );

    let result = proof_engine::prove_contract(
        &pre,
        &post,
        &[
            ("count".into(), Type::Custom("Int".into())),
            ("result".into(), Type::Custom("Int".into())),
        ],
        false,
    );
    assert!(result.is_ok(), "real contract should be satisfiable");
}

#[test]
fn test_frgn_contract_tautology_rejected() {
    let pre = Expr::Bool(true);
    let post = Expr::Bool(true);

    let result = proof_engine::prove_contract(&pre, &post, &[], true);
    assert!(result.is_err(), "[true][true] explicit contract must be rejected as tautology");
}

#[test]
fn test_frgn_contract_tautology_allowed_when_implicit() {
    let pre = Expr::Bool(true);
    let post = Expr::Bool(true);

    let result = proof_engine::prove_contract(&pre, &post, &[], false);
    assert!(result.is_ok(), "implicit no-contract should not be flagged as tautology");
}

#[test]
fn test_frgn_contract_self_comparison_vacuous() {
    let eq = Expr::BinaryOp(
        BinaryOpKind::Eq,
        Box::new(Expr::Identifier("x".into())),
        Box::new(Expr::Identifier("x".into())),
    );
    assert!(proof_engine::is_vacuously_true(&eq), "x == x is vacuously true");
}

#[test]
fn test_frgn_contract_real_not_vacuous() {
    let neq = Expr::BinaryOp(
        BinaryOpKind::Gt,
        Box::new(Expr::Identifier("x".into())),
        Box::new(Expr::Identifier("y".into())),
    );
    assert!(!proof_engine::is_vacuously_true(&neq), "x > y is not vacuously true");
}

#[test]
fn test_frgn_convergence_linear_body() {
    let program = parse_program(
        r#"frgn write(path: String) from "libc.so.6";

let x: Int = 5;"#,
    );

    for tl in &program {
        if let TopLevel::Definition(def) = tl {
            assert!(
                proof_engine::prove_linear(&def.body),
                "linear body should be provably linear"
            );
        }
    }
}

#[test]
fn test_frgn_convergence_with_bound() {
    let body = vec![Statement::Term(Some(Expr::Decimal(0)))];
    let post = Expr::BinaryOp(
        BinaryOpKind::Eq,
        Box::new(Expr::Identifier("done".into())),
        Box::new(Expr::Decimal(100)),
    );
    assert!(
        proof_engine::check_convergence(&body, &post).is_ok(),
        "convergent with numeric bound"
    );
}

#[test]
fn test_frgn_convergence_without_bound() {
    let guarded = Statement::Guarded(
        Expr::Bool(true),
        vec![Statement::Expression(Expr::Decimal(1))],
    );
    let body = vec![guarded];
    let post = Expr::Bool(true);
    assert!(
        proof_engine::check_convergence(&body, &post).is_err(),
        "non-linear body without bound should fail convergence"
    );
}

#[test]
fn test_split_and() {
    let conjunct = Expr::BinaryOp(
        BinaryOpKind::And,
        Box::new(Expr::Bool(true)),
        Box::new(Expr::Bool(false)),
    );
    let parts = proof_engine::split_and(&conjunct);
    assert_eq!(parts.len(), 2);
}

#[test]
fn test_extract_bound_from_postcondition() {
    let post = Expr::BinaryOp(
        BinaryOpKind::Eq,
        Box::new(Expr::Identifier("done".into())),
        Box::new(Expr::Decimal(256)),
    );
    assert_eq!(proof_engine::extract_bound_from_postcondition(&post), Some(256));
}

#[test]
fn test_expr_cost_literal() {
    assert_eq!(proof_engine::expr_cost(&Expr::Decimal(42)), 1);
    assert_eq!(proof_engine::expr_cost(&Expr::Bool(true)), 1);
}

#[test]
fn test_expr_cost_intrinsic_call() {
    let expr = Expr::Call(
        "Sqrt#".into(),
        vec![Expr::Decimal(4)],
        None,
    );
    assert_eq!(proof_engine::expr_cost(&expr), 6);
}

#[test]
fn test_count_calls_intrinsic() {
    let expr = Expr::Call("AddI64#".into(), vec![Expr::Decimal(1), Expr::Decimal(2)], None);
    let mut intrinsics = 0;
    let mut includes_io = false;
    proof_engine::count_calls(&expr, &mut intrinsics, &mut includes_io);
    assert_eq!(intrinsics, 1);
    assert!(!includes_io);
}

#[test]
fn test_count_calls_io_intrinsic() {
    let expr = Expr::Call("Print#".into(), vec![Expr::Decimal(1)], None);
    let mut intrinsics = 0;
    let mut includes_io = false;
    proof_engine::count_calls(&expr, &mut intrinsics, &mut includes_io);
    assert_eq!(intrinsics, 1);
    assert!(includes_io);
}

#[test]
fn test_detect_tautology_true_true() {
    let err = proof_engine::detect_tautology(&Expr::Bool(true), &Expr::Bool(true), true);
    assert!(err.is_some(), "[true][true] must be a tautology");
}

#[test]
fn test_detect_tautology_not_flagged_when_implicit() {
    let err = proof_engine::detect_tautology(&Expr::Bool(true), &Expr::Bool(true), false);
    assert!(err.is_none(), "no-contract default is not a tautology");
}

#[test]
fn test_check_satisfiable_basic() {
    assert!(proof_engine::check_satisfiable(&Expr::Bool(true), &Expr::Bool(true)));
    assert!(!proof_engine::check_satisfiable(&Expr::Bool(true), &Expr::Bool(false)));
    assert!(!proof_engine::check_satisfiable(&Expr::Decimal(1), &Expr::Decimal(2)));
}
