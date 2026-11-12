// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! AST-vs-HIR parity for the destination ownership analysis (Phase R1).
//!
//! Each case parses one source program, runs the AST analyzer, lowers the
//! same items to HIR, runs the HIR analyzer, and asserts EXACT equality of
//! the canonical dumps (callables, body facts, call sites, diagnostics).
//! Cases: simple destination proofs, aliases, nested fields, collections,
//! ineligible/unknown calls, loop carriers, members/exports/recursion,
//! control-flow results, spawn/await, the `endprogram` quirk, determinism,
//! and the empty program.

use super::{analyze as hir_analyze, parity_dump, HirDestinationOwnershipPlan};
use crate::analysis::destination_ownership::{
    self as ast, analyze as ast_analyze, DestinationClass, DestinationDiagnosticCode, EscapeKind,
    SinkKind, ValueOrigin,
};
use crate::ast::TopLevel;
use std::collections::BTreeMap;

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = crate::lexer::tokenize(src).expect("tokenize");
    let mut parser = crate::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn dump_hir(plan: &HirDestinationOwnershipPlan) -> BTreeMap<String, String> {
    parity_dump::dump(
        &plan.callables,
        &plan.callables_by_owner,
        &plan.call_sites,
        &plan.diagnostics,
    )
}

/// Parse once, analyze both ways, assert exact canonical parity.
fn assert_parity(src: &str) -> (BTreeMap<String, String>, HirDestinationOwnershipPlan) {
    let items = parse(src);
    let ast_plan = ast_analyze(&items);
    let ast_dump = ast::canonical_dump(&ast_plan);
    let hir_plan = hir_analyze(&crate::hir::lower_program(&items));
    let hir_dump = dump_hir(&hir_plan);
    assert_eq!(hir_dump, ast_dump, "HIR/AST destination-ownership drift");
    (ast_dump, hir_plan)
}

#[test]
fn empty_program_matches() {
    let (dump, plan) = assert_parity("");
    assert!(dump.is_empty());
    assert!(plan.callables.is_empty());
    assert!(plan.callables_by_owner.is_empty());
    assert!(plan.call_sites.is_empty());
    assert!(plan.diagnostics.is_empty());
}

#[test]
fn simple_destination_proofs_match() {
    let src = "defn identity(value: Int) -> Int { term value; };\n";
    let (dump, plan) = assert_parity(src);
    let facts = plan
        .callables_by_owner
        .get("identity")
        .expect("identity facts");
    assert_eq!(facts.param_values.len(), 1);
    let param = &facts.param_values[0];
    let param_fact = facts.values.get(param).expect("param fact");
    assert_eq!(param_fact.class, DestinationClass::ScalarValue);
    assert_eq!(param_fact.origins, [ValueOrigin::Parameter].into());
    let return_fact = facts
        .values
        .values()
        .find(|fact| fact.sinks.contains_key(&SinkKind::Return))
        .expect("return sink");
    assert_eq!(return_fact.class, DestinationClass::ScalarValue);
    assert!(dump.contains_key("callable/identity"));
}

#[test]
fn aliases_share_semantic_value() {
    let src = "defn aliaser(v: Int) -> Int {\n let alias = v;\n term alias;\n};\n";
    let (_dump, plan) = assert_parity(src);
    let facts = plan.callables_by_owner.get("aliaser").expect("facts");
    let param = facts.param_values.first().expect("param").clone();
    let param_fact = facts.values.get(&param).expect("param fact");
    // Both the alias binding and the return read the SAME value: three uses
    // (initializer read, let transfer, term read).
    assert!(param_fact.uses >= 2, "alias must count uses, got {}", param_fact.uses);
    let alias_fact = facts
        .values
        .values()
        .find(|fact| fact.source_name.as_deref() == Some("alias"))
        .expect("alias binding");
    assert_eq!(
        alias_fact.class,
        DestinationClass::ScalarValue,
        "alias of a scalar stays scalar"
    );
}

#[test]
fn nested_field_stores_escape() {
    let src = r#"
struct Inner { w: Int; };
struct Outer { mid: Inner; };
defn put(p: Outer) {
    p.mid.w = 7;
    p.mid = Inner { w: 8 };
};
"#;
    let (_dump, plan) = assert_parity(src);
    let facts = plan.callables_by_owner.get("put").expect("put facts");
    // The root receiver `p` (a non-primitive parameter, Borrow class) is the
    // escape carrier for BOTH the deep field store and the literal store.
    let param = facts.param_values.first().expect("param").clone();
    let param_fact = facts.values.get(&param).expect("param fact");
    assert_eq!(param_fact.class, DestinationClass::Borrowed);
    assert!(
        param_fact
            .sinks
            .get(&SinkKind::ObjectFieldStore)
            .is_some_and(|sites| sites.len() >= 2),
        "nested-field stores must reach the root receiver: {param_fact:?}"
    );
    let literal_fact = facts
        .values
        .values()
        .find(|fact| fact.origins.contains(&ValueOrigin::Constructor))
        .expect("struct literal value");
    assert_eq!(literal_fact.class, DestinationClass::ScalarReplacedObject);
    assert!(literal_fact.ty.is_some(), "literal carries its type");
}

#[test]
fn collection_tuple_and_method_sinks() {
    let src = r#"
defn collect(buf: Int, i: Int) -> Int {
    let packed = (buf, 1);
    let list = [buf, 2];
    buf[i] <- 3;
    buf.push(4);
    term buf;
};
"#;
    let (_dump, plan) = assert_parity(src);
    let facts = plan.callables_by_owner.get("collect").expect("facts");
    let tuple = facts
        .values
        .values()
        .find(|fact| fact.tuple_element_count == Some(2))
        .expect("tuple value");
    assert_eq!(tuple.class, DestinationClass::PackedMemory);
    assert!(
        facts
            .values
            .values()
            .any(|fact| fact.sinks.contains_key(&SinkKind::TupleStore)),
        "tuple elements get tuple-store sinks"
    );
    let list = facts
        .values
        .values()
        .find(|fact| fact.class == DestinationClass::Boxed && fact.origins.contains(&ValueOrigin::Composite))
        .expect("list value");
    assert!(
        facts
            .values
            .values()
            .any(|fact| fact.sinks.contains_key(&SinkKind::CollectionInsert)),
        "list elements get collection-insert sinks"
    );
    let arrow_value = facts
        .values
        .values()
        .find(|fact| {
            fact.origins.contains(&ValueOrigin::Literal)
                && fact.sinks.contains_key(&SinkKind::CollectionInsert)
        })
        .expect("arrow literal");
    assert!(
        arrow_value
            .sinks
            .get(&SinkKind::CollectionInsert)
            .is_some_and(|sites| {
                sites.iter().any(|site| site.contains("/stmt/") || site.contains("/collection"))
            }),
        "index-target arrow assign records a collection insert"
    );
}

#[test]
fn unknown_call_result_is_ineligible() {
    let src = r#"
defn call_mystery() -> Int {
    let value: Int = Mystery#();
    term value;
};
"#;
    let (_dump, plan) = assert_parity(src);
    let owner = plan
        .callables_by_owner
        .values()
        .find(|facts| {
            facts
                .values
                .values()
                .any(|fact| fact.origins.contains(&ValueOrigin::UnknownCall))
        })
        .expect("calling callable");
    let call_fact = owner
        .values
        .values()
        .find(|fact| fact.origins.contains(&ValueOrigin::UnknownCall))
        .expect("unknown call result");
    assert!(
        matches!(
            call_fact.class,
            DestinationClass::Boxed | DestinationClass::Unknown
        ),
        "unknown call result must use a conservative destination class"
    );
    assert!(owner
        .diagnostics
        .iter()
        .any(|d| d.code == DestinationDiagnosticCode::UnknownCallResult));
}

#[test]
fn loop_carriers_escape() {
    let src = r#"
struct Item { n: Int; };
let saved: Int = 0;
defn loopy(items: Int) {
    foreach(item in items) {
        saved = item;
        let widget = Item { n: 1 };
    };
};
"#;
    let (_dump, plan) = assert_parity(src);
    let facts = plan.callables_by_owner.get("loopy").expect("facts");
    let carrier = facts
        .values
        .values()
        .find(|fact| fact.sinks.contains_key(&SinkKind::LoopCarrier))
        .expect("loop carrier");
    assert_eq!(carrier.origins, [ValueOrigin::Constructor].into());
    assert!(
        carrier
            .sinks
            .get(&SinkKind::LoopCarrier)
            .is_some_and(|sites| sites.iter().any(|site| site.contains("/expr/"))),
        "carrier sink must carry its construction site"
    );
    let item = facts
        .values
        .values()
        .find(|fact| fact.source_name.as_deref() == Some("item"))
        .expect("foreach item value");
    assert_eq!(item.origins, [ValueOrigin::CollectionLoad].into());
    assert!(item.sinks.contains_key(&SinkKind::StateStore));
}

#[test]
fn state_escape_records_copy_point() {
    let src = r#"
let saved: Int = 0;
defn save(value: Int) { saved = value; };
"#;
    let (_dump, plan) = assert_parity(src);
    let facts = plan.callables_by_owner.get("save").expect("save facts");
    let value_fact = facts
        .values
        .values()
        .find(|fact| fact.source_name.as_deref() == Some("value"))
        .expect("parameter value");
    assert!(value_fact.escapes.contains(&EscapeKind::StateLifetime));
    assert!(value_fact
        .sinks
        .get(&SinkKind::StateStore)
        .is_some_and(|sites| sites.iter().any(|site| site.contains("/stmt/"))));
    assert!(
        value_fact
            .copy_points
            .keys()
            .any(|point| point.contains("/stmt/")),
        "state escape must record a copy point"
    );
}

#[test]
fn members_exports_and_recursion_match() {
    let src = r#"
obj Counter() {
    defn bump(v: Int) -> Int { term v; };
};
export defn pub_add(l: Int, r: Int) -> Int { term l; };
defn recur(n: Int) -> Int { term recur(n); };
"#;
    let (_dump, plan) = assert_parity(src);
    let member = plan
        .callables
        .get("Counter::bump")
        .expect("member summary");
    assert_eq!(member.name, "Counter::bump");
    let export = plan.callables.get("pub_add").expect("export summary");
    assert!(export.exported);
    let recur = plan.callables.get("recur").expect("recur summary");
    assert!(recur.direct_recursive);
    let export_facts = plan
        .callables_by_owner
        .get("pub_add")
        .expect("export facts");
    assert!(
        export_facts
            .values
            .values()
            .any(|fact| fact.sinks.contains_key(&SinkKind::ExportReturn)),
        "exported bodies sink with export-return"
    );
    assert!(plan
        .diagnostics
        .iter()
        .all(|d| d.code != DestinationDiagnosticCode::UnknownCallResult
            || export_facts.owner == d.owner));
}

#[test]
fn control_flow_results_match() {
    let src = r#"
defn choose(flag: Bool) -> Int {
    term match flag { true => 1; false => 2; };
};
defn pick(v: Int) -> Int {
    term match v { 1 => 2; _ => 3; };
};
defn blocky() -> Int {
    let inner = { let y = 1; y };
    term inner;
};
defn gated(v: Int) -> Int {
    when v > 0 { term v; };
    term 0;
};
"#;
    let (_dump, plan) = assert_parity(src);
    let choose = plan.callables_by_owner.get("choose").expect("choose facts");
    let if_result = choose
        .values
        .values()
        .find(|fact| fact.origins.contains(&ValueOrigin::MatchResult))
        .expect("if result");
    assert_eq!(if_result.class, DestinationClass::ScalarValue);
    let pick = plan.callables_by_owner.get("pick").expect("pick facts");
    let match_result = pick
        .values
        .values()
        .find(|fact| fact.origins.contains(&ValueOrigin::MatchResult))
        .expect("match result");
    assert_eq!(match_result.class, DestinationClass::ScalarValue);
}

#[test]
fn spawn_and_await_match() {
    let src = r#"
struct Task { v: Int; };
defn make() -> Int {
    let handle = spawn Task(7);
    term 0;
};
"#;
    let (_dump, plan) = assert_parity(src);
    let facts = plan.callables_by_owner.get("make").expect("facts");
    let spawned = facts
        .values
        .values()
        .find(|fact| fact.origins.contains(&ValueOrigin::UnknownCall))
        .expect("spawn result");
    assert_eq!(spawned.class, DestinationClass::Boxed);
    let arg = facts
        .values
        .values()
        .find(|fact| fact.sinks.contains_key(&SinkKind::UnknownCallArgument))
        .expect("spawn argument");
    assert!(
        arg.sinks
            .get(&SinkKind::UnknownCallArgument)
            .is_some_and(|sites| sites.iter().any(|site| site.contains("/spawn"))),
        "spawn args escape at the spawn site"
    );
}

/// AST parity pin: `endprogram expr` classifies as Control and its
/// expression is never walked, so the literal produces NO value fact — the
/// HIR port mirrors this exactly.
#[test]
fn endprogram_expression_is_not_walked() {
    let src = "defn done() { endprogram 1; };\n";
    let (_dump, plan) = assert_parity(src);
    let facts = plan.callables_by_owner.get("done").expect("done facts");
    assert!(
        !facts
            .values
            .values()
            .any(|fact| fact.origins.contains(&ValueOrigin::Literal)),
        "endprogram's expression must not produce a fact (AST parity)"
    );
}

#[test]
fn plans_are_deterministic_for_same_source() {
    let src = r#"
defn helper(v: Int) -> Int { term v; };
defn caller(v: Int) -> Int { term helper(v); };
"#;
    let (first, _) = assert_parity(src);
    let items = parse(src);
    let second_ast = ast::canonical_dump(&ast_analyze(&items));
    assert_eq!(first, second_ast);
    let second_hir = dump_hir(&hir_analyze(&crate::hir::lower_program(&items)));
    assert_eq!(first, second_hir);
}


