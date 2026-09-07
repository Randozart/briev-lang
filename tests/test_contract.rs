use briev_compiler::ast::*;
use briev_compiler::lexer::tokenize;
use briev_compiler::parser::Parser;

fn parse_program(source: &str) -> Vec<TopLevel> {
    let tokens = tokenize(source).unwrap();
    let mut p = Parser::new(tokens, source);
    p.parse_program().unwrap()
}

#[test]
fn test_contract_before_arrow() {
    let src = "defn foo(x: Int) -> Int [x > 0][x < 100] { term x; };";
    let prog = parse_program(src);
    let defn = match &prog[0] {
        TopLevel::Definition(d) => d,
        _ => panic!("expected Definition"),
    };
    assert_eq!(defn.name, "foo");
    assert!(matches!(defn.contract.pre_condition, Expr::BinaryOp(..)), "pre should be BinaryOp");
    assert!(matches!(defn.contract.post_condition, Expr::BinaryOp(..)), "post should be BinaryOp");
    assert!(defn.output_type.is_some(), "output_type should be Some");
    let types = defn.output_type.as_ref().unwrap().all_types();
    assert_eq!(types.len(), 1, "should be 1 output type");
    assert_eq!(types[0], Type::int());
}

#[test]
fn test_contract_no_contract() {
    let src = "defn foo(x: Int) -> Int { term x; };";
    let prog = parse_program(src);
    let defn = match &prog[0] {
        TopLevel::Definition(d) => d,
        _ => panic!("expected Definition"),
    };
    assert!(matches!(defn.contract.pre_condition, Expr::Bool(true)));
    assert!(matches!(defn.contract.post_condition, Expr::Bool(true)));
}

#[test]
fn test_transaction_with_contract() {
    let src = "txn foo(x: Int) [x > 0][x < 100] { term; };";
    let prog = parse_program(src);
    let t = match &prog[0] {
        TopLevel::Transaction(t) => t,
        _ => panic!("expected Transaction"),
    };
    assert!(matches!(t.contract.pre_condition, Expr::BinaryOp(..)));
    assert!(matches!(t.contract.post_condition, Expr::BinaryOp(..)));
}
