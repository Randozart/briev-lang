// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! AST-vs-HIR parity for the generalized destination model (Phase D1).
//!
//! Each case parses one source program, builds the AST plan
//! (`analysis::destination_model::build` over the AST legacy facts), lowers
//! the same items to HIR, builds the HIR plan (`build` over
//! `hir::destination_ownership` facts), and asserts EXACT canonical equality:
//! both the shared `PartialEq` of the plan itself and a canonical string dump
//! (readable diff). Cases: layout decisions, alias/escape handling, nested
//! structures, collections, spawn/storage, enum/object declarations,
//! boundaries, recursive carriers, ineligible/unknown calls, determinism,
//! and the empty program.

use super::build as hir_build;
use crate::analysis::destination_model::{
    BoundaryKind, BoundaryRole, CallableId, ConstructorAgreement, DestinationSinkKind,
    EnumResultRepresentation, GeneralDestinationClass, GeneralResultDestination,
    GeneralizedDestinationOwnershipPlan, OwnershipClass, ProofReason, ProofStatus,
    RecursiveCarrierArgClass, ValueShapeKind,
};
use crate::analysis::destination_ownership::{
    analyze as ast_analyze, EscapeKind, ParamObligation,
};
use crate::ast::TopLevel;
use std::collections::{BTreeMap, BTreeSet};

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = crate::lexer::tokenize(src).expect("tokenize");
    let mut parser = crate::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

/// Parse once, build both plans, assert exact canonical parity. Returns the
/// HIR plan (the AST plan equals it exactly).
fn assert_parity(src: &str) -> GeneralizedDestinationOwnershipPlan {
    let items = parse(src);
    let ast_plan = ast_analyze(&items);
    let ast_generalized = ast_plan.generalized.clone();
    let program = crate::hir::lower_program(&items);
    let hir_legacy = crate::hir::destination_ownership::analyze(&program);
    let hir_generalized = hir_build(&program, &hir_legacy);
    assert_eq!(
        dump(&hir_generalized),
        dump(&ast_generalized),
        "HIR/AST destination-model drift"
    );
    assert!(
        hir_generalized == ast_generalized,
        "HIR/AST destination-model plan inequality"
    );
    hir_generalized
}

// ── canonical dump ─────────────────────────────────────────────────────

fn proof_str(proof: &ProofStatus) -> String {
    match proof {
        ProofStatus::Proven { reasons } => format!(
            "proven({})",
            reasons
                .iter()
                .map(|reason| reason.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ),
        ProofStatus::Conservative { reason } => format!("conservative({})", reason.as_str()),
        ProofStatus::Unknown => "unknown".to_string(),
    }
}

fn type_str(ty: &Option<crate::ast::Type>) -> String {
    ty.as_ref()
        .map(|ty| format!("{ty:?}"))
        .unwrap_or_else(|| "-".to_string())
}

fn value_key(id: &super::SemanticValueId) -> String {
    format!("{id:?}")
}

fn callable_key(id: &CallableId) -> String {
    format!("{:?}", id.namespace) + "/" + &id.name
}

fn dump(plan: &GeneralizedDestinationOwnershipPlan) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    dump_values(&mut out, plan);
    dump_callables(&mut out, plan);
    dump_call_sites(&mut out, plan);
    dump_loops(&mut out, plan);
    dump_boundaries(&mut out, plan);
    dump_enums(&mut out, plan);
    dump_enum_callables(&mut out, plan);
    dump_objects(&mut out, plan);
    dump_object_values(&mut out, plan);
    dump_sinks(&mut out, plan);
    dump_tuples(&mut out, plan);
    dump_recursive_carriers(&mut out, plan);
    dump_loop_engines(&mut out, plan);
    out
}

fn dump_values(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (id, fact) in &plan.values {
        out.insert(
            format!("value/{}", value_key(id)),
            format!(
                "ty={};shape=({:?},{},{:?},{:?},{},{},{});ownership={};dest={};overlap={};proof={}",
                type_str(&fact.ty),
                fact.shape.kind,
                type_str(&Some(fact.shape.ty.clone())),
                fact.shape.payload_words,
                fact.shape.inline_words,
                fact.shape.boxed_fallback,
                fact.shape.recursive,
                fact.shape.external_abi,
                fact.ownership.as_str(),
                fact.destination.as_str(),
                fact.may_overlap,
                proof_str(&fact.proof),
            ),
        );
    }
}

fn dump_callables(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (id, callable) in &plan.callables {
        let parameters = callable
            .parameters
            .iter()
            .map(|parameter| {
                format!(
                    "{}/{}:{}/{}",
                    value_key(&parameter.value),
                    parameter.obligation.as_str(),
                    type_str(&parameter.ty),
                    parameter.requires_copy
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let result = callable
            .result
            .value
            .as_ref()
            .map(value_key)
            .unwrap_or_else(|| "-".to_string());
        out.insert(
            format!("callable/{}", callable_key(id)),
            format!(
                "exported={};recursive={};params=[{}];result={result}:{}/{};mat={};hidden=(state={},plain={},enum_sret={},tail={},obj_sret={});proof={}",
                callable.exported,
                callable.direct_recursive,
                parameters,
                callable.result.destination.as_str(),
                type_str(&callable.result.ty),
                callable.result.requires_materialization,
                callable.hidden_abi.state,
                callable.hidden_abi.plain,
                callable.hidden_abi.enum_result_destination,
                callable.hidden_abi.tail_carriers,
                callable.hidden_abi.object_result_destination,
                proof_str(&callable.proof),
            ),
        );
    }
}

fn dump_call_sites(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (site, call) in &plan.call_sites {
        let arguments = call
            .arguments
            .iter()
            .map(|argument| {
                format!(
                    "{}:{}/{}",
                    value_key(&argument.value),
                    argument.obligation.as_str(),
                    argument.requires_copy
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let result = call
            .result
            .as_ref()
            .map(|result| {
                format!(
                    "{}/{}:{}",
                    value_key(&result.value),
                    result.class.as_str(),
                    result.requires_copy
                )
            })
            .unwrap_or_else(|| "-".to_string());
        out.insert(
            format!("callsite/{}/{}", site.key(), callable_key(&call.callee)),
            format!(
                "args=[{arguments}];result={result};overlap={};proof={}",
                call.may_overlap_inputs,
                proof_str(&call.proof),
            ),
        );
    }
}

fn dump_loops(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (id, loop_plan) in &plan.loops {
        let carriers = loop_plan
            .carrier_values
            .iter()
            .map(|carrier| {
                format!(
                    "{}/{}:{}/{}/{}:{}",
                    value_key(&carrier.value),
                    carrier.carrier.path,
                    carrier.source_name.as_deref().unwrap_or("-"),
                    carrier.site,
                    carrier.class.as_str(),
                    carrier.requires_backedge_copy,
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        out.insert(
            format!("loop/{}/{}", loop_plan.loop_id.owner, loop_plan.loop_id.path),
            format!(
                "carriers=[{carriers}];iter={};proof={}",
                loop_plan
                    .iteration_local_values
                    .iter()
                    .map(value_key)
                    .collect::<Vec<_>>()
                    .join(","),
                proof_str(&loop_plan.proof),
            ),
        );
    }
}

fn dump_boundaries(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (id, boundary) in &plan.boundaries {
        let values = boundary
            .values
            .iter()
            .map(|value| {
                format!(
                    "{:?}:{}/{}/{}->{}/{:?}",
                    value.role,
                    value.name,
                    type_str(&value.ty),
                    value.internal_class.as_str(),
                    value.external_class.as_str(),
                    value.conversion,
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        out.insert(
            format!(
                "boundary/{}/{}:{:?}",
                boundary.boundary.owner, boundary.boundary.path, boundary.kind
            ),
            format!(
                "dir={:?};{}->{}/{};ownership={};values=[{values}];proof={}",
                boundary.direction,
                boundary.internal_class.as_str(),
                boundary.external_class.as_str(),
                boundary.conversion.as_str(),
                boundary.ownership.as_str(),
                proof_str(&boundary.proof),
            ),
        );
    }
}

fn dump_enums(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (name, enum_plan) in &plan.enums {
        let variants = enum_plan
            .variants
            .iter()
            .map(|variant| {
                format!(
                    "{}={}",
                    variant.name,
                    variant
                        .payload
                        .as_ref()
                        .map(|payload| format!("{payload:?}"))
                        .unwrap_or_else(|| "-".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        out.insert(
            format!("enum/{name}"),
            format!(
                "params=[{}];variants=[{variants}];order={}",
                enum_plan.type_params.join(","),
                enum_plan.declaration_order,
            ),
        );
    }
}

fn dump_enum_callables(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (id, callable) in &plan.enum_callables {
        out.insert(
            format!("enum-callable/{}", callable_key(id)),
            format!(
                "ty={};repr={:?};agreement={:?};hidden={};proof={}",
                type_str(&callable.result_type),
                callable.representation,
                callable.agreement,
                callable.hidden_destination,
                proof_str(&callable.proof),
            ),
        );
    }
}

fn dump_objects(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (name, object) in &plan.objects {
        let fields = object
            .fields
            .iter()
            .map(|field| format!("{}={:?}", field.name, field.ty))
            .collect::<Vec<_>>()
            .join(",");
        out.insert(
            format!("object/{name}"),
            format!("params=[{}];fields=[{fields}]", object.type_params.join(",")),
        );
    }
}

fn dump_object_values(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (id, value) in &plan.object_values {
        out.insert(
            format!("object-value/{}", value_key(id)),
            format!(
                "ty={:?};object={};class={};escapes={:?};mat={};proof={}",
                value.ty,
                value.object,
                value.class.as_str(),
                value.escapes,
                value.requires_materialization,
                proof_str(&value.proof),
            ),
        );
    }
}

fn dump_sinks(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (id, sink) in &plan.sinks {
        out.insert(
            format!("sink/{}/{}", sink.destination.owner, sink.destination.path),
            format!(
                "value={};kind={};class={};copy={};proof={}",
                value_key(&sink.value),
                sink.kind.as_str(),
                sink.class.as_str(),
                sink.requires_copy,
                proof_str(&sink.proof),
            ),
        );
    }
}

fn dump_tuples(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (id, tuple) in &plan.tuples {
        out.insert(
            format!("tuple/{}/{}", tuple.destination.owner, tuple.destination.path),
            format!(
                "value={};elements={};class={};mat={};proof={}",
                value_key(&tuple.value),
                tuple.element_count,
                tuple.class.as_str(),
                tuple.requires_materialization,
                proof_str(&tuple.proof),
            ),
        );
    }
}

fn dump_recursive_carriers(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (id, carrier_plan) in &plan.recursive_carriers {
        let carriers = carrier_plan
            .carriers
            .iter()
            .map(|carrier| {
                format!(
                    "{}:{}:{}/{}/{}/{:?}/{}:{}",
                    carrier.param_index,
                    carrier.param_name,
                    carrier.result_local,
                    carrier.sret_callee,
                    value_key(&carrier.parameter),
                    carrier.arg_classes,
                    carrier.class.as_str(),
                    carrier.requires_backedge_copy,
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        out.insert(
            format!("carrier/{}", callable_key(id)),
            format!("carriers=[{carriers}];proof={}", proof_str(&carrier_plan.proof)),
        );
    }
}

fn dump_loop_engines(out: &mut BTreeMap<String, String>, plan: &GeneralizedDestinationOwnershipPlan) {
    for (node, engine) in &plan.loop_engines {
        let fields = engine
            .fields
            .iter()
            .map(|(name, field)| {
                format!(
                    "{}={:?}/{:?}",
                    name, field.storage, field.representation
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        out.insert(
            format!("loop-engine/{node}"),
            format!("counter={};fields=[{fields}];proof={}", engine.counter, proof_str(&engine.proof)),
        );
    }
}

// ── parity cases ───────────────────────────────────────────────────────

#[test]
fn empty_program_matches() {
    let plan = assert_parity("");
    assert!(plan.values.is_empty());
    assert!(plan.callables.is_empty());
    assert!(plan.call_sites.is_empty());
    assert!(plan.loops.is_empty());
    assert!(plan.boundaries.is_empty());
    assert!(plan.enums.is_empty());
    assert!(plan.objects.is_empty());
    assert!(plan.sinks.is_empty());
    assert!(plan.tuples.is_empty());
    assert!(plan.recursive_carriers.is_empty());
    assert!(plan.loop_engines.is_empty());
}

#[test]
fn callable_parameters_and_results_match() {
    let plan = assert_parity("defn identity(value: Int) -> Int { term value; };\n");
    let callable = plan.callables.values().next().expect("callable plan");
    assert_eq!(callable.parameters.len(), 1);
    assert_eq!(
        callable.result.destination,
        GeneralResultDestination::SsaValue
    );
    let parameter = &callable.parameters[0];
    assert_eq!(parameter.obligation, ParamObligation::Value);
    assert!(parameter.ty.is_some());
    assert!(plan.values.contains_key(&parameter.value));
    assert!(plan
        .values
        .contains_key(callable.result.value.as_ref().expect("result value")));
}

#[test]
fn unknown_calls_are_conservative() {
    let plan = assert_parity(
        r#"
defn call_unknown() -> Int {
    let value: Int = unknown_fn(1);
    term value;
};
"#,
    );
    assert!(plan.conservative_count() > 0);
    let call = plan.call_sites.values().next().expect("call site");
    assert!(matches!(call.proof, ProofStatus::Conservative { .. }));
    let result = call.result.as_ref().expect("result use");
    assert!(plan.values.contains_key(&result.value));
    assert!(plan.values.contains_key(&call.arguments[0].value));
}

#[test]
fn loop_carried_assignments_create_loop_plans() {
    let plan = assert_parity(
        r#"
defn total(n: Int) -> Int {
    let sum: Int = 0;
    foreach i in 0..n {
        sum = sum + i;
    };
    term sum;
};
"#,
    );
    assert!(!plan.loops.is_empty());
    let loop_plan = plan.loops.values().next().expect("loop plan");
    assert_eq!(loop_plan.carrier_values.len(), 1);
    let scalar = &loop_plan.carrier_values[0];
    assert_eq!(scalar.source_name.as_deref(), Some("sum"));
    assert_eq!(scalar.class, GeneralDestinationClass::ScalarValue);
    assert!(!scalar.requires_backedge_copy);
    assert!(matches!(scalar.proof, ProofStatus::Proven { .. }));
    assert!(matches!(loop_plan.proof, ProofStatus::Proven { .. }));
}

#[test]
fn aggregate_loop_carriers_keep_explicit_backedge_copy() {
    let plan = assert_parity(
        r#"
obj P { x: Int; y: Int; };
defn run(n: Int) -> P {
    let acc: P = P { x: 0; y: 0 };
    foreach i in 0..n {
        acc = P { x: i; y: acc.x };
    };
    term acc.x;
};
"#,
    );
    let loop_plan = plan.loops.values().next().expect("aggregate carrier");
    assert!(loop_plan
        .carrier_values
        .iter()
        .all(|carrier| carrier.class == GeneralDestinationClass::Boxed
            && carrier.requires_backedge_copy
            && matches!(carrier.proof, ProofStatus::Proven { .. })));
    let obligations = plan.foreach_carrier_obligations("defn:run");
    assert!(!obligations.is_empty());
    assert!(obligations.values().all(|obligation| {
        obligation.class == GeneralDestinationClass::Boxed
            && obligation.requires_backedge_copy
            && matches!(obligation.proof, ProofStatus::Proven { .. })
    }));
}

#[test]
fn foreach_arrow_assignments_are_carriers() {
    let plan = assert_parity(
        r#"
defn drain(n: Int, q: List<Int>) -> Int {
    let sum: Int = 0;
    foreach i in 0..n {
        sum <- i;
    };
    term sum;
};
"#,
    );
    let obligations = plan.foreach_carrier_obligations("defn:drain");
    let obligation = obligations.get("sum").expect("scalar obligation");
    assert_eq!(obligation.class, GeneralDestinationClass::ScalarValue);
    assert!(!obligation.requires_backedge_copy);
    assert!(matches!(obligation.proof, ProofStatus::Proven { .. }));
}

#[test]
fn exported_callables_create_boundary_plans() {
    let plan = assert_parity("export defn add(a: Int, b: Int) -> Int { term a + b; };\n");
    let boundary = plan.boundaries.values().next().expect("boundary plan");
    assert_eq!(boundary.kind, BoundaryKind::Export);
    assert_eq!(boundary.direction, crate::analysis::destination_model::BoundaryDirection::Export);
    assert_eq!(boundary.conversion, crate::analysis::destination_model::BoundaryConversion::None);
    assert_eq!(boundary.ownership, crate::analysis::destination_model::OwnershipClass::Unknown);
    assert_eq!(boundary.values.len(), 3);
    assert!(boundary.values.iter().all(|value| {
        value.internal_class == GeneralDestinationClass::ScalarValue
            && value.external_class == GeneralDestinationClass::ScalarValue
            && value.conversion == crate::analysis::destination_model::BoundaryConversion::None
    }));
    assert_eq!(
        boundary
            .values
            .iter()
            .filter(|value| value.role == BoundaryRole::Result)
            .count(),
        1
    );
}

#[test]
fn every_foreign_binding_has_a_lookupable_boundary() {
    let plan = assert_parity(
        r#"
frgn one(x: Int) -> Int from "libc";
frgn two(s: String) from "libc";
"#,
    );
    let foreign: Vec<_> = plan
        .boundaries
        .values()
        .filter(|boundary| boundary.kind == BoundaryKind::ForeignCall)
        .collect();
    assert_eq!(foreign.len(), 2);
    for boundary in &foreign {
        assert!(
            plan.foreign_boundary(&boundary.boundary.owner).is_some(),
            "missing foreign boundary lookup for {}",
            boundary.boundary.owner
        );
    }
}

#[test]
fn foreign_binding_creates_import_boundary_values() {
    let plan = assert_parity(r#"frgn strlen(s: String) -> Int from "libc";"#);
    let boundary = plan
        .boundaries
        .values()
        .find(|boundary| boundary.kind == BoundaryKind::ForeignCall)
        .expect("foreign boundary");
    assert_eq!(
        boundary.direction,
        crate::analysis::destination_model::BoundaryDirection::Import
    );
    assert_eq!(boundary.values.len(), 2);
    assert_eq!(boundary.values[0].role, BoundaryRole::Parameter);
    assert_eq!(boundary.values[1].role, BoundaryRole::Result);
    assert_eq!(
        boundary.values[1].ownership,
        crate::analysis::destination_model::OwnershipClass::Owned
    );
    assert_eq!(
        boundary.values[1].internal_class,
        GeneralDestinationClass::ScalarValue
    );
    assert_eq!(
        boundary.values[1].conversion,
        crate::analysis::destination_model::BoundaryConversion::None
    );
}

/// The boundary owner is the LINK SYMBOL, not the Briev call name — a
/// renamed foreign (`frgn local : symbol from "c"`) keys on `symbol`.
#[test]
fn renamed_foreign_keys_on_link_symbol() {
    let plan = assert_parity(r#"frgn strlen(s: String) -> Int : c_strlen from "libc";"#);
    let boundary = plan
        .boundaries
        .values()
        .find(|boundary| boundary.kind == BoundaryKind::ForeignCall)
        .expect("foreign boundary");
    assert_eq!(boundary.boundary.owner, "c_strlen");
    assert!(plan.foreign_boundary("c_strlen").is_some());
    assert!(plan.foreign_boundary("strlen").is_none());
}

#[test]
fn production_plan_equals_direct_rebuild() {
    let src = r#"
defn helper(v: Int) -> Int { term v; };
defn caller(v: Int) -> Int { term helper(v); };
"#;
    let items = parse(src);
    let ast_plan = ast_analyze(&items);
    let program = crate::hir::lower_program(&items);
    let hir_legacy = crate::hir::destination_ownership::analyze(&program);
    let hir_generalized = hir_build(&program, &hir_legacy);
    assert_eq!(ast_plan.generalized, hir_generalized);
    assert!(!ast_plan.generalized.call_sites.is_empty());
}

#[test]
fn enum_constructor_return_selects_caller_destination() {
    let plan = assert_parity(
        r#"
enum R { Ok(Int), Err(String) };
defn make(x: Int) -> R { term Ok(x); };
"#,
    );
    let shape = plan.enums.get("R").expect("enum definition");
    assert_eq!(shape.variants.len(), 2);
    assert_eq!(shape.variants[0].name, "Ok");
    let callable = plan
        .enum_callables
        .values()
        .find(|callable| callable.callable.name == "make")
        .expect("enum callable");
    assert_eq!(
        callable.representation,
        EnumResultRepresentation::CallerDestination
    );
    assert_eq!(
        callable.agreement,
        crate::analysis::destination_model::ConstructorAgreement::AllConstructors
    );
    assert!(callable.hidden_destination);
    let result_value = plan
        .callables
        .values()
        .find(|plan| plan.callable.name == "make")
        .and_then(|plan| plan.result.value.clone())
        .expect("result value");
    assert_eq!(plan.values[&result_value].shape.kind, ValueShapeKind::Enum);
}

/// Multi-payload variant slots store a Tuple payload on both sides.
#[test]
fn multi_payload_variant_keeps_tuple_payload() {
    let plan = assert_parity("enum Pair { Two(Int, String) };\n");
    let shape = plan.enums.get("Pair").expect("enum definition");
    assert_eq!(shape.variants.len(), 1);
    assert_eq!(shape.variants[0].name, "Two");
    assert!(shape.variants[0].payload.is_some());
}

#[test]
fn non_constructor_enum_return_stays_boxed() {
    let plan = assert_parity(
        r#"
enum R { Ok(Int), Err(Int) };
defn unsupported(x: Int) -> R { term x; };
"#,
    );
    let callable = plan
        .enum_callables
        .values()
        .find(|callable| callable.callable.name == "unsupported")
        .expect("enum callable");
    assert_eq!(callable.representation, EnumResultRepresentation::Boxed);
    assert_eq!(
        callable.agreement,
        crate::analysis::destination_model::ConstructorAgreement::Unknown
    );
    assert!(!callable.hidden_destination);
}

#[test]
fn exported_enum_return_keeps_public_boxed_abi() {
    let plan = assert_parity(
        r#"
enum R { Ok(Int), Err(Int) };
export defn make(x: Int) -> R { term Ok(x); };
"#,
    );
    let callable = plan
        .enum_callables
        .values()
        .find(|callable| callable.callable.name == "make")
        .expect("enum callable");
    assert_eq!(callable.representation, EnumResultRepresentation::Boxed);
    assert!(!callable.hidden_destination);
}

#[test]
fn enum_hidden_destinations_match_sret_safety() {
    let src = r#"
enum R { A(Int), B(Int) };
defn direct(x: Int) -> R { term A(x); };
defn passthrough(x: Int) -> R { let r: R = direct(x); term r; };
defn other(x: Int) -> R { term B(x); };
"#;
    let items = parse(src);
    let ast_plan = ast_analyze(&items);
    let program = crate::hir::lower_program(&items);
    let hir_legacy = crate::hir::destination_ownership::analyze(&program);
    let hir_generalized = hir_build(&program, &hir_legacy);
    let representation = crate::hir::enum_repr::build_enum_representation_plan(&program);
    let expected: BTreeSet<String> = crate::hir::enum_repr::sret_safe_defns(&program, &representation)
        .into_iter()
        .collect();
    assert_eq!(hir_generalized.enum_hidden_result_callables(), expected);
    // The HIR-derived set must also equal the AST oracle's set.
    let ast_representation = crate::analysis::enum_repr::build_enum_representation_plan(&items);
    let ast_expected: BTreeSet<String> =
        crate::analysis::enum_repr::sret_safe_defns(&items, &ast_representation)
            .into_iter()
            .collect();
    assert_eq!(ast_plan.generalized.enum_hidden_result_callables(), ast_expected);
    assert_eq!(hir_generalized.enum_hidden_result_callables(), ast_expected);
}

#[test]
fn object_definition_and_escape_are_recorded() {
    let plan = assert_parity(
        r#"
obj P { x: Int; y: Int; };
defn make(x: Int) -> P { term P { x: x; y: 0 }; };
"#,
    );
    let object = plan.objects.get("P").expect("object definition");
    assert_eq!(object.fields.len(), 2);
    let value = plan
        .object_values
        .values()
        .find(|value| value.object == "P")
        .expect("object value");
    assert!(value.requires_materialization);
    assert!(value.escapes.contains(&EscapeKind::Returned));
    assert_eq!(value.class, GeneralDestinationClass::Boxed);
}

#[test]
fn local_object_has_non_escaping_proof() {
    let plan = assert_parity(
        r#"
obj P { x: Int; y: Int; };
defn read(x: Int) -> Int {
    let p: P = P { x: x; y: 0 };
    term p.x;
};
"#,
    );
    let value = plan
        .object_values
        .values()
        .find(|value| value.object == "P")
        .expect("object value");
    assert!(!value.requires_materialization);
    assert_eq!(value.class, GeneralDestinationClass::ScalarReplacedObject);
    assert_eq!(
        value.proof,
        ProofStatus::Proven {
            reasons: BTreeSet::from([ProofReason::NonEscaping]),
        }
    );
}

#[test]
fn borrowed_object_argument_is_conservatively_escaping() {
    let plan = assert_parity(
        r#"
obj P { x: Int; y: Int; };
defn consume(p: P) -> Int { term p.x; };
defn caller(x: Int) -> Int {
    let p: P = P { x: x; y: 0 };
    term consume(p);
};
"#,
    );
    let escaped = plan
        .object_values
        .values()
        .find(|value| value.object == "P" && value.requires_materialization)
        .expect("escaping object value");
    assert!(escaped.escapes.contains(&EscapeKind::UnknownCallLifetime));
}

#[test]
fn tuple_element_aliases_tuple_lifetime() {
    let plan = assert_parity(
        r#"
obj P { x: Int; };
defn make(x: Int) -> (P, Int) {
    term (P { x: x }, x);
};
"#,
    );
    let child = plan
        .object_values
        .values()
        .find(|value| value.object == "P")
        .expect("tuple child object");
    assert!(child.requires_materialization);
    assert!(child.escapes.contains(&EscapeKind::TupleLifetime));
    assert!(!plan
        .stack_object_types_by_owner()
        .get("defn:make")
        .is_some_and(|types| types.contains("P")));
}

#[test]
fn collection_element_aliases_collection_lifetime() {
    let plan = assert_parity(
        r#"
obj P { x: Int; };
defn make(x: Int) -> List<P> {
    term [P { x: x }];
};
"#,
    );
    let child = plan
        .object_values
        .values()
        .find(|value| value.object == "P")
        .expect("collection child object");
    assert!(child.requires_materialization);
    assert!(child.escapes.contains(&EscapeKind::CollectionLifetime));
    assert!(!plan
        .stack_object_types_by_owner()
        .get("defn:make")
        .is_some_and(|types| types.contains("P")));
}

#[test]
fn object_address_escape_blocks_frame_image() {
    let plan = assert_parity(
        r#"
obj P { x: Int; };
defn address(x: Int) -> Int {
    let p: P = P { x: x };
    let raw: Int = &p;
    term raw;
};
"#,
    );
    let escaped = plan
        .object_values
        .values()
        .find(|value| {
            value.object == "P" && value.escapes.contains(&EscapeKind::AddressLifetime)
        })
        .expect("addressed object");
    assert!(escaped.requires_materialization);
    assert!(!plan
        .stack_object_types_by_owner()
        .get("defn:address")
        .is_some_and(|types| types.contains("P")));
}

#[test]
fn loop_assigned_parent_field_child_stays_heap_backed() {
    let plan = assert_parity(
        r#"
obj Child { x: Int; };
obj Parent { child: Child; };
defn make(n: Int) -> Parent {
    let p: Parent = Parent { child: Child { x: 0 } };
    foreach i in 0..n {
        p = Parent { child: Child { x: i } };
    };
    term p;
};
"#,
    );
    let child = plan
        .object_values
        .values()
        .find(|value| {
            value.object == "Child" && value.escapes.contains(&EscapeKind::ObjectLifetime)
        })
        .expect("loop-assigned field child");
    assert!(child.requires_materialization);
    assert!(!plan
        .stack_object_types_by_owner()
        .get("defn:make")
        .is_some_and(|types| types.contains("Child")));
}

#[test]
fn object_literal_field_aliases_parent_lifetime() {
    let plan = assert_parity(
        r#"
obj Child { x: Int; };
obj Parent { child: Child; };
defn make(x: Int) -> Parent {
    term Parent { child: Child { x: x } };
};
"#,
    );
    let child = plan
        .object_values
        .values()
        .find(|value| value.object == "Child")
        .expect("child object");
    assert!(child.requires_materialization);
    assert!(child.escapes.contains(&EscapeKind::ObjectLifetime));
    assert!(!plan
        .stack_object_types_by_owner()
        .get("defn:make")
        .is_some_and(|types| types.contains("Child")));
}

#[test]
fn nested_field_store_aliases_receiver_lifetime() {
    // 2026-11-07: assigning a constructed object into a field of a parameter
    // reached through MORE THAN ONE projection (`w.mid.inner`) must record
    // the same ObjectLifetime escape as the depth-1 form.
    let plan = assert_parity(
        r#"
obj Inner { fd: Int; };
obj Mid { a: Int; inner: Inner; b: Int; };
obj Wrap { marker: Int; mid: Mid; };
defn assign(w: Wrap) -> Void {
    w.mid.inner = Inner { fd: 3 };
};
"#,
    );
    let inner = plan
        .object_values
        .values()
        .find(|value| value.object == "Inner")
        .expect("nested-store object");
    assert!(
        inner.requires_materialization,
        "a nested field store must own the constructed object"
    );
    assert!(inner.escapes.contains(&EscapeKind::ObjectLifetime));
    assert!(!plan
        .stack_object_types_by_owner()
        .get("defn:assign")
        .is_some_and(|types| types.contains("Inner")));
}

#[test]
fn stack_object_types_require_all_aliases_non_escaping() {
    let local = assert_parity(
        r#"
obj P { x: Int; y: Int; };
defn read(x: Int) -> Int {
    let p: P = P { x: x; y: 0 };
    term p.x;
};
"#,
    );
    assert!(local
        .stack_object_types_by_owner()
        .get("defn:read")
        .is_some_and(|types| types.contains("P")));

    let escaping = assert_parity(
        r#"
obj P { x: Int; y: Int; };
defn consume(p: P) -> Int { term p.x; };
defn caller(x: Int) -> Int {
    let p: P = P { x: x; y: 0 };
    term consume(p);
};
"#,
    );
    assert!(!escaping
        .stack_object_types_by_owner()
        .get("defn:caller")
        .is_some_and(|types| types.contains("P")));
}

#[test]
fn container_state_and_tuple_destinations_are_recorded() {
    let plan = assert_parity(
        r#"
obj P { x: Int; y: Int; };
defn run(v: Int) -> Int {
    let p: P = P { x: 0; y: 0 };
    let pair: (Int, Int) = (v, 2);
    global_total = v;
    p.x = v;
    slot[0] <- v;
    term v;
};
"#,
    );
    assert!(plan
        .sinks
        .values()
        .any(|sink| sink.kind == DestinationSinkKind::State && sink.requires_copy));
    assert!(plan
        .sinks
        .values()
        .any(|sink| sink.kind == DestinationSinkKind::ObjectField));
    assert!(plan
        .sinks
        .values()
        .any(|sink| sink.kind == DestinationSinkKind::Collection));
    let tuple = plan.tuples.values().next().expect("tuple construction");
    assert_eq!(tuple.element_count, 2);
    assert_eq!(tuple.class, GeneralDestinationClass::PackedMemory);
    assert!(!tuple.requires_materialization);
}

#[test]
fn loop_constructed_objects_are_not_stack_eligible() {
    let plan = assert_parity(
        r#"
obj P { x: Int; y: Int; };
defn run(n: Int) -> Int {
    let sum: Int = 0;
    foreach i in 0..n {
        let p: P = P { x: i; y: 0 };
        sum = p.x;
    };
    term sum;
};
"#,
    );
    assert!(!plan
        .stack_object_types_by_owner()
        .get("defn:run")
        .is_some_and(|types| types.contains("P")));
    assert!(plan
        .object_values
        .values()
        .any(|value| value.escapes.contains(&EscapeKind::LoopLifetime)));
}

#[test]
fn recursive_carriers_unify_tail_carrier_facts() {
    let plan = assert_parity(
        r#"
enum R { Ok(Int), Err(Int) };
enum S { A };
defn step(s: S) -> R { term Ok(1); };
defn loop(n: Int, state: S, sum: Int) -> Int {
    when n <= 0 { term sum; };
    let r: R = step(state);
    term match r {
        Ok(next) => loop(n - 1, next, sum + 1),
        Err(_) => loop(n - 1, state, sum),
    };
};
"#,
    );
    let callable = plan
        .recursive_carriers
        .keys()
        .find(|callable| callable.name == "loop")
        .expect("recursive carrier");
    let recursive = &plan.recursive_carriers[callable];
    assert_eq!(recursive.carriers.len(), 1);
    assert_eq!(recursive.carriers[0].param_index, 1);
    assert_eq!(recursive.carriers[0].param_name, "state");
    assert_eq!(
        recursive.carriers[0].arg_classes,
        vec![
            RecursiveCarrierArgClass::InlineNested,
            RecursiveCarrierArgClass::ExistingCarrier,
        ]
    );
    assert_eq!(
        recursive.carriers[0].class,
        GeneralDestinationClass::StableCarrier
    );
    let callable_plan = &plan.callables[callable];
    assert_eq!(callable_plan.hidden_abi.tail_carriers, 1);
}

#[test]
fn spawn_and_storage_match() {
    let plan = assert_parity(
        r#"
struct Task { v: Int; };
defn make() -> Int {
    let handle = spawn Task(7);
    term 0;
};
"#,
    );
    // Spawn's R1 facts flow in only through the legacy collision rule (a
    // spawn produces no CallSiteFact), so the callable's own conservative
    // result fact is what appears — exact parity is the gate above.
    assert!(!plan.values.is_empty());
}

/// A parented type (`type T: Parent { fields }`) is never a plain object —
/// the HIR `parent` field carries the guard fact the AST reads from
/// `TypeDef.parent`.
#[test]
fn parented_typedef_is_not_an_object() {
    let plan = assert_parity(
        r#"
type Special: Base { x: Int; };
obj Plain { y: Int; };
"#,
    );
    assert!(plan.objects.get("Special").is_none());
    assert!(plan.objects.get("Plain").is_some());
}

#[test]
fn generalized_plan_is_deterministic() {
    let src = r#"
defn helper(v: Int) -> Int { term v; };
defn caller(v: Int) -> Int { term helper(v); };
"#;
    let items = parse(src);
    let ast_first = ast_analyze(&items).generalized;
    let ast_second = ast_analyze(&parse(src)).generalized;
    assert_eq!(ast_first, ast_second);
    let program = crate::hir::lower_program(&items);
    let hir_first = hir_build(&program, &crate::hir::destination_ownership::analyze(&program));
    let program2 = crate::hir::lower_program(&parse(src));
    let hir_second =
        hir_build(&program2, &crate::hir::destination_ownership::analyze(&program2));
    assert_eq!(hir_first, hir_second);
    assert_eq!(hir_first, ast_first);
}

/// Declaration-order parity: two enums separated by other top-level items
/// keep their source positions (the `HirTypeDef.declaration_order` field
/// carries the AST item index).
#[test]
fn enum_declaration_order_matches_source_positions() {
    let plan = assert_parity(
        r#"
enum First { A };
defn between(x: Int) -> Int { term x; };
enum Second { B };
obj Last { z: Int; };
"#,
    );
    let first = plan.enums.get("First").expect("First");
    let second = plan.enums.get("Second").expect("Second");
    assert!(first.declaration_order < second.declaration_order);
    assert_eq!(second.declaration_order, 2);
}
