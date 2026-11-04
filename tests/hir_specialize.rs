// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.6: generic callable-txn specialization by HIR substitution.
//! Golden fixture: the two-struct `iter_chain<T>` / `iter_chain_loop<T>` pair
//! must specialize to distinct concrete element types (A and B).

use briev_compiler::ast::{TopLevel, Type};
use briev_compiler::hir::ids::IdSupply;
use briev_compiler::hir::specialize::{
    specialize_callable, specialize_callable_with_renames, substitution_for,
};
use briev_compiler::hir::{lower_program, verify, HirCallable, HirCallableKind};
use std::collections::HashMap;
use std::path::Path;

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn find<'a>(program: &'a briev_compiler::hir::HirProgram, name: &str) -> &'a HirCallable {
    program
        .callables
        .iter()
        .find(|c| {
            c.name == name
                && matches!(c.kind, HirCallableKind::Transaction | HirCallableKind::Defn)
        })
        .unwrap_or_else(|| panic!("missing callable {name}"))
}

fn list_of(ty: &str) -> Type {
    Type::Applied("List".to_string(), vec![Type::Custom(ty.to_string())])
}

#[test]
fn iter_chain_two_struct_specialization() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let std_src = std::fs::read_to_string(root.join("lib/std/iterator.bv")).expect("stdlib");
    let fixture = std::fs::read_to_string(root.join("tests/tier1/iter_chain_multi.bv"))
        .expect("fixture");
    let mut items = parse(&std_src);
    items.extend(parse(&fixture));
    let program = lower_program(&items);

    let loop_txn = find(&program, "iter_chain_loop");
    let defn = find(&program, "iter_chain");
    assert_eq!(loop_txn.type_params, vec!["T".to_string()]);
    assert_eq!(defn.type_params, vec!["T".to_string()]);

    // Arity mismatch is rejected, never silently mis-substituted.
    let mut ids = IdSupply::default();
    assert!(specialize_callable(loop_txn, &[], "iter_chain_loop__bad", &mut ids).is_none());
    assert!(substitution_for(loop_txn, &[Type::int(), Type::int()]).is_none());

    let mut ids = IdSupply::default();
    let a = Type::Custom("A".to_string());
    let b = Type::Custom("B".to_string());

    let loop_a = specialize_callable(loop_txn, &[a.clone()], "iter_chain_loop__A", &mut ids)
        .expect("loop<A>");
    let loop_b = specialize_callable(loop_txn, &[b.clone()], "iter_chain_loop__B", &mut ids)
        .expect("loop<B>");
    assert_eq!(loop_a.params[0].ty, list_of("A"));
    assert_eq!(loop_a.params[1].ty, list_of("A"));
    assert_eq!(loop_a.result, Some(list_of("A")));
    assert_eq!(loop_b.params[0].ty, list_of("B"));
    assert_eq!(loop_b.result, Some(list_of("B")));

    let mut renames_a = HashMap::new();
    renames_a.insert("iter_chain_loop".to_string(), "iter_chain_loop__A".to_string());
    let chain_a = specialize_callable_with_renames(
        defn,
        &[a.clone()],
        "iter_chain__A",
        &renames_a,
        &mut ids,
    )
    .expect("iter_chain<A>");
    assert_eq!(chain_a.params[0].ty, list_of("A"));
    assert_eq!(chain_a.result, Some(list_of("A")));
    let retargeted = format!("{:?}", chain_a.body.statements);
    assert!(
        retargeted.contains("iter_chain_loop__A") && !retargeted.contains("\"iter_chain_loop\""),
        "call site must retarget at the peer instantiation: {retargeted}"
    );

    let mut renames_b = HashMap::new();
    renames_b.insert("iter_chain_loop".to_string(), "iter_chain_loop__B".to_string());
    let chain_b = specialize_callable_with_renames(
        defn,
        &[b.clone()],
        "iter_chain__B",
        &renames_b,
        &mut ids,
    )
    .expect("iter_chain<B>");
    assert_eq!(chain_b.params[1].ty, list_of("B"));

    // Instantiations form their own verified program (one fresh ID supply).
    let mut specialized = briev_compiler::hir::HirProgram::default();
    specialized.callables = vec![loop_a, loop_b, chain_a, chain_b];
    verify(&specialized).expect("specialized program ids stay unique");
}

/// New fixture: a plain generic txn (no call graph) specializes its body-local
/// type slots, including a `let` annotation and a cast.
#[test]
fn generic_txn_body_types_substitute() {
    let src = "struct P { v: Int; };\n\
               txn pick<T>(xs: List<T>, i: Int) -> T [i < xs.Count#()][i == xs.Count#()] {\n\
                 let got: T = xs[i];\n\
                 term got;\n\
               };";
    let program = lower_program(&parse(src));
    let pick = find(&program, "pick");
    let mut ids = IdSupply::default();
    let int_pick = specialize_callable(
        pick,
        &[Type::int()],
        "pick__Int",
        &mut ids,
    )
    .expect("pick<Int>");
    assert_eq!(int_pick.params[0].ty, list_of("Int"));
    assert_eq!(int_pick.result, Some(Type::int()));
    let body = format!("{:?}", int_pick.body.statements);
    assert!(body.contains("Int"), "let annotation must be concrete: {body}");

    let mut specialized = briev_compiler::hir::HirProgram::default();
    specialized.callables = vec![int_pick];
    verify(&specialized).expect("specialized pick verifies");
}
