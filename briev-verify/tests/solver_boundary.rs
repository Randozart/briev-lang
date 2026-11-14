// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

use briev_verify::ast::{BinaryOpKind, Contract, Definition, Expr, Statement, TopLevel, Type};
use briev_verify::smt::{SmtResult, is_z3_available, prove_smt, prove_smt_formula};
use briev_verify::verify::verify_bodies;

#[test]
#[ignore = "requires a working Z3 executable; run explicitly for the solver-boundary gate"]
fn solver_status_is_not_taken_from_model_or_error_text() {
    assert!(is_z3_available(), "solver-boundary gate requires Z3");
    let sat = "(set-option :produce-models true)\n\
        (declare-const unsat (_ BitVec 64))\n\
        (assert (= unsat #x0000000000000001))\n\
        (check-sat)\n(get-model)\n";
    assert!(matches!(prove_smt_formula(sat, 1000), SmtResult::Sat(_)));

    // A failed assertion followed by UNSAT must not certify the rejected query.
    let invalid = "(assert unsat)\n(assert false)\n(check-sat)\n";
    assert!(matches!(
        prove_smt_formula(invalid, 1000),
        SmtResult::Unknown
    ));
    assert!(matches!(
        prove_smt(&Expr::Identifier("unsat".into()), &[]),
        SmtResult::Unknown
    ));

    // Existing VC queries request a model even when UNSAT has no model.
    let unsat = "(assert false)\n(check-sat)\n(get-model)\n";
    assert!(matches!(prove_smt_formula(unsat, 1000), SmtResult::Unsat));
}

#[test]
#[ignore = "requires a working Z3 executable; run explicitly for the solver-boundary gate"]
fn a_sat_model_named_unsat_cannot_prove_a_false_body_contract() {
    assert!(is_z3_available(), "solver-boundary gate requires Z3");
    // defn square(unsat: Int) [true][term == 1] { term unsat * unsat; }
    // The contract is false at unsat = 0. Parameter-dependent counterexamples
    // are advisory in the existing API, but must never be marked proven.
    let post = Expr::BinaryOp(
        BinaryOpKind::Eq,
        Box::new(Expr::Identifier("term".into())),
        Box::new(Expr::Decimal(1)),
    );
    let square = Expr::BinaryOp(
        BinaryOpKind::Mul,
        Box::new(Expr::Identifier("unsat".into())),
        Box::new(Expr::Identifier("unsat".into())),
    );
    let mut definition = Definition::prover_defn(
        "square",
        Contract::proven(Expr::Bool(true), post),
        vec![Statement::Term(Some(square))],
    );
    definition.parameters.push(("unsat".into(), Type::int()));
    let report = verify_bodies(&[TopLevel::Definition(definition)]);
    assert!(report.proven.is_empty(), "false proof: {:?}", report.proven);
    assert_eq!(report.unproven.len(), 1);
    assert!(report.skipped.is_empty());
}

#[test]
#[ignore = "requires a working Z3 executable; run explicitly for the solver-boundary gate"]
fn wrapping_addition_cannot_prove_unbounded_monotonicity() {
    assert!(is_z3_available(), "solver-boundary gate requires Z3");
    // x + 1 > x fails at MAX. This must stay unproven despite holding over Int.
    let x = Expr::Identifier("x".into());
    let mut definition = Definition::prover_defn(
        "increment",
        Contract::proven(
            Expr::Bool(true),
            Expr::BinaryOp(
                BinaryOpKind::Gt,
                Box::new(Expr::Identifier("term".into())),
                Box::new(x.clone()),
            ),
        ),
        vec![Statement::Term(Some(Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(x),
            Box::new(Expr::Decimal(1)),
        )))],
    );
    definition.parameters.push(("x".into(), Type::int()));
    let report = verify_bodies(&[TopLevel::Definition(definition)]);
    assert!(report.proven.is_empty(), "false proof: {:?}", report.proven);
    assert_eq!(report.unproven.len(), 1);
}
