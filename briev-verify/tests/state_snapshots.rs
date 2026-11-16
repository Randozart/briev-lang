// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Saved values and path facts describe the instant they were evaluated.
use briev_verify::ast::{BinaryOpKind as Op, Contract, Definition, Expr, Statement, TopLevel};
use briev_verify::verify::{BodyReport, verify_bodies, verify_bodies_with_types};

fn id(name: &str) -> Expr {
    Expr::Identifier(name.into())
}

fn binary(op: Op, left: Expr, right: Expr) -> Expr {
    Expr::BinaryOp(op, Box::new(left), Box::new(right))
}

fn saved_copy(expression: Expr) -> Vec<Statement> {
    vec![
        Statement::Assign(id("saved"), expression),
        Statement::Assign(id("x"), Expr::Decimal(0)),
        Statement::Term(Some(id("saved"))),
    ]
}

fn check(pre: Expr, post: Expr, body: Vec<Statement>) -> BodyReport {
    verify_bodies(&[TopLevel::Definition(Definition::prover_defn(
        "snapshot",
        Contract::proven(pre, post),
        body,
    ))])
}

#[test]
fn copied_input_and_expression_cannot_be_rewritten_by_later_assignments() {
    for (expression, false_value) in [
        (id("x"), 0),
        (binary(Op::Add, id("x"), Expr::Decimal(1)), 1),
    ] {
        let report = check(
            Expr::Bool(true),
            binary(Op::Eq, id("term"), Expr::Decimal(false_value)),
            saved_copy(expression),
        );
        assert!(
            report.proven.is_empty(),
            "false saved-value proof: {report:?}"
        );
        assert_eq!(report.unproven.len(), 1, "{report:?}");
    }
}

#[test]
fn saved_input_still_has_its_entry_value() {
    let report = check(
        binary(Op::Eq, id("x"), Expr::Decimal(7)),
        binary(Op::Eq, id("term"), Expr::Decimal(7)),
        saved_copy(id("x")),
    );
    assert_eq!(report.proven.len(), 1, "{report:?}");
}

#[test]
fn guard_facts_use_the_value_at_the_guard() {
    let report = check(
        binary(Op::Eq, id("x"), Expr::Decimal(1)),
        binary(Op::Eq, id("term"), id("y")),
        vec![
            Statement::Assign(id("x"), Expr::Decimal(2)),
            Statement::Guarded(
                binary(Op::Eq, id("x"), Expr::Decimal(2)),
                vec![Statement::Term(Some(id("x")))],
            ),
            Statement::Term(Some(id("y"))),
        ],
    );
    assert!(
        report.proven.is_empty(),
        "false contradictory-guard proof: {report:?}"
    );
    assert_eq!(report.unproven.len(), 1, "{report:?}");
}

#[test]
fn a_guard_decided_by_the_current_value_has_only_its_feasible_branch() {
    let report = check(
        binary(Op::Eq, id("x"), Expr::Decimal(1)),
        binary(Op::Eq, id("term"), Expr::Decimal(7)),
        vec![
            Statement::Assign(id("x"), Expr::Decimal(2)),
            Statement::Guarded(
                binary(Op::Eq, id("x"), Expr::Decimal(2)),
                vec![Statement::Term(Some(Expr::Decimal(7)))],
            ),
            Statement::Term(Some(Expr::Decimal(0))),
        ],
    );
    assert_eq!(report.proven.len(), 1, "{report:?}");
}

#[test]
fn helper_summary_preserves_a_copy_before_parameter_mutation() {
    let mut helper = Definition::prover_defn(
        "helper",
        Contract::proven(Expr::Bool(true), Expr::Bool(true)),
        saved_copy(id("x")),
    );
    helper
        .parameters
        .push(("x".into(), briev_verify::ast::Type::int()));
    let caller = Definition::prover_defn(
        "caller",
        Contract::proven(
            Expr::Bool(true),
            binary(Op::Eq, id("term"), Expr::Decimal(0)),
        ),
        vec![Statement::Term(Some(Expr::Call(
            "helper".into(),
            vec![id("input")],
            None,
        )))],
    );
    let report = verify_bodies_with_types(
        &[TopLevel::Definition(helper), TopLevel::Definition(caller)],
        &|_| briev_verify::domain::ScalarModel::SignedWord { bits: 64 },
    );
    assert!(
        report.proven.is_empty(),
        "false helper-snapshot proof: {report:?}"
    );
    assert_eq!(report.unproven.len(), 1, "{report:?}");
}
