// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-12 (typed-HIR port): generalized destination model over `HirProgram`.
//!
//! Faithful port of `analysis::destination_model` (Phase D1,
//! docs/plans/2026-10-30-generalized-value-destination-ownership.md) onto
//! typed HIR. Every plan, layout decision, and declaration walk reads HIR
//! callables/type-defs/foreigns — no `hir::to_ast` crossing. The plan schema
//! (`GeneralizedDestinationOwnershipPlan` and every sub-record) is AST-free
//! apart from `ast::Type`, which HIR uses natively, so the types are REUSED
//! from `analysis::destination_model` and a later consumer flip is a
//! type-identical swap.
//!
//! Coverage notes (mirrors the AST builder exactly, quirks included):
//! - Callable type shapes seed from the legacy summaries, then HIR callables
//!   override. The AST matches only top-level `Definition`/`Transaction`/
//!   `CompileTimeDefn`/`CompileTimeTxn`/`Export` items plus the (for
//!   top-level iteration unreachable) `TypeDefOperator` arm; wrapper-exported
//!   callables flatten to the same namespaces, and `isr`/`impl`/trait-default/
//!   `sync<>` callables are skipped exactly like the missing AST arms.
//!   Member callables keep the AST quirk: their shapes are stored under the
//!   BARE name (never looked up — legacy member keys are owner-qualified), so
//!   members resolve to the empty default shapes.
//! - Result shapes come from the declared output SHAPE only (the AST builder
//!   never falls back to the multi-output `outputs` list).
//! - Enum/object definitions read `type_defs`: every `enum`/`obj` declaration
//!   parses as a TypeDef, so the HIR `Enum`/`Struct` kinds cover the AST
//!   `TopLevel::Enum`/`TypeDef`/`StaticStruct` arms. The object guard is
//!   coll/parent/protocol (a parented type is never a plain object);
//!   `StaticStruct` lowers with all three unset. `TopLevel::Obj` has no AST
//!   arm, so HIR `Object`-kind defs are skipped for parity. Mixed
//!   variant-slot typedefs filter `__variant_` fields like the AST.
//! - Foreign boundaries key on the LINK SYMBOL (the AST
//!   `ForeignBinding.foreign_name`), carried by `HirForeign.link_symbol`;
//!   the success-output value name is the parser's empty string, like AST.
//! - `attach_loop_engine_plans` is NOT duplicated: its inputs
//!   (`analysis::loop_shape::LoopShape`, `FieldClass`, this plan type) are
//!   already HIR-shared types, so the backend call site keeps using the
//!   single implementation in `analysis::destination_model`.
//! - The sret-safe set comes from `hir::enum_repr::sret_safe_defns` (HIR
//!   mirror added with this port); recursive carriers from
//!   `hir::tail_carrier::build_recursive_carriers`.
//!
//! Undo: delete this module, its `pub mod` line in `src/hir/mod.rs`, and the
//! `sret_safe_defns` mirror in `src/hir/enum_repr.rs`; no other code depends
//! on it (the backend still consumes the AST-built `generalized` field).

use super::program::{HirCallable, HirCallableKind, HirEnumVariant, HirProgram, HirTypeDef, HirTypeDefKind};
use crate::analysis::destination_model::{
    ArgumentPlan, BoundaryConversion, BoundaryDirection, BoundaryKind, BoundaryPlan, BoundaryRole,
    BoundaryValuePlan, CallableId, CallableNamespace, CallablePlan, CallPlan, ConstructorAgreement,
    DestinationId, DestinationSinkKind, DestinationSinkPlan, EnumCallablePlan,
    EnumDefinitionPlan, EnumResultRepresentation, EnumVariantPlan, FallbackReason,
    GeneralDestinationClass, GeneralResultDestination, GeneralValueFact,
    GeneralizedDestinationOwnershipPlan, HiddenAbiPlan, LoopPlan, LoopValuePlan,
    ObjectDefinitionPlan, ObjectFieldPlan, ObjectValuePlan, OwnershipClass, ParameterPlan,
    ProgramPoint, ProofReason, ProofStatus, RecursiveCarrierArgClass, RecursiveCarrierPlan,
    ResultPlan, ResultUsePlan, SemanticValueId, SemanticValueKind, TupleDestinationPlan,
    ValueShape, ValueShapeKind,
};
use crate::analysis::destination_ownership::{
    CallableKind, CallableSummary, DestinationClass as LegacyDestinationClass, EscapeKind,
    ParamObligation, ResultDestination, SinkKind, ValueOrigin,
};
use crate::analysis::enum_repr::EnumRepresentation;
use crate::ast::Type;
use super::destination_ownership::HirDestinationOwnershipPlan;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

/// Build the generalized Phase D1 schema from the Phase R1 facts derived from
/// a HIR program.
///
/// Construction is deliberately conservative (identical to the AST builder):
/// every generalized value is `Unknown` ownership unless R1 proves a narrower
/// fact, every non-primitive shape is opaque, every loop/boundary set is empty
/// until dedicated walkers are added in later phases.
pub fn build(
    program: &HirProgram,
    legacy: &HirDestinationOwnershipPlan,
) -> GeneralizedDestinationOwnershipPlan {
    let mut plan = GeneralizedDestinationOwnershipPlan::default();
    let shapes = callable_shapes(program, legacy);

    for (name, summary) in &legacy.callables {
        let (callable_plan, facts) = build_callable(name, summary, &shapes);
        plan.values
            .extend(facts.into_iter().map(|fact| (fact.id.clone(), fact)));
        plan.callables
            .insert(callable_plan.callable.clone(), callable_plan);
    }

    for (site_id, call) in &legacy.call_sites {
        let (call_plan, facts) = build_call(site_id, call);
        plan.values
            .extend(facts.into_iter().map(|fact| (fact.id.clone(), fact)));
        plan.call_sites.insert(call_plan.site.clone(), call_plan);
    }

    plan.loops = build_loops(legacy);
    plan.boundaries = build_boundaries(program, legacy, &shapes);
    let loop_facts: Vec<GeneralValueFact> = plan
        .loops
        .values()
        .flat_map(|loop_plan| loop_plan.carrier_values.iter())
        .map(|carrier| {
            general_fact(
                carrier.value.clone(),
                None,
                OwnershipClass::Unknown,
                GeneralDestinationClass::Boxed,
            )
        })
        .collect();
    plan.values
        .extend(loop_facts.into_iter().map(|fact| (fact.id.clone(), fact)));
    plan.enums = build_enum_definitions(program);
    plan.enum_callables = build_enum_callables(program, legacy, &plan.enums);
    mark_enum_value_shapes(&mut plan.values, &plan.enums);
    plan.objects = build_object_definitions(program);
    plan.object_values = build_object_values(legacy, &plan.objects);
    insert_object_facts(&mut plan.values, &plan.object_values);
    mark_object_value_shapes(&mut plan.values, &plan.objects);
    insert_legacy_value_facts(legacy, &mut plan.values);
    plan.sinks = build_sink_plans(legacy);
    plan.tuples = build_tuple_plans(legacy);
    plan.recursive_carriers = super::tail_carrier::build_recursive_carriers(program);
    populate_hidden_tail_carriers(&mut plan.callables, &plan.recursive_carriers);
    plan
}

fn populate_hidden_tail_carriers(
    callables: &mut BTreeMap<CallableId, CallablePlan>,
    carriers: &BTreeMap<CallableId, RecursiveCarrierPlan>,
) {
    for (callable, plan) in carriers {
        if let Some(callable_plan) = callables.get_mut(callable) {
            callable_plan.hidden_abi.tail_carriers = plan.carriers.len() as u64;
        }
    }
}

fn insert_legacy_value_facts(
    legacy: &HirDestinationOwnershipPlan,
    values: &mut BTreeMap<SemanticValueId, GeneralValueFact>,
) {
    let facts = legacy
        .callables_by_owner
        .values()
        .flat_map(|facts| facts.values.values())
        .filter_map(|fact| {
            let id = legacy_semantic_value(fact);
            values.contains_key(&id).then_some(()).map(|()| (id, fact))
        })
        .map(|(id, fact)| {
            general_fact(
                id,
                fact.ty.clone(),
                OwnershipClass::Unknown,
                general_legacy_class(fact.class),
            )
        })
        .collect::<Vec<_>>();
    values.extend(facts.into_iter().map(|fact| (fact.id.clone(), fact)));
}

fn general_legacy_class(class: LegacyDestinationClass) -> GeneralDestinationClass {
    match class {
        LegacyDestinationClass::ScalarValue => GeneralDestinationClass::ScalarValue,
        LegacyDestinationClass::CallerDestination => GeneralDestinationClass::CallerDestination,
        LegacyDestinationClass::ScalarReplacedObject => {
            GeneralDestinationClass::ScalarReplacedObject
        }
        LegacyDestinationClass::StableCarrier => GeneralDestinationClass::StableCarrier,
        LegacyDestinationClass::PackedMemory => GeneralDestinationClass::PackedMemory,
        LegacyDestinationClass::Boxed | LegacyDestinationClass::Unknown => {
            GeneralDestinationClass::Boxed
        }
        LegacyDestinationClass::Borrowed => GeneralDestinationClass::Unknown,
    }
}

fn legacy_semantic_value(fact: &crate::analysis::destination_ownership::ValueFact) -> SemanticValueId {
    semantic_value(
        &CallableId {
            namespace: CallableNamespace::Unknown,
            name: fact.id.owner.clone(),
        },
        SemanticValueKind::Local,
        &fact.id.path,
        0,
    )
}

fn build_sink_plans(
    legacy: &HirDestinationOwnershipPlan,
) -> BTreeMap<DestinationId, DestinationSinkPlan> {
    legacy
        .callables_by_owner
        .values()
        .flat_map(|facts| facts.values.values())
        .flat_map(fact_sink_plans)
        .collect()
}

fn fact_sink_plans(
    fact: &crate::analysis::destination_ownership::ValueFact,
) -> Vec<(DestinationId, DestinationSinkPlan)> {
    fact.sinks
        .iter()
        .filter_map(|(kind, sites)| destination_sink_kind(*kind).map(|kind| (kind, sites)))
        .flat_map(|(kind, sites)| {
            sites.iter().map(move |site| {
                let destination = DestinationId {
                    owner: fact.id.owner.clone(),
                    path: format!("{}/{}/{}", fact.id.path, kind.as_str(), site),
                };
                let plan = DestinationSinkPlan {
                    destination: destination.clone(),
                    value: legacy_semantic_value(fact),
                    kind,
                    class: GeneralDestinationClass::Boxed,
                    requires_copy: true,
                    proof: ProofStatus::Conservative {
                        reason: FallbackReason::UnknownOwnership,
                    },
                };
                (destination, plan)
            })
        })
        .collect()
}

fn destination_sink_kind(kind: SinkKind) -> Option<DestinationSinkKind> {
    match kind {
        SinkKind::StateStore => Some(DestinationSinkKind::State),
        SinkKind::CollectionInsert => Some(DestinationSinkKind::Collection),
        SinkKind::ObjectFieldStore => Some(DestinationSinkKind::ObjectField),
        _ => None,
    }
}

fn build_tuple_plans(
    legacy: &HirDestinationOwnershipPlan,
) -> BTreeMap<DestinationId, TupleDestinationPlan> {
    legacy
        .callables_by_owner
        .values()
        .flat_map(|facts| facts.values.values())
        .filter(|fact| fact.origins.contains(&ValueOrigin::Composite))
        .filter_map(|fact| {
            let element_count = fact.tuple_element_count?;
            let destination = DestinationId {
                owner: fact.id.owner.clone(),
                path: format!("{}/tuple", fact.id.path),
            };
            let class = general_legacy_class(fact.class);
            let plan = TupleDestinationPlan {
                destination: destination.clone(),
                value: legacy_semantic_value(fact),
                element_count,
                class,
                requires_materialization: matches!(
                    class,
                    GeneralDestinationClass::Boxed | GeneralDestinationClass::Unknown
                ),
                proof: ProofStatus::Conservative {
                    reason: FallbackReason::UnknownOwnership,
                },
            };
            Some((destination, plan))
        })
        .collect()
}

fn build_object_definitions(program: &HirProgram) -> BTreeMap<String, ObjectDefinitionPlan> {
    program
        .type_defs
        .iter()
        .filter_map(object_definition)
        .map(|plan| (plan.name.clone(), plan))
        .collect()
}

/// AST `object_definition` mirror: the `TypeDef` arm guards
/// coll/parent/protocol (a parented type is never a plain object) and filters
/// `__variant_` slots; the `StaticStruct` arm lowers with all guards unset,
/// so one guarded match over `Struct`-kind defs serves both. `Object`-kind
/// defs (non-source `TopLevel::Obj`) have no AST arm and are skipped.
fn object_definition(td: &HirTypeDef) -> Option<ObjectDefinitionPlan> {
    let HirTypeDefKind::Struct { fields } = &td.kind else {
        return None;
    };
    if td.is_coll || td.parent.is_some() || td.protocol.is_some() {
        return None;
    }
    let fields: Vec<ObjectFieldPlan> = fields
        .iter()
        .filter(|field| !field.name.starts_with("__variant_"))
        .map(|field| ObjectFieldPlan {
            name: field.name.clone(),
            ty: field.ty.clone(),
        })
        .collect();
    if fields.is_empty() {
        return None;
    }
    Some(ObjectDefinitionPlan {
        name: td.name.clone(),
        type_params: td.type_params.clone(),
        fields,
    })
}

fn build_object_values(
    legacy: &HirDestinationOwnershipPlan,
    objects: &BTreeMap<String, ObjectDefinitionPlan>,
) -> BTreeMap<SemanticValueId, ObjectValuePlan> {
    legacy
        .callables_by_owner
        .values()
        .flat_map(|facts| facts.values.values())
        .filter_map(|fact| object_value(fact, objects))
        .map(|plan| (plan.value.clone(), plan))
        .collect()
}

fn object_value(
    fact: &crate::analysis::destination_ownership::ValueFact,
    objects: &BTreeMap<String, ObjectDefinitionPlan>,
) -> Option<ObjectValuePlan> {
    if fact.class != LegacyDestinationClass::ScalarReplacedObject {
        return None;
    }
    let ty = fact.ty.clone()?;
    let object = object_type_name(&ty)?.to_string();
    if !objects.contains_key(&object) {
        return None;
    }
    let requires_materialization = !fact.escapes.is_empty();
    let class = if requires_materialization {
        GeneralDestinationClass::Boxed
    } else {
        GeneralDestinationClass::ScalarReplacedObject
    };
    let proof = if requires_materialization {
        ProofStatus::Conservative {
            reason: FallbackReason::Escapes,
        }
    } else {
        ProofStatus::Proven {
            reasons: BTreeSet::from([ProofReason::NonEscaping]),
        }
    };
    Some(ObjectValuePlan {
        value: semantic_value(
            &CallableId {
                namespace: CallableNamespace::Unknown,
                name: fact.id.owner.clone(),
            },
            SemanticValueKind::Local,
            &fact.id.path,
            0,
        ),
        ty,
        object,
        class,
        escapes: fact.escapes.clone(),
        requires_materialization,
        proof,
    })
}

fn insert_object_facts(
    values: &mut BTreeMap<SemanticValueId, GeneralValueFact>,
    object_values: &BTreeMap<SemanticValueId, ObjectValuePlan>,
) {
    for object in object_values.values() {
        if values.contains_key(&object.value) {
            continue;
        }
        let fact = general_fact(
            object.value.clone(),
            Some(object.ty.clone()),
            OwnershipClass::Owned,
            object.class,
        );
        values.insert(object.value.clone(), fact);
    }
}

fn mark_object_value_shapes(
    values: &mut BTreeMap<SemanticValueId, GeneralValueFact>,
    objects: &BTreeMap<String, ObjectDefinitionPlan>,
) {
    for fact in values.values_mut() {
        if fact
            .ty
            .as_ref()
            .and_then(object_type_name)
            .is_some_and(|name| objects.contains_key(name))
        {
            fact.shape.kind = ValueShapeKind::Object;
        }
    }
}

fn object_type_name(ty: &Type) -> Option<&str> {
    match ty {
        Type::Custom(name) | Type::Applied(name, _) => Some(name.as_str()),
        _ => None,
    }
}

fn build_enum_definitions(program: &HirProgram) -> BTreeMap<String, EnumDefinitionPlan> {
    program
        .type_defs
        .iter()
        .filter_map(enum_definition)
        .map(|plan| (plan.name.clone(), plan))
        .collect()
}

/// AST `enum_definition` mirror over `Enum`-kind type defs. Every `enum`
/// declaration parses as a TypeDef with `__variant_` slots, so this one arm
/// covers the AST `TopLevel::Enum` + `TypeDef` + `Export` unwrap arms.
fn enum_definition(td: &HirTypeDef) -> Option<EnumDefinitionPlan> {
    let HirTypeDefKind::Enum { variants } = &td.kind else {
        return None;
    };
    Some(EnumDefinitionPlan {
        name: td.name.clone(),
        type_params: td.type_params.clone(),
        variants: variants.iter().map(hir_enum_variant_plan).collect(),
        declaration_order: td.declaration_order,
    })
}

/// Payload rule uniform over HIR variants: a typedef variant slot stores a
/// BARE type for single payloads and a `Tuple` for multi payloads, and the
/// AST applies `enum_payload` to constructor-variant type lists — both reduce
/// to `enum_payload` over the HIR variant's type list.
fn hir_enum_variant_plan(variant: &HirEnumVariant) -> EnumVariantPlan {
    match variant {
        HirEnumVariant::Unit(name) => EnumVariantPlan {
            name: name.clone(),
            payload: None,
        },
        HirEnumVariant::Tuple(name, types) => EnumVariantPlan {
            name: name.clone(),
            payload: enum_payload(types),
        },
        HirEnumVariant::Struct(name, fields) => {
            let types: Vec<Type> = fields.iter().map(|field| field.ty.clone()).collect();
            EnumVariantPlan {
                name: name.clone(),
                payload: enum_payload(&types),
            }
        }
    }
}

fn enum_payload(types: &[Type]) -> Option<Type> {
    match types {
        [] => None,
        [single] => Some(single.clone()),
        _ => Some(Type::Tuple(types.to_vec())),
    }
}

fn build_enum_callables(
    program: &HirProgram,
    legacy: &HirDestinationOwnershipPlan,
    enums: &BTreeMap<String, EnumDefinitionPlan>,
) -> BTreeMap<CallableId, EnumCallablePlan> {
    let representation = super::enum_repr::build_enum_representation_plan(program);
    let sret_safe: BTreeSet<String> =
        super::enum_repr::sret_safe_defns(program, &representation)
            .into_iter()
            .collect();
    let shapes = callable_shapes(program, legacy);
    legacy
        .callables
        .iter()
        .filter_map(|(name, summary)| {
            let callable = callable_id(name, summary.kind);
            let result_type = shapes.get(&callable)?.result.clone()?;
            if !is_enum_type(&result_type, enums) {
                return None;
            }
            let plan = enum_callable_plan(
                &callable,
                &result_type,
                summary,
                representation.defns.get(name).copied(),
                sret_safe.contains(name),
            );
            Some((callable, plan))
        })
        .collect()
}

fn enum_callable_plan(
    callable: &CallableId,
    result_type: &Type,
    summary: &CallableSummary,
    class: Option<EnumRepresentation>,
    sret_safe: bool,
) -> EnumCallablePlan {
    if summary.exported {
        return EnumCallablePlan {
            callable: callable.clone(),
            result_type: Some(result_type.clone()),
            representation: EnumResultRepresentation::Boxed,
            agreement: ConstructorAgreement::Unknown,
            hidden_destination: false,
            proof: ProofStatus::Conservative {
                reason: FallbackReason::ExternalBoundary,
            },
        };
    }
    match class.unwrap_or(EnumRepresentation::Boxed) {
        EnumRepresentation::ReturnedSmallCandidate if sret_safe => EnumCallablePlan {
            callable: callable.clone(),
            result_type: Some(result_type.clone()),
            representation: EnumResultRepresentation::CallerDestination,
            agreement: ConstructorAgreement::AllConstructors,
            hidden_destination: true,
            proof: ProofStatus::Proven {
                reasons: BTreeSet::from([
                    ProofReason::CallerOwnedDestination,
                    ProofReason::LayoutAgreement,
                ]),
            },
        },
        EnumRepresentation::ReturnedSmallCandidate => EnumCallablePlan {
            callable: callable.clone(),
            result_type: Some(result_type.clone()),
            representation: EnumResultRepresentation::Boxed,
            agreement: ConstructorAgreement::AllConstructors,
            hidden_destination: false,
            proof: ProofStatus::Conservative {
                reason: FallbackReason::RecursiveOrCyclic,
            },
        },
        EnumRepresentation::InlineLocalCandidate => EnumCallablePlan {
            callable: callable.clone(),
            result_type: Some(result_type.clone()),
            representation: EnumResultRepresentation::InlineLocal,
            agreement: ConstructorAgreement::Unknown,
            hidden_destination: false,
            proof: ProofStatus::Proven {
                reasons: BTreeSet::from([ProofReason::NonEscaping]),
            },
        },
        EnumRepresentation::Boxed => EnumCallablePlan {
            callable: callable.clone(),
            result_type: Some(result_type.clone()),
            representation: EnumResultRepresentation::Boxed,
            agreement: ConstructorAgreement::Unknown,
            hidden_destination: false,
            proof: ProofStatus::Conservative {
                reason: FallbackReason::UnknownOwnership,
            },
        },
    }
}

fn mark_enum_value_shapes(
    values: &mut BTreeMap<SemanticValueId, GeneralValueFact>,
    enums: &BTreeMap<String, EnumDefinitionPlan>,
) {
    for fact in values.values_mut() {
        if fact.ty.as_ref().is_some_and(|ty| is_enum_type(ty, enums)) {
            fact.shape.kind = ValueShapeKind::Enum;
        }
    }
}

fn is_enum_type(ty: &Type, enums: &BTreeMap<String, EnumDefinitionPlan>) -> bool {
    match ty {
        Type::Custom(name) | Type::Applied(name, _) => enums.contains_key(name),
        Type::Union(_) => true,
        Type::Generic(_, args) => args.iter().any(|ty| is_enum_type(ty, enums)),
        Type::Constrained(inner, _) => is_enum_type(inner.as_ref(), enums),
        _ => false,
    }
}

fn build_callable(
    name: &str,
    summary: &CallableSummary,
    shapes: &BTreeMap<CallableId, CallableTypeShapes>,
) -> (CallablePlan, Vec<GeneralValueFact>) {
    let callable_plan = build_callable_plan(name, summary, shapes);
    let mut facts = Vec::new();
    if let Some(result_value) = callable_plan.result.value.clone() {
        facts.push(general_fact(
            result_value,
            callable_plan.result.ty.clone(),
            OwnershipClass::Unknown,
            GeneralDestinationClass::Unknown,
        ));
    }
    for parameter in &callable_plan.parameters {
        facts.push(general_fact(
            parameter.value.clone(),
            parameter.ty.clone(),
            ownership_for_obligation(parameter.obligation),
            GeneralDestinationClass::Unknown,
        ));
    }
    (callable_plan, facts)
}

fn build_callable_plan(
    name: &str,
    summary: &CallableSummary,
    shapes: &BTreeMap<CallableId, CallableTypeShapes>,
) -> CallablePlan {
    let callable = callable_id(name, summary.kind);
    let type_shapes = shapes.get(&callable);
    let parameters = summary
        .parameters
        .iter()
        .enumerate()
        .map(|(index, obligation)| build_parameter(&callable, index, *obligation, type_shapes))
        .collect();
    let result_type = type_shapes.and_then(|shapes| shapes.result.clone());
    let result_value = result_destination(summary.result)
        .ne(&GeneralResultDestination::Void)
        .then(|| semantic_value(&callable, SemanticValueKind::Temporary, "result", 0));
    let destination = result_destination(summary.result);
    CallablePlan {
        callable,
        exported: summary.exported,
        direct_recursive: summary.direct_recursive,
        parameters,
        result: ResultPlan {
            value: result_value,
            ty: result_type,
            destination,
            requires_materialization: destination == GeneralResultDestination::Boxed,
        },
        hidden_abi: HiddenAbiPlan::default(),
        proof: ProofStatus::Conservative {
            reason: FallbackReason::UnknownOwnership,
        },
    }
}

fn build_parameter(
    callable: &CallableId,
    index: usize,
    obligation: ParamObligation,
    shapes: Option<&CallableTypeShapes>,
) -> ParameterPlan {
    let source_path = format!("param/{index}/{index}");
    let value = semantic_value(callable, SemanticValueKind::Parameter, &source_path, 0);
    let ty = shapes
        .and_then(|shapes| shapes.parameters.get(index))
        .and_then(Option::clone);
    ParameterPlan {
        value,
        ty,
        obligation,
        destination: None,
        requires_copy: true,
    }
}

fn build_call(
    site_id: &crate::analysis::destination_ownership::CallSiteId,
    call: &crate::analysis::destination_ownership::CallSiteFact,
) -> (CallPlan, Vec<GeneralValueFact>) {
    let point = ProgramPoint {
        owner: site_id.owner.clone(),
        statement_path: statement_path_from_site(&site_id.site),
        expression_path: Vec::new(),
    };
    let callee = CallableId {
        namespace: namespace_for_kind(call.kind),
        name: call.callee.clone(),
    };
    let mut facts = Vec::new();
    let arguments = build_argument_plans(call, &mut facts);
    let result = build_result_use(site_id, call, &mut facts);
    let proof = if call.kind == CallableKind::Unknown {
        ProofStatus::Conservative {
            reason: FallbackReason::UnknownCallSummary,
        }
    } else {
        ProofStatus::Unknown
    };
    (
        CallPlan {
            site: point.clone(),
            callee,
            arguments,
            result,
            may_overlap_inputs: call.may_alias_args,
            proof,
        },
        facts,
    )
}

fn build_argument_plans(
    call: &crate::analysis::destination_ownership::CallSiteFact,
    facts: &mut Vec<GeneralValueFact>,
) -> Vec<ArgumentPlan> {
    call.argument_values
        .iter()
        .map(|value| {
            let source_path = format!("arg/{}", value.path);
            let argument_value = semantic_value(
                &CallableId {
                    namespace: CallableNamespace::Unknown,
                    name: value.owner.clone(),
                },
                SemanticValueKind::Temporary,
                &source_path,
                0,
            );
            facts.push(general_fact(
                argument_value.clone(),
                None,
                OwnershipClass::Unknown,
                GeneralDestinationClass::Unknown,
            ));
            ArgumentPlan {
                value: argument_value,
                obligation: ParamObligation::Unknown,
                requires_copy: true,
                may_overlap_callee_destination: true,
            }
        })
        .collect()
}

fn build_result_use(
    site_id: &crate::analysis::destination_ownership::CallSiteId,
    call: &crate::analysis::destination_ownership::CallSiteFact,
    facts: &mut Vec<GeneralValueFact>,
) -> Option<ResultUsePlan> {
    if call.result == ResultDestination::Void {
        return None;
    }
    let value = semantic_value(
        &CallableId {
            namespace: CallableNamespace::Unknown,
            name: site_id.owner.clone(),
        },
        SemanticValueKind::Temporary,
        &format!("{}/result", site_id.site),
        0,
    );
    facts.push(general_fact(
        value.clone(),
        None,
        OwnershipClass::Unknown,
        GeneralDestinationClass::Unknown,
    ));
    Some(ResultUsePlan {
        value,
        destination: None,
        class: general_destination(call.result),
        requires_copy: true,
    })
}

fn build_loops(legacy: &HirDestinationOwnershipPlan) -> BTreeMap<DestinationId, LoopPlan> {
    legacy
        .callables_by_owner
        .iter()
        .filter_map(|(_name, facts)| {
            let owner = facts.owner.as_str();
            let values = loop_values(owner, facts);
            if values.is_empty() {
                return None;
            }
            let loop_id = DestinationId {
                owner: owner.to_string(),
                path: "loop/0".to_string(),
            };
            let proof = loop_plan_proof(&values);
            Some((
                loop_id.clone(),
                LoopPlan {
                    loop_id,
                    carrier_values: values,
                    iteration_local_values: Vec::new(),
                    proof,
                },
            ))
        })
        .collect()
}

fn loop_values(
    owner: &str,
    facts: &crate::analysis::destination_ownership::CallableDestinationFacts,
) -> Vec<LoopValuePlan> {
    facts
        .values
        .values()
        .filter(|fact| fact.escapes.contains(&EscapeKind::LoopLifetime))
        // Loop-carried object temporaries are recorded for stack eligibility,
        // but only named bindings are carrier obligations. Unnamed temporaries
        // belong to the iteration, not the backedge state.
        .filter(|fact| fact.source_name.is_some())
        .map(|fact| LoopValuePlan {
            value: semantic_value(
                &CallableId {
                    namespace: CallableNamespace::Unknown,
                    name: owner.to_string(),
                },
                SemanticValueKind::Local,
                &fact.id.path,
                0,
            ),
            carrier: DestinationId {
                owner: owner.to_string(),
                path: format!("carrier/{}", fact.id.path),
            },
            source_name: fact.source_name.clone(),
            site: fact.id.path.clone(),
            class: carrier_class(fact),
            requires_backedge_copy: carrier_requires_backedge_copy(fact),
            proof: carrier_proof(fact),
        })
        .collect()
}

/// A foreach binding survives in its named frame slot. Scalars need no
/// separate image copy; aggregates and boxed handles do.
fn carrier_class(
    fact: &crate::analysis::destination_ownership::ValueFact,
) -> GeneralDestinationClass {
    match general_legacy_class(fact.class) {
        GeneralDestinationClass::ScalarReplacedObject => GeneralDestinationClass::Boxed,
        other => other,
    }
}

fn carrier_requires_backedge_copy(
    fact: &crate::analysis::destination_ownership::ValueFact,
) -> bool {
    !matches!(
        carrier_class(fact),
        GeneralDestinationClass::ScalarValue | GeneralDestinationClass::StableCarrier
    )
}

fn carrier_proof(fact: &crate::analysis::destination_ownership::ValueFact) -> ProofStatus {
    let known_binding = fact.source_name.is_some()
        && fact.class != crate::analysis::destination_ownership::DestinationClass::Unknown;
    if known_binding {
        ProofStatus::Proven {
            reasons: BTreeSet::from([ProofReason::StableCarrierLifetime]),
        }
    } else {
        ProofStatus::Conservative {
            reason: FallbackReason::UnknownOwnership,
        }
    }
}

fn loop_plan_proof(values: &[LoopValuePlan]) -> ProofStatus {
    if values
        .iter()
        .all(|value| matches!(value.proof, ProofStatus::Proven { .. }))
    {
        ProofStatus::Proven {
            reasons: BTreeSet::from([ProofReason::StableCarrierLifetime]),
        }
    } else {
        ProofStatus::Conservative {
            reason: FallbackReason::UnknownOwnership,
        }
    }
}

fn build_boundaries(
    program: &HirProgram,
    legacy: &HirDestinationOwnershipPlan,
    shapes: &BTreeMap<CallableId, CallableTypeShapes>,
) -> BTreeMap<DestinationId, BoundaryPlan> {
    let exports = export_boundaries(legacy, shapes);
    let imports = foreign_boundaries(program);
    exports.into_iter().chain(imports).collect()
}

fn export_boundaries(
    legacy: &HirDestinationOwnershipPlan,
    shapes: &BTreeMap<CallableId, CallableTypeShapes>,
) -> BTreeMap<DestinationId, BoundaryPlan> {
    legacy
        .callables
        .values()
        .filter(|summary| summary.exported)
        .filter_map(|summary| {
            let callable = callable_id(&summary.name, summary.kind);
            let shape = shapes.get(&callable)?;
            // The frozen export ABI passes every parameter as a scalar or
            // opaque handle, including borrowed object/collection handles.
            let parameter_classes: Vec<GeneralDestinationClass> = summary
                .parameters
                .iter()
                .map(|_| GeneralDestinationClass::ScalarValue)
                .collect();
            let result_class = match general_destination(summary.result) {
                GeneralDestinationClass::Unknown => GeneralDestinationClass::Unknown,
                _ => GeneralDestinationClass::ScalarValue,
            };
            let mut values = boundary_values(
                &shape.parameters,
                &parameter_classes,
                shape.result.as_ref(),
                result_class,
            );
            for value in &mut values {
                value.external_class = GeneralDestinationClass::ScalarValue;
                value.conversion = BoundaryConversion::None;
            }
            let boundary = DestinationId {
                owner: summary.name.clone(),
                path: "boundary/export".to_string(),
            };
            Some((
                boundary.clone(),
                BoundaryPlan {
                    boundary,
                    kind: BoundaryKind::Export,
                    direction: BoundaryDirection::Export,
                    internal_class: result_class,
                    external_class: GeneralDestinationClass::ScalarValue,
                    conversion: BoundaryConversion::None,
                    ownership: OwnershipClass::Unknown,
                    proof: ProofStatus::Conservative {
                        reason: FallbackReason::ExternalBoundary,
                    },
                    values,
                },
            ))
        })
        .collect()
}

/// AST `foreign_boundaries` mirror over `HirForeign`. The boundary owner is
/// the LINK SYMBOL (`HirForeign.link_symbol`), and the success-output value
/// name is the parser's empty string — both exactly like the AST walk.
fn foreign_boundaries(program: &HirProgram) -> BTreeMap<DestinationId, BoundaryPlan> {
    program
        .foreign
        .iter()
        .map(|binding| {
            let mut values: Vec<BoundaryValuePlan> = binding
                .params
                .iter()
                .enumerate()
                .map(|(index, (_, ty))| {
                    boundary_value(
                        BoundaryRole::Parameter,
                        format!("p{index}"),
                        Some(ty.clone()),
                        OwnershipClass::Unknown,
                        GeneralDestinationClass::ScalarValue,
                    )
                })
                .collect();
            if let Some(ty) = &binding.ret {
                let mut value = boundary_value(
                    BoundaryRole::Result,
                    String::new(),
                    Some(ty.clone()),
                    OwnershipClass::Owned,
                    GeneralDestinationClass::ScalarValue,
                );
                // The declared FFI result type IS the ABI contract; no
                // implicit representation change happens at this boundary.
                value.conversion = BoundaryConversion::None;
                values.push(value);
            }
            let internal_class = values
                .iter()
                .find(|value| value.role == BoundaryRole::Result)
                .map(|value| value.internal_class)
                .unwrap_or(GeneralDestinationClass::Unknown);
            let boundary = DestinationId {
                owner: binding.link_symbol.clone(),
                path: "boundary/foreign".to_string(),
            };
            (
                boundary.clone(),
                BoundaryPlan {
                    boundary,
                    kind: BoundaryKind::ForeignCall,
                    direction: BoundaryDirection::Import,
                    internal_class,
                    external_class: GeneralDestinationClass::ScalarValue,
                    conversion: BoundaryConversion::None,
                    ownership: OwnershipClass::Unknown,
                    proof: ProofStatus::Conservative {
                        reason: FallbackReason::ExternalBoundary,
                    },
                    values,
                },
            )
        })
        .collect()
}

fn boundary_values(
    parameters: &[Option<Type>],
    parameter_classes: &[GeneralDestinationClass],
    result: Option<&Type>,
    result_class: GeneralDestinationClass,
) -> Vec<BoundaryValuePlan> {
    let mut values = parameters
        .iter()
        .enumerate()
        .map(|(index, ty)| {
            let class = parameter_classes
                .get(index)
                .copied()
                .unwrap_or(GeneralDestinationClass::Unknown);
            boundary_value(
                BoundaryRole::Parameter,
                format!("p{index}"),
                ty.clone(),
                OwnershipClass::Unknown,
                class,
            )
        })
        .collect::<Vec<_>>();
    if let Some(ty) = result {
        values.push(boundary_value(
            BoundaryRole::Result,
            "result".to_string(),
            Some(ty.clone()),
            OwnershipClass::Owned,
            result_class,
        ));
    } else if result_class != GeneralDestinationClass::Unknown {
        values.push(boundary_value(
            BoundaryRole::Result,
            "result".to_string(),
            None,
            OwnershipClass::Owned,
            result_class,
        ));
    }
    values
}

fn boundary_value(
    role: BoundaryRole,
    name: String,
    ty: Option<Type>,
    ownership: OwnershipClass,
    internal_class: GeneralDestinationClass,
) -> BoundaryValuePlan {
    let conversion = value_boundary_conversion(internal_class);
    BoundaryValuePlan {
        role,
        name,
        ty,
        internal_class,
        external_class: GeneralDestinationClass::Boxed,
        conversion,
        ownership,
        proof: ProofStatus::Conservative {
            reason: FallbackReason::ExternalBoundary,
        },
    }
}

fn value_boundary_conversion(internal: GeneralDestinationClass) -> BoundaryConversion {
    match internal {
        GeneralDestinationClass::ScalarValue => BoundaryConversion::WidthConvert,
        GeneralDestinationClass::Unknown => BoundaryConversion::CopyAtBoundary,
        _ => BoundaryConversion::BoxAtBoundary,
    }
}

fn general_destination(result: ResultDestination) -> GeneralDestinationClass {
    match result {
        ResultDestination::Void => GeneralDestinationClass::Unknown,
        ResultDestination::SsaValue => GeneralDestinationClass::ScalarValue,
        ResultDestination::CallerDestination => GeneralDestinationClass::CallerDestination,
        ResultDestination::Boxed | ResultDestination::Unknown => GeneralDestinationClass::Boxed,
    }
}

fn result_destination(result: ResultDestination) -> GeneralResultDestination {
    match result {
        ResultDestination::Void => GeneralResultDestination::Void,
        ResultDestination::SsaValue => GeneralResultDestination::SsaValue,
        ResultDestination::CallerDestination => GeneralResultDestination::CallerDestination,
        ResultDestination::Boxed | ResultDestination::Unknown => GeneralResultDestination::Boxed,
    }
}

fn ownership_for_obligation(obligation: ParamObligation) -> OwnershipClass {
    match obligation {
        ParamObligation::Value => OwnershipClass::Owned,
        ParamObligation::Borrow => OwnershipClass::Borrowed,
        _ => OwnershipClass::Unknown,
    }
}

fn general_fact(
    id: SemanticValueId,
    ty: Option<Type>,
    ownership: OwnershipClass,
    destination: GeneralDestinationClass,
) -> GeneralValueFact {
    let shape = shape_for_type(ty.as_ref());
    GeneralValueFact {
        id,
        ty,
        shape,
        ownership,
        destination,
        may_overlap: true,
        proof: ProofStatus::Conservative {
            reason: FallbackReason::UnknownOwnership,
        },
    }
}

#[derive(Debug, Clone, Default)]
struct CallableTypeShapes {
    parameters: Vec<Option<Type>>,
    result: Option<Type>,
}

fn callable_shapes(
    program: &HirProgram,
    legacy: &HirDestinationOwnershipPlan,
) -> BTreeMap<CallableId, CallableTypeShapes> {
    let mut out = BTreeMap::new();
    for (name, summary) in &legacy.callables {
        let callable = callable_id(name, summary.kind);
        out.insert(callable, CallableTypeShapes::default());
    }
    for callable in &program.callables {
        if let Some((id, shapes)) = callable_type_shapes(callable) {
            out.insert(id, shapes);
        }
    }
    out
}

/// AST `callable_type_shapes` mirror. The AST matches top-level
/// `Definition`/`Transaction`/`CompileTimeDefn`/`CompileTimeTxn` items and
/// unwraps `Export`; HIR lowers wrappers flat, so the same namespaces fall
/// out of the callable kinds. `isr`/`impl`/trait-default/`sync<>` callables
/// mirror the AST arms that never match. Result types come from the declared
/// output SHAPE (the AST builder has no multi-output list fallback); member
/// callables keep the bare-name quirk — those entries are never looked up
/// because legacy member keys are owner-qualified.
fn callable_type_shapes(callable: &HirCallable) -> Option<(CallableId, CallableTypeShapes)> {
    if callable.is_isr || callable.is_impl_fn || callable.trait_default {
        return None;
    }
    if !callable.sync_domains.is_empty() {
        return None;
    }
    let namespace = match callable.kind {
        HirCallableKind::Defn => CallableNamespace::Definition,
        HirCallableKind::Transaction => CallableNamespace::Transaction,
        HirCallableKind::CompileTimeDefn => CallableNamespace::CompileTimeDefinition,
        HirCallableKind::CompileTimeTxn => CallableNamespace::CompileTimeTransaction,
        HirCallableKind::MemberDefn | HirCallableKind::MemberTransaction => {
            CallableNamespace::Member
        }
        HirCallableKind::IsrHandler => return None,
    };
    let result = callable
        .outputs
        .as_ref()
        .and_then(|output| output.all_types().into_iter().next());
    Some((
        CallableId {
            namespace,
            name: callable.name.clone(),
        },
        CallableTypeShapes {
            parameters: callable
                .params
                .iter()
                .map(|param| Some(param.ty.clone()))
                .collect(),
            result,
        },
    ))
}

fn callable_id(name: &str, kind: CallableKind) -> CallableId {
    CallableId {
        namespace: namespace_for_kind(kind),
        name: name.to_string(),
    }
}

fn namespace_for_kind(kind: CallableKind) -> CallableNamespace {
    match kind {
        CallableKind::InternalDefn => CallableNamespace::Definition,
        CallableKind::InternalTransaction => CallableNamespace::Transaction,
        CallableKind::Member => CallableNamespace::Member,
        CallableKind::Foreign => CallableNamespace::Foreign,
        CallableKind::Unknown => CallableNamespace::Unknown,
    }
}

fn semantic_value(
    callable: &CallableId,
    kind: SemanticValueKind,
    path: &str,
    incarnation: u32,
) -> SemanticValueId {
    SemanticValueId {
        callable: callable.clone(),
        kind,
        path: path.to_string(),
        incarnation,
    }
}

fn statement_path_from_site(site: &str) -> Vec<usize> {
    // R1 site paths use `/stmt/...`. Parse only the canonical numeric suffixes;
    // malformed paths simply produce an empty statement path.
    let marker = "/stmt/";
    let Some(path) = site.split_once(marker).map(|(_, tail)| tail) else {
        return Vec::new();
    };
    path.split('/')
        .filter_map(|part| part.parse::<usize>().ok())
        .collect()
}

fn shape_for_type(ty: Option<&Type>) -> ValueShape {
    let Some(ty) = ty else {
        return ValueShape {
            kind: ValueShapeKind::Unknown,
            ty: Type::Void,
            payload_words: None,
            inline_words: None,
            boxed_fallback: true,
            recursive: false,
            external_abi: false,
        };
    };
    let mut shape = ValueShape {
        kind: ValueShapeKind::Unknown,
        ty: ty.clone(),
        payload_words: None,
        inline_words: None,
        boxed_fallback: true,
        recursive: false,
        external_abi: false,
    };
    match ty {
        Type::Bits(_) | Type::Void => shape.kind = ValueShapeKind::Scalar,
        Type::Tuple(fields) => {
            shape.kind = ValueShapeKind::Tuple;
            shape.payload_words = Some(fields.len() as u64);
        }
        Type::Union(_) => shape.kind = ValueShapeKind::Enum,
        Type::Task(_) => shape.kind = ValueShapeKind::Task,
        Type::Ptr(_) | Type::PtrConst(_) | Type::LayoutPtr(_) => {
            shape.kind = ValueShapeKind::Reference;
        }
        Type::Function(_, _) => shape.kind = ValueShapeKind::Closure,
        Type::Vector(_, _) => shape.kind = ValueShapeKind::Collection,
        Type::Dyn(_) => shape.external_abi = true,
        // Named/composite shapes require declaration/layout facts. D1 does not
        // guess from source type names.
        Type::Custom(_)
        | Type::Generic(..)
        | Type::Applied(..)
        | Type::TypeVar(_)
        | Type::Constrained(..) => {
            shape.kind = ValueShapeKind::Opaque;
        }
        Type::Width(_) | Type::Number(_) => {
            shape.kind = ValueShapeKind::Scalar;
        }
    }
    shape
}
