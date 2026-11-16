// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
use briev_verify::ast::{BinaryOpKind as Op, Contract, Definition, Expr, Statement, TopLevel};
use briev_verify::verify::{BodyReport, verify_bodies};

fn comparison(op: Op, left: Expr, right: Expr) -> Expr {
    Expr::BinaryOp(op, Box::new(left), Box::new(right))
}

fn x() -> Expr {
    Expr::Identifier("x".into())
}
fn term() -> Expr {
    Expr::Identifier("term".into())
}

fn check(pre: Expr, post: Expr, body: Vec<Statement>) -> BodyReport {
    verify_bodies(&[TopLevel::Definition(Definition::prover_defn(
        "bounds",
        Contract::proven(pre, post),
        body,
    ))])
}

#[test]
fn reversed_comparisons_cannot_prove_the_opposite_sign() {
    let report = check(
        comparison(Op::Lt, Expr::Decimal(0), x()),
        comparison(Op::Lt, term(), Expr::Decimal(0)),
        vec![Statement::Term(Some(x()))],
    );
    assert!(report.proven.is_empty(), "false reversed bound: {report:?}");
    assert_eq!(report.violated.len(), 1, "{report:?}");
}

#[test]
fn impossible_strict_endpoint_guards_are_unreachable() {
    for (op, endpoint) in [(Op::Gt, i64::MAX), (Op::Lt, i64::MIN)] {
        let report = check(
            Expr::Bool(true),
            comparison(Op::Eq, term(), x()),
            vec![
                Statement::Guarded(
                    comparison(op, x(), Expr::Decimal(endpoint)),
                    vec![Statement::Term(Some(Expr::Decimal(5)))],
                ),
                Statement::Term(Some(x())),
            ],
        );
        assert_eq!(report.proven.len(), 1, "{report:?}");
    }
}

#[test]
fn unreachable_boundary_branches_cannot_create_constant_violations() {
    for (op, endpoint) in [(Op::Gt, i64::MAX), (Op::Lt, i64::MIN)] {
        let report = check(
            Expr::Bool(true),
            comparison(Op::Eq, term(), Expr::Decimal(0)),
            vec![
                Statement::Guarded(
                    comparison(op, x(), Expr::Decimal(endpoint)),
                    vec![Statement::Term(Some(Expr::Decimal(5)))],
                ),
                Statement::Term(Some(Expr::Decimal(0))),
            ],
        );
        assert_eq!(report.proven.len(), 1, "{report:?}");
    }
}

#[test]
fn contradictory_interval_facts_cannot_refute_a_postcondition() {
    let pre = comparison(
        Op::And,
        comparison(Op::Ge, x(), Expr::Decimal(5)),
        comparison(Op::Lt, x(), Expr::Decimal(5)),
    );
    let report = check(pre, Expr::Bool(false), vec![Statement::Term(Some(x()))]);
    assert_eq!(
        report.proven.len(),
        1,
        "unreachable path was refuted: {report:?}"
    );
}

#[test]
fn reversed_and_negated_bounds_preserve_valid_proofs() {
    let pre = Expr::UnaryOp(
        briev_verify::ast::UnaryOpKind::Not,
        Box::new(comparison(Op::Ge, Expr::Decimal(0), x())),
    );
    let report = check(
        pre,
        comparison(Op::Gt, term(), Expr::Decimal(0)),
        vec![Statement::Term(Some(x()))],
    );
    assert_eq!(report.proven.len(), 1, "{report:?}");
}

#[test]
#[ignore = "requires real Z3 for relational infeasibility"]
fn contradictory_noninterval_guards_do_not_refute_valid_code() {
    let y = Expr::Identifier("y".into());
    let body = vec![
        Statement::Guarded(
            comparison(Op::Lt, x(), y.clone()),
            vec![Statement::Guarded(
                comparison(Op::Le, y, x()),
                vec![Statement::Term(Some(Expr::Decimal(5)))],
            )],
        ),
        Statement::Term(Some(Expr::Decimal(0))),
    ];
    let report = check(
        Expr::Bool(true),
        comparison(Op::Eq, term(), Expr::Decimal(0)),
        body,
    );
    assert_eq!(report.proven.len(), 1, "{report:?}");
}

#[test]
fn a_false_entry_condition_is_unreachable() {
    let report = check(
        Expr::Bool(false),
        Expr::Bool(false),
        vec![Statement::Term(Some(x()))],
    );
    assert_eq!(report.proven.len(), 1, "{report:?}");
}
