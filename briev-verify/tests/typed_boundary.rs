// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

use briev_verify::ast::{
    BinaryOpKind, Contract, Definition, Expr, OutputType, Statement, TopLevel, Type,
};
use briev_verify::smt::is_z3_available;
use briev_verify::verify::verify_bodies;

fn binary(op: BinaryOpKind, left: Expr, right: Expr) -> Expr {
    Expr::BinaryOp(op, Box::new(left), Box::new(right))
}

#[test]
#[ignore = "requires real Z3; run explicitly for the typed-verification boundary"]
fn floating_increment_is_not_proven_by_integer_bitvectors() {
    assert!(is_z3_available());
    let x = 16_777_216_f32;
    assert_eq!(
        x + 1.0,
        x,
        "the proposed contract has a rounding counterexample"
    );
    let mut definition = Definition::prover_defn(
        "floating_increment",
        Contract::proven(
            Expr::Bool(true),
            binary(
                BinaryOpKind::Neq,
                Expr::Identifier("term".into()),
                Expr::Identifier("x".into()),
            ),
        ),
        vec![Statement::Term(Some(binary(
            BinaryOpKind::Add,
            Expr::Identifier("x".into()),
            Expr::Decimal(1),
        )))],
    );
    definition.parameters.push(("x".into(), Type::float()));
    definition.output_type = Some(OutputType::single(Type::float()));
    let report = verify_bodies(&[TopLevel::Definition(definition)]);
    assert!(
        report.proven.is_empty(),
        "false floating proof: {:?}",
        report.proven
    );
    assert_eq!(
        report.skipped.len(),
        1,
        "unsupported arithmetic must be explicit: {report:?}"
    );
}

#[test]
fn a_float_local_cannot_acquire_integer_cancellation_guarantees() {
    let definition = Definition::prover_defn(
        "floating_local",
        Contract::proven(
            Expr::Bool(true),
            binary(
                BinaryOpKind::Eq,
                Expr::Identifier("term".into()),
                Expr::Decimal(16_777_216),
            ),
        ),
        vec![
            Statement::Let {
                name: "x".into(),
                ty: Some(Type::float()),
                expr: Some(Expr::Decimal(16_777_216)),
                modifiers: vec![],
                names: vec![],
            },
            Statement::Term(Some(binary(
                BinaryOpKind::Sub,
                binary(
                    BinaryOpKind::Add,
                    Expr::Identifier("x".into()),
                    Expr::Decimal(1),
                ),
                Expr::Decimal(1),
            ))),
        ],
    );
    let x = 16_777_216_f32;
    assert_ne!((x + 1.0) - 1.0, x);
    let report = verify_bodies(&[TopLevel::Definition(definition)]);
    assert!(report.proven.is_empty(), "false floating proof: {report:?}");
    assert_eq!(report.skipped.len(), 1);
}

#[test]
fn a_floating_helper_cannot_launder_a_word_summary() {
    let mut helper = Definition::prover_defn(
        "helper",
        Contract::proven(Expr::Bool(true), Expr::Bool(true)),
        vec![Statement::Term(Some(Expr::Identifier("x".into())))],
    );
    helper.parameters.push(("x".into(), Type::float()));
    let caller = Definition::prover_defn(
        "caller",
        Contract::proven(
            Expr::Bool(true),
            binary(
                BinaryOpKind::Eq,
                Expr::Identifier("term".into()),
                Expr::Decimal(7),
            ),
        ),
        vec![Statement::Term(Some(Expr::Call(
            "helper".into(),
            vec![Expr::Decimal(7)],
            None,
        )))],
    );
    let report = verify_bodies(&[TopLevel::Definition(helper), TopLevel::Definition(caller)]);
    assert!(report.proven.is_empty(), "{report:?}");
    assert_eq!(report.skipped.len(), 1, "{report:?}");
}

#[test]
fn exceptional_division_is_not_certified_by_total_smt_arithmetic() {
    for divisor in [
        Expr::Decimal(0),
        Expr::Decimal(-1),
        Expr::Identifier("d".into()),
    ] {
        let quotient = binary(BinaryOpKind::Div, Expr::Identifier("x".into()), divisor);
        let definition = Definition::prover_defn(
            "divide",
            Contract::proven(
                Expr::Bool(true),
                binary(
                    BinaryOpKind::Eq,
                    Expr::Identifier("term".into()),
                    quotient.clone(),
                ),
            ),
            vec![Statement::Term(Some(quotient))],
        );
        let report = verify_bodies(&[TopLevel::Definition(definition)]);
        assert!(report.proven.is_empty(), "{report:?}");
        assert_eq!(report.skipped.len(), 1, "{report:?}");
    }
}

#[test]
fn resolved_models_accept_only_the_supported_word_width() {
    use briev_verify::domain::ScalarModel;
    use briev_verify::verify::verify_bodies_with_types;
    for bits in [0, 8, 16, 32, 64, 128] {
        let mut definition = Definition::prover_defn(
            "identity",
            Contract::proven(
                Expr::Bool(true),
                binary(
                    BinaryOpKind::Eq,
                    Expr::Identifier("term".into()),
                    Expr::Identifier("x".into()),
                ),
            ),
            vec![Statement::Term(Some(Expr::Identifier("x".into())))],
        );
        definition
            .parameters
            .push(("x".into(), Type::Custom("UserWord".into())));
        let report = verify_bodies_with_types(&[TopLevel::Definition(definition)], &|_| {
            ScalarModel::SignedWord { bits }
        });
        assert_eq!(report.proven.len(), usize::from(bits == 64), "{report:?}");
        assert_eq!(report.skipped.len(), usize::from(bits != 64), "{report:?}");
    }
}

#[test]
fn a_global_float_cannot_be_treated_as_a_free_word() {
    let global = TopLevel::StateDecl(briev_verify::ast::StateDecl {
        name: "floating_state".into(),
        ty: Type::float(),
        span: None,
    });
    let value = Expr::Identifier("floating_state".into());
    let definition = Definition::prover_defn(
        "global",
        Contract::proven(
            Expr::Bool(true),
            binary(
                BinaryOpKind::Eq,
                Expr::Identifier("term".into()),
                value.clone(),
            ),
        ),
        vec![Statement::Term(Some(value))],
    );
    let report = verify_bodies(&[global, TopLevel::Definition(definition)]);
    assert!(report.proven.is_empty(), "{report:?}");
    assert_eq!(report.skipped.len(), 1);
}
