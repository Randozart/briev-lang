// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Contract-call facts must describe the evaluation that actually happened.
use briev_verify::ast::{
    BinaryOpKind as Op, Contract, Definition, Expr, Statement, TopLevel, Type,
};
use briev_verify::domain::ScalarModel;
use briev_verify::verify::{BodyReport, verify_bodies_with_types};

fn id(name: &str) -> Expr {
    Expr::Identifier(name.into())
}
fn number(value: i64) -> Expr {
    Expr::Decimal(value)
}
fn bin(op: Op, left: Expr, right: Expr) -> Expr {
    Expr::BinaryOp(op, Box::new(left), Box::new(right))
}
fn call(arg: Expr) -> Expr {
    Expr::Call("positive".into(), vec![arg], None)
}
fn assign(name: &str, value: Expr) -> Statement {
    Statement::Assign(id(name), value)
}
fn ret(value: Expr) -> Statement {
    Statement::Term(Some(value))
}

fn positive() -> Definition {
    let mut d = Definition::prover_defn(
        "positive",
        Contract::proven(
            bin(Op::Gt, id("a"), number(0)),
            bin(
                Op::And,
                bin(Op::Eq, id("term"), id("a")),
                bin(Op::Gt, id("term"), number(0)),
            ),
        ),
        vec![ret(id("a"))],
    );
    d.parameters = vec![("a".into(), Type::int())];
    d
}
fn check(expected: i64, body: Vec<Statement>) -> BodyReport {
    let mut caller = Definition::prover_defn(
        "caller",
        Contract::proven(
            bin(Op::Eq, id("x"), number(0)),
            bin(Op::Eq, id("term"), number(expected)),
        ),
        body,
    );
    caller.parameters = vec![("x".into(), Type::int()), ("y".into(), Type::int())];
    verify_bodies_with_types(
        &[
            TopLevel::Definition(positive()),
            TopLevel::Definition(caller),
        ],
        &|_| ScalarModel::SignedWord { bits: 64 },
    )
}
fn must_refute(report: BodyReport) {
    assert!(
        !report.proven.iter().any(|name| name == "caller"),
        "false proof: {report:?}"
    );
    assert!(
        report
            .violated
            .iter()
            .any(|name| name.starts_with("caller:")),
        "missing feasible refutation: {report:?}"
    );
}
// SMT SAT counterexamples remain advisory under the existing public API.
fn must_not_prove(report: BodyReport) {
    assert!(
        !report.proven.iter().any(|name| name == "caller"),
        "{report:?}"
    );
    assert!(
        report
            .unproven
            .iter()
            .chain(&report.violated)
            .any(|name| name.starts_with("caller:")),
        "{report:?}"
    );
}

fn must_prove(report: BodyReport) {
    assert!(
        report.proven.iter().any(|name| name == "caller"),
        "lost valid proof: {report:?}"
    );
}

#[test]
fn an_unexecuted_call_cannot_authorize_a_false_postcondition() {
    let body = vec![
        Statement::Guarded(
            Expr::Bool(false),
            vec![Statement::Expression(call(id("x")))],
        ),
        ret(number(0)),
    ];
    must_refute(check(999, body.clone()));
    must_prove(check(0, body));
}
#[test]
fn call_arguments_are_captured_after_prior_assignments() {
    let body = vec![assign("x", number(1)), ret(call(id("x")))];
    must_refute(check(999, body.clone()));
    must_prove(check(1, body));
}
#[test]
fn guarded_call_facts_do_not_pollute_the_other_path() {
    let body = vec![
        Statement::Guarded(
            bin(Op::Gt, id("y"), number(0)),
            vec![assign("x", number(1)), Statement::Expression(call(id("x")))],
        ),
        ret(number(0)),
    ];
    must_refute(check(999, body.clone()));
    must_prove(check(0, body));
}
#[test]
fn a_call_on_a_short_circuited_rhs_contributes_no_fact() {
    for condition in [
        bin(
            Op::And,
            bin(Op::Gt, id("x"), number(0)),
            bin(Op::Gt, call(id("x")), number(0)),
        ),
        bin(
            Op::Or,
            bin(Op::Eq, id("x"), number(0)),
            bin(Op::Gt, call(id("x")), number(0)),
        ),
    ] {
        let body = vec![
            Statement::Guarded(condition, vec![ret(number(0))]),
            ret(number(0)),
        ];
        must_refute(check(999, body.clone()));
        must_prove(check(0, body));
    }
}
#[test]
fn a_call_on_an_executed_logical_rhs_preserves_valid_bounds() {
    for op in [Op::And, Op::Or] {
        let condition = bin(
            op,
            Expr::Bool(op == Op::And),
            bin(Op::Gt, call(id("x")), number(0)),
        );
        let body = vec![
            assign("x", number(1)),
            Statement::Guarded(condition, vec![ret(number(1))]),
            ret(number(2)),
        ];
        must_prove(check(1, body));
    }
}
#[test]
fn a_saved_call_result_survives_later_argument_writes() {
    let body = vec![
        assign("x", number(1)),
        assign("y", call(id("x"))),
        assign("x", number(2)),
        ret(id("y")),
    ];
    must_prove(check(1, body.clone()));
    must_refute(check(2, body));
}
#[test]
fn the_rhs_call_is_captured_before_overwriting_its_argument() {
    let body = vec![
        assign("x", number(1)),
        assign("x", bin(Op::Add, call(id("x")), number(1))),
        ret(id("x")),
    ];
    must_prove(check(2, body.clone()));
    must_not_prove(check(1, body));
}
#[test]
fn nested_call_facts_follow_argument_evaluation() {
    let body = vec![assign("x", number(1)), ret(call(call(id("x"))))];
    must_prove(check(1, body.clone()));
    must_not_prove(check(999, body));
}

#[test]
fn a_user_binding_cannot_alias_a_generated_call_atom() {
    let name = "call#1#positive#0";
    let body = vec![
        assign("x", number(1)),
        Statement::Let {
            name: name.into(),
            names: vec![name.into()],
            ty: None,
            expr: Some(number(0)),
            modifiers: vec![],
        },
        ret(call(id("x"))),
    ];
    must_prove(check(1, body.clone()));
    must_refute(check(0, body));
}
