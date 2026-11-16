// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
use briev_verify::ast::{BinaryOpKind as Op, Contract, Definition, Expr, Statement, TopLevel};
use briev_verify::verify::verify_bodies;
fn field(name: &str) -> Expr {
    Expr::Field(Box::new(Expr::Identifier(name.into())), "n".into())
}
#[test]
fn overwritten_rebound_field_cannot_prove_the_old_value() {
    let body = vec![
        Statement::Assign(field("y"), Expr::Decimal(1)),
        Statement::Assign(Expr::Identifier("x".into()), Expr::Identifier("y".into())),
        Statement::Assign(field("x"), Expr::Decimal(2)),
        Statement::Term(Some(Expr::Decimal(0))),
    ];
    let post = Expr::BinaryOp(Op::Eq, Box::new(field("x")), Box::new(Expr::Decimal(1)));
    let item = TopLevel::Definition(Definition::prover_defn(
        "field_rebound",
        Contract::proven(Expr::Bool(true), post),
        body,
    ));
    let report = verify_bodies(&[item]);
    assert!(report.proven.is_empty(), "false field proof: {report:?}");
    assert_eq!(report.violated.len(), 1, "{report:?}");
}

#[test]
fn latest_rebound_field_value_remains_provable() {
    let body = vec![
        Statement::Assign(field("y"), Expr::Decimal(1)),
        Statement::Assign(Expr::Identifier("x".into()), Expr::Identifier("y".into())),
        Statement::Assign(field("x"), Expr::Decimal(2)),
        Statement::Term(Some(Expr::Decimal(0))),
    ];
    let post = Expr::BinaryOp(Op::Eq, Box::new(field("x")), Box::new(Expr::Decimal(2)));
    let item = TopLevel::Definition(Definition::prover_defn(
        "latest_field",
        Contract::proven(Expr::Bool(true), post),
        body,
    ));
    let report = verify_bodies(&[item]);
    assert_eq!(report.proven, vec!["latest_field"], "{report:?}");
}
#[test]
fn every_plain_assignment_invalidates_its_fields_after_reading_rhs() {
    use briev_verify::symbolic::{SymbolicState, SymbolicValue, eval_symbolic};
    let mut state = SymbolicState::empty();
    state.assign_field("x", "n", &Expr::Decimal(7));
    state.assign("x", &field("x"));
    assert!(matches!(
        state.get_value("x"),
        Some(SymbolicValue::Literal(7, _))
    ));
    assert!(matches!(
        eval_symbolic(&field("x"), &state),
        SymbolicValue::Unknown
    ));
}
#[test]
fn a_field_word_cannot_be_equated_to_a_boolean() {
    let body = vec![
        Statement::Assign(field("x"), Expr::Decimal(1)),
        Statement::Term(Some(Expr::Decimal(0))),
    ];
    let post = Expr::BinaryOp(Op::Eq, Box::new(field("x")), Box::new(Expr::Bool(true)));
    let item = TopLevel::Definition(Definition::prover_defn(
        "mixed_field",
        Contract::proven(Expr::Bool(true), post),
        body,
    ));
    let report = verify_bodies(&[item]);
    assert!(report.proven.is_empty(), "{report:?}");
    assert_eq!(report.unproven.len(), 1, "{report:?}");
}
