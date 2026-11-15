// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
use briev_verify::ast::{BinaryOpKind as Op, Expr, Type, UnaryOpKind};
use briev_verify::smt::{
    SmtResult, build_contract_query, build_vc_query, is_z3_available, prove_smt, prove_smt_formula,
};

fn eq(left: Expr, right: Expr) -> Expr {
    Expr::BinaryOp(Op::Eq, Box::new(left), Box::new(right))
}
fn id(name: &str) -> Expr {
    Expr::Identifier(name.into())
}

#[test]
#[ignore = "requires real Z3; exercised by the encoding gate"]
fn unsupported_forms_cannot_become_proofs() {
    assert!(is_z3_available());
    for post in [
        Expr::If(
            Box::new(Expr::Bool(true)),
            Box::new(Expr::Bool(false)),
            Some(Box::new(Expr::Bool(false))),
        ),
        eq(
            Expr::UnaryOp(UnaryOpKind::BitNot, Box::new(Expr::Decimal(0))),
            Expr::Decimal(0),
        ),
    ] {
        assert!(matches!(
            prove_smt_formula(&build_vc_query(&[], &post), 1000),
            SmtResult::Unknown
        ));
        assert!(matches!(
            prove_smt_formula(&build_contract_query(&post, &[]), 1000),
            SmtResult::Unknown
        ));
        assert!(matches!(prove_smt(&post, &[]), SmtResult::Unknown));
    }
}

#[test]
#[ignore = "requires real Z3; exercised by the encoding gate"]
fn distinct_names_and_self_aliases_have_consistent_declarations() {
    assert!(is_z3_available());
    for (a, b) in [
        ("#Foo", "hash_Foo"),
        ("#Self", "self_var"),
        ("#Self", "b_self"),
        ("α", "v_ceb1"),
    ] {
        assert!(
            matches!(
                prove_smt_formula(&build_vc_query(&[], &eq(id(a), id(b))), 1000),
                SmtResult::Sat(_)
            ),
            "{a} / {b}"
        );
    }
    assert!(matches!(
        prove_smt_formula(&build_vc_query(&[], &eq(id("#Self"), id("#self"))), 1000),
        SmtResult::Unsat
    ));
    assert!(matches!(
        prove_smt_formula(
            &build_contract_query(
                &eq(id("#Self"), id("#self")),
                &[("#Self".into(), Type::int()), ("#self".into(), Type::int())]
            ),
            1000
        ),
        SmtResult::Unsat
    ));
}

#[test]
#[ignore = "requires real Z3; exercised by the encoding gate"]
fn sort_mismatches_fail_closed_and_boolean_equalities_remain_supported() {
    let invalid = eq(Expr::Bool(true), Expr::Decimal(1));
    assert!(matches!(
        prove_smt_formula(&build_vc_query(&[], &invalid), 1000),
        SmtResult::Unknown
    ));
    let valid = eq(Expr::Bool(true), Expr::Bool(true));
    assert!(matches!(
        prove_smt_formula(&build_vc_query(&[], &valid), 1000),
        SmtResult::Unsat
    ));
}

#[test]
#[ignore = "requires real Z3; exercised by the encoding gate"]
fn identifier_contents_cannot_escape_a_declaration() {
    let name = "x) (assert false) ;";
    assert!(matches!(
        prove_smt_formula(&build_vc_query(&[], &eq(id(name), id(name))), 1000),
        SmtResult::Unsat
    ));
}
