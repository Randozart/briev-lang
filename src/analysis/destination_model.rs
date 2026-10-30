// ── Generalized destination model ──────────────────────────────────────
// 2026-10-30 (Phase D1, plan
// docs/plans/2026-10-30-generalized-value-destination-ownership.md): complete
// target-independent destination/ownership schema for all Briev value
// domains.
//
// This module defines the generalized schema and builds a conservative plan
// from the Phase R1 facts. No backend consumes it yet. Unknown shape,
// ownership, aliasing, or boundary information remains boxed/unknown.
//
// Undo: delete this module and the `generalized` field on
// `DestinationOwnershipPlan`; no other code depends on it.

use crate::ast::{TopLevel, Type};
use crate::analysis::destination_ownership::{
    CallableKind, DestinationClass as LegacyDestinationClass,
    DestinationOwnershipPlan, ParamObligation, ResultDestination,
    ValueOrigin,
};
use std::collections::{BTreeMap, BTreeSet};

/// Stable callable identity. The namespace keeps definition/transaction/member
/// owners distinguishable while Briev call resolution remains name-based.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CallableId {
    pub namespace: CallableNamespace,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CallableNamespace {
    Definition,
    Transaction,
    CompileTimeDefinition,
    CompileTimeTransaction,
    Member,
    Foreign,
    Unknown,
}

/// Stable identity of one destination/storage cell.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DestinationId {
    pub owner: String,
    pub path: String,
}

/// Stable identity of one semantic value.
///
/// This is intentionally independent of LLVM register numbering. `path` is
/// the canonical source path and `incarnation` distinguishes reinitialized
/// bindings in the same path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemanticValueId {
    pub callable: CallableId,
    pub kind: SemanticValueKind,
    pub path: String,
    pub incarnation: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticValueKind {
    Parameter,
    Local,
    Temporary,
    CallResult,
    MatchArm,
    LoopItem,
    StateCell,
    External,
    FieldProjection,
    CollectionElement,
    Constructor,
    Unknown,
}

/// Canonical program point. Source paths are stable; expression path is empty
/// for statement-only points.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProgramPoint {
    pub owner: String,
    pub statement_path: Vec<usize>,
    pub expression_path: Vec<usize>,
}

impl ProgramPoint {
    pub fn key(&self) -> String {
        let statement = self
            .statement_path
            .iter()
            .map(|index| index.to_string())
            .collect::<Vec<_>>()
            .join("_");
        let expression = self
            .expression_path
            .iter()
            .map(|index| index.to_string())
            .collect::<Vec<_>>()
            .join("_");
        if expression.is_empty() {
            format!("{}/{}", self.owner, statement)
        } else {
            format!("{}/{}/e{}", self.owner, statement, expression)
        }
    }
}

/// One observed use of a semantic value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UseSite {
    pub point: ProgramPoint,
    pub kind: UseKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UseKind {
    Read,
    Mutate,
    Project,
    Move,
    Borrow,
}

/// Target-independent physical shape. Unknown named/composite types remain
/// opaque until concrete layout facts are supplied.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueShape {
    pub kind: ValueShapeKind,
    pub ty: Type,
    pub payload_words: Option<u64>,
    pub inline_words: Option<u64>,
    pub boxed_fallback: bool,
    pub recursive: bool,
    pub external_abi: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValueShapeKind {
    Scalar,
    Enum,
    Object,
    Tuple,
    Collection,
    Slice,
    Closure,
    Task,
    Reference,
    Opaque,
    Unknown,
}

/// Generalized ownership class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OwnershipClass {
    Owned,
    Borrowed,
    Consumed,
    Shared,
    Unknown,
}

impl OwnershipClass {
    pub fn as_str(self) -> &'static str {
        match self {
            OwnershipClass::Owned => "owned",
            OwnershipClass::Borrowed => "borrowed",
            OwnershipClass::Consumed => "consumed",
            OwnershipClass::Shared => "shared",
            OwnershipClass::Unknown => "unknown",
        }
    }
}

/// Generalized destination class. `ZeroCopyView` requires a lifetime proof
/// that dominates every use and proves no overlapping mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GeneralDestinationClass {
    ScalarValue,
    CallerDestination,
    ScalarReplacedObject,
    StableCarrier,
    PackedMemory,
    ZeroCopyView,
    Boxed,
    Unknown,
}

impl GeneralDestinationClass {
    pub fn as_str(self) -> &'static str {
        match self {
            GeneralDestinationClass::ScalarValue => "scalar-value",
            GeneralDestinationClass::CallerDestination => "caller-destination",
            GeneralDestinationClass::ScalarReplacedObject => "scalar-replaced-object",
            GeneralDestinationClass::StableCarrier => "stable-carrier",
            GeneralDestinationClass::PackedMemory => "packed-memory",
            GeneralDestinationClass::ZeroCopyView => "zero-copy-view",
            GeneralDestinationClass::Boxed => "boxed",
            GeneralDestinationClass::Unknown => "unknown",
        }
    }
}

/// Proof status for a plan decision.
#[derive(Debug, Clone, PartialEq)]
pub enum ProofStatus {
    Proven { reasons: BTreeSet<ProofReason> },
    Conservative { reason: FallbackReason },
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProofReason {
    NonEscaping,
    CallerOwnedDestination,
    NonOverlapping,
    StableCarrierLifetime,
    ConsumedAtLastUse,
    BoundaryOwned,
    LayoutAgreement,
}

impl ProofReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ProofReason::NonEscaping => "non-escaping",
            ProofReason::CallerOwnedDestination => "caller-owned-destination",
            ProofReason::NonOverlapping => "non-overlapping",
            ProofReason::StableCarrierLifetime => "stable-carrier-lifetime",
            ProofReason::ConsumedAtLastUse => "consumed-at-last-use",
            ProofReason::BoundaryOwned => "boundary-owned",
            ProofReason::LayoutAgreement => "layout-agreement",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FallbackReason {
    UnknownOwnership,
    MayOverlap,
    Escapes,
    RecursiveOrCyclic,
    ExternalBoundary,
    UnsupportedShape,
    UnknownCallSummary,
}

impl FallbackReason {
    pub fn as_str(self) -> &'static str {
        match self {
            FallbackReason::UnknownOwnership => "unknown-ownership",
            FallbackReason::MayOverlap => "may-overlap",
            FallbackReason::Escapes => "escapes",
            FallbackReason::RecursiveOrCyclic => "recursive-or-cyclic",
            FallbackReason::ExternalBoundary => "external-boundary",
            FallbackReason::UnsupportedShape => "unsupported-shape",
            FallbackReason::UnknownCallSummary => "unknown-call-summary",
        }
    }
}

/// Generalized ownership obligation attached to a value at a program point.
#[derive(Debug, Clone, PartialEq)]
pub struct Obligation {
    pub point: ProgramPoint,
    pub obligation: OwnershipObligation,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OwnershipObligation {
    CopyBeforeUse,
    MoveBeforeUse,
    MaterializeBeforeBoundary,
    BorrowOnly,
    KeepAlive,
}

impl OwnershipObligation {
    pub fn as_str(self) -> &'static str {
        match self {
            OwnershipObligation::CopyBeforeUse => "copy-before-use",
            OwnershipObligation::MoveBeforeUse => "move-before-use",
            OwnershipObligation::MaterializeBeforeBoundary => "materialize-before-boundary",
            OwnershipObligation::BorrowOnly => "borrow-only",
            OwnershipObligation::KeepAlive => "keep-alive",
        }
    }
}

/// Generalized parameter plan.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterPlan {
    pub value: SemanticValueId,
    pub ty: Option<Type>,
    pub obligation: ParamObligation,
    pub destination: Option<DestinationId>,
    pub requires_copy: bool,
}

/// Generalized result plan.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultPlan {
    pub value: Option<SemanticValueId>,
    pub ty: Option<Type>,
    pub destination: GeneralResultDestination,
    pub requires_materialization: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GeneralResultDestination {
    Void,
    SsaValue,
    CallerDestination,
    Boxed,
    Unknown,
}

impl GeneralResultDestination {
    pub fn as_str(self) -> &'static str {
        match self {
            GeneralResultDestination::Void => "void",
            GeneralResultDestination::SsaValue => "ssa-value",
            GeneralResultDestination::CallerDestination => "caller-destination",
            GeneralResultDestination::Boxed => "boxed",
            GeneralResultDestination::Unknown => "unknown",
        }
    }
}

/// Hidden internal ABI parameters selected for a callable. Public ABI is not
/// part of this plan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HiddenAbiPlan {
    pub state: bool,
    pub plain: bool,
    pub enum_result_destination: bool,
    pub tail_carriers: u64,
    pub object_result_destination: bool,
}

/// Generalized per-callable plan.
#[derive(Debug, Clone, PartialEq)]
pub struct CallablePlan {
    pub callable: CallableId,
    pub exported: bool,
    pub direct_recursive: bool,
    pub parameters: Vec<ParameterPlan>,
    pub result: ResultPlan,
    pub hidden_abi: HiddenAbiPlan,
    pub proof: ProofStatus,
}

/// Generalized argument plan.
#[derive(Debug, Clone, PartialEq)]
pub struct ArgumentPlan {
    pub value: SemanticValueId,
    pub obligation: ParamObligation,
    pub requires_copy: bool,
    pub may_overlap_callee_destination: bool,
}

/// Generalized result-use plan at a call site.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultUsePlan {
    pub value: SemanticValueId,
    pub destination: Option<DestinationId>,
    pub class: GeneralDestinationClass,
    pub requires_copy: bool,
}

/// Generalized call-site plan.
#[derive(Debug, Clone, PartialEq)]
pub struct CallPlan {
    pub site: ProgramPoint,
    pub callee: CallableId,
    pub arguments: Vec<ArgumentPlan>,
    pub result: Option<ResultUsePlan>,
    pub may_overlap_inputs: bool,
    pub proof: ProofStatus,
}

/// One loop-carried value and its carrier obligation.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopValuePlan {
    pub value: SemanticValueId,
    pub carrier: DestinationId,
    pub class: GeneralDestinationClass,
    pub requires_backedge_copy: bool,
}

/// Generalized loop plan.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopPlan {
    pub loop_id: DestinationId,
    pub carrier_values: Vec<LoopValuePlan>,
    pub iteration_local_values: Vec<SemanticValueId>,
    pub proof: ProofStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BoundaryKind {
    Export,
    ForeignCall,
    RuntimeHelper,
    WebView,
    PersistedState,
    ForeignTask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BoundaryConversion {
    None,
    BoxAtBoundary,
    UnboxAtBoundary,
    CopyAtBoundary,
    ZeroCopyWithLifetimeProof,
    WidthConvert,
}

impl BoundaryConversion {
    pub fn as_str(self) -> &'static str {
        match self {
            BoundaryConversion::None => "none",
            BoundaryConversion::BoxAtBoundary => "box-at-boundary",
            BoundaryConversion::UnboxAtBoundary => "unbox-at-boundary",
            BoundaryConversion::CopyAtBoundary => "copy-at-boundary",
            BoundaryConversion::ZeroCopyWithLifetimeProof => "zero-copy-with-lifetime-proof",
            BoundaryConversion::WidthConvert => "width-convert",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryPlan {
    pub boundary: DestinationId,
    pub kind: BoundaryKind,
    pub internal_class: GeneralDestinationClass,
    pub external_class: GeneralDestinationClass,
    pub conversion: BoundaryConversion,
    pub ownership: OwnershipClass,
    pub proof: ProofStatus,
}

/// Generalized value fact.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneralValueFact {
    pub id: SemanticValueId,
    pub ty: Option<Type>,
    pub shape: ValueShape,
    pub ownership: OwnershipClass,
    pub destination: GeneralDestinationClass,
    pub may_overlap: bool,
    pub proof: ProofStatus,
}

/// Generalized Phase D1 result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GeneralizedDestinationOwnershipPlan {
    pub values: BTreeMap<SemanticValueId, GeneralValueFact>,
    pub callables: BTreeMap<CallableId, CallablePlan>,
    pub call_sites: BTreeMap<ProgramPoint, CallPlan>,
    pub loops: BTreeMap<DestinationId, LoopPlan>,
    pub boundaries: BTreeMap<DestinationId, BoundaryPlan>,
}

impl GeneralizedDestinationOwnershipPlan {
    /// Deterministic debug snapshot. Derived `Debug` over ordered maps is
    /// stable for a given compiler version and is sufficient for D1 evidence.
    pub fn debug_snapshot(&self) -> String {
        format!("{self:#?}")
    }

    /// Number of plan facts still conservative. This makes hidden fallback
    /// visible to reports without printing during normal compilation.
    pub fn conservative_count(&self) -> usize {
        self.values
            .values()
            .filter(|fact| {
                matches!(
                    fact.proof,
                    ProofStatus::Conservative { .. } | ProofStatus::Unknown
                )
            })
            .count()
            + self
                .callables
                .values()
                .filter(|plan| {
                    matches!(
                        plan.proof,
                        ProofStatus::Conservative { .. } | ProofStatus::Unknown
                    )
                })
                .count()
            + self
                .call_sites
                .values()
                .filter(|plan| {
                    matches!(
                        plan.proof,
                        ProofStatus::Conservative { .. } | ProofStatus::Unknown
                    )
                })
                .count()
            + self
                .loops
                .values()
                .filter(|plan| {
                    matches!(
                        plan.proof,
                        ProofStatus::Conservative { .. } | ProofStatus::Unknown
                    )
                })
                .count()
            + self
                .boundaries
                .values()
                .filter(|plan| {
                    matches!(
                        plan.proof,
                        ProofStatus::Conservative { .. } | ProofStatus::Unknown
                    )
                })
                .count()
    }
}

/// Build the generalized Phase D1 schema from the Phase R1 facts.
///
/// Construction is deliberately conservative: every generalized value is
/// `Unknown` ownership unless R1 proves a narrower fact, every non-primitive
/// shape is opaque, every loop/boundary set is empty until dedicated walkers
/// are added in later phases.
pub fn build(items: &[TopLevel], legacy: &DestinationOwnershipPlan) -> GeneralizedDestinationOwnershipPlan {
    let mut plan = GeneralizedDestinationOwnershipPlan::default();
    let shapes = callable_shapes(items, legacy);

    for (name, summary) in &legacy.callables {
        let (callable_plan, facts) = build_callable(name, summary, &shapes);
        plan.values.extend(
            facts
                .into_iter()
                .map(|fact| (fact.id.clone(), fact)),
        );
        plan.callables.insert(callable_plan.callable.clone(), callable_plan);
    }

    for (site_id, call) in &legacy.call_sites {
        let (call_plan, facts) = build_call(site_id, call);
        plan.values.extend(facts.into_iter().map(|fact| (fact.id.clone(), fact)));
        plan.call_sites.insert(call_plan.site.clone(), call_plan);
    }

    plan.loops = build_loops(legacy);
    plan.boundaries = build_boundaries(legacy);
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
    plan.values.extend(loop_facts.into_iter().map(|fact| (fact.id.clone(), fact)));
    plan
}

fn build_callable(
    name: &str,
    summary: &crate::analysis::destination_ownership::CallableSummary,
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
    summary: &crate::analysis::destination_ownership::CallableSummary,
    shapes: &BTreeMap<CallableId, CallableTypeShapes>,
) -> CallablePlan {
    let callable = callable_id(name, summary.kind);
    let type_shapes = shapes.get(&callable);
    let parameters = summary
        .parameters
        .iter()
        .enumerate()
        .map(|(index, obligation)| {
            build_parameter(&callable, index, *obligation, type_shapes)
        })
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
    let ty = shapes.and_then(|shapes| shapes.parameters.get(index)).and_then(Option::clone);
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
    let arguments = call
        .argument_values
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
        .collect();
    let result = (call.result != ResultDestination::Void).then(|| {
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
        ResultUsePlan {
            value,
            destination: None,
            class: general_destination(call.result),
            requires_copy: true,
        }
    });
    let proof = if call.kind == CallableKind::Unknown {
        ProofStatus::Conservative {
            reason: FallbackReason::UnknownCallSummary,
        }
    } else {
        ProofStatus::Unknown
    };
    (CallPlan {
        site: point.clone(),
        callee,
        arguments,
        result,
        may_overlap_inputs: call.may_alias_args,
        proof,
    }, facts)
}

fn build_loops(
    legacy: &DestinationOwnershipPlan,
) -> BTreeMap<DestinationId, LoopPlan> {
    legacy
        .callables_by_owner
        .iter()
        .filter_map(|(owner, facts)| {
            let values = loop_values(owner, facts);
            if values.is_empty() {
                return None;
            }
            let loop_id = DestinationId {
                owner: owner.clone(),
                path: "loop/0".to_string(),
            };
            Some((loop_id.clone(), LoopPlan {
                loop_id,
                carrier_values: values,
                iteration_local_values: Vec::new(),
                proof: ProofStatus::Conservative {
                    reason: FallbackReason::Escapes,
                },
            }))
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
        .filter(|fact| fact.escapes.contains(&crate::analysis::destination_ownership::EscapeKind::LoopLifetime))
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
            class: GeneralDestinationClass::Boxed,
            requires_backedge_copy: true,
        })
        .collect()
}

fn build_boundaries(
    legacy: &DestinationOwnershipPlan,
) -> BTreeMap<DestinationId, BoundaryPlan> {
    legacy
        .callables
        .values()
        .filter(|summary| summary.exported)
        .map(|summary| {
            let boundary = DestinationId {
                owner: summary.name.clone(),
                path: "boundary/export".to_string(),
            };
            (boundary.clone(), BoundaryPlan {
                boundary,
                kind: BoundaryKind::Export,
                internal_class: GeneralDestinationClass::Unknown,
                external_class: GeneralDestinationClass::Boxed,
                conversion: BoundaryConversion::BoxAtBoundary,
                ownership: OwnershipClass::Unknown,
                proof: ProofStatus::Conservative {
                    reason: FallbackReason::ExternalBoundary,
                },
            })
        })
        .collect()
}

fn general_destination(result: ResultDestination) -> GeneralDestinationClass {
    match result {
        ResultDestination::Void => GeneralDestinationClass::Unknown,
        ResultDestination::SsaValue => GeneralDestinationClass::ScalarValue,
        ResultDestination::CallerDestination => GeneralDestinationClass::CallerDestination,
        ResultDestination::Boxed | ResultDestination::Unknown => {
            GeneralDestinationClass::Boxed
        }
    }
}


fn result_destination(result: ResultDestination) -> GeneralResultDestination {
    match result {
        ResultDestination::Void => GeneralResultDestination::Void,
        ResultDestination::SsaValue => GeneralResultDestination::SsaValue,
        ResultDestination::CallerDestination => GeneralResultDestination::CallerDestination,
        ResultDestination::Boxed | ResultDestination::Unknown => {
            GeneralResultDestination::Boxed
        }
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
    items: &[TopLevel],
    legacy: &DestinationOwnershipPlan,
) -> BTreeMap<CallableId, CallableTypeShapes> {
    let mut out = BTreeMap::new();
    for (name, summary) in &legacy.callables {
        let callable = callable_id(name, summary.kind);
        out.insert(callable, CallableTypeShapes::default());
    }
    for item in items {
        if let Some((callable, shapes)) = callable_type_shapes(item) {
            out.insert(callable, shapes);
        }
    }
    out
}

fn callable_type_shapes(item: &TopLevel) -> Option<(CallableId, CallableTypeShapes)> {
    match item {
        TopLevel::Definition(d) => Some((
            CallableId {
                namespace: CallableNamespace::Definition,
                name: d.name.clone(),
            },
            callable_type_shapes_from_parts(&d.parameters, d.output_type.as_ref()),
        )),
        TopLevel::TypeDefOperator(d) => Some((
            CallableId {
                namespace: CallableNamespace::Member,
                name: d.name.clone(),
            },
            callable_type_shapes_from_parts(&d.parameters, d.output_type.as_ref()),
        )),
        TopLevel::Transaction(t) => Some((
            CallableId {
                namespace: CallableNamespace::Transaction,
                name: t.name.clone(),
            },
            callable_type_shapes_from_parts(&t.parameters, t.output_type.as_ref()),
        )),
        TopLevel::CompileTimeDefn(d) => Some((
            CallableId {
                namespace: CallableNamespace::CompileTimeDefinition,
                name: d.name.clone(),
            },
            callable_type_shapes_from_parts(&d.parameters, d.output_type.as_ref()),
        )),
        TopLevel::CompileTimeTxn(t) => Some((
            CallableId {
                namespace: CallableNamespace::CompileTimeTransaction,
                name: t.name.clone(),
            },
            callable_type_shapes_from_parts(&t.parameters, t.output_type.as_ref()),
        )),
        TopLevel::Export(export) => callable_type_shapes(export.inner.as_ref()),
        _ => None,
    }
}

fn callable_type_shapes_from_parts(
    parameters: &[(String, Type)],
    output: Option<&crate::ast::OutputType>,
) -> CallableTypeShapes {
    CallableTypeShapes {
        parameters: parameters.iter().map(|(_, ty)| Some(ty.clone())).collect(),
        result: output.and_then(|output| match output {
            crate::ast::OutputType::Single(ty) => Some(ty.clone()),
            other => other.all_types().first().cloned(),
        }),
    }
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
        Type::Custom(_) | Type::Generic(..) | Type::Applied(..) | Type::TypeVar(_)
        | Type::Constrained(..) => {
            shape.kind = ValueShapeKind::Opaque;
        }
        Type::Width(_) | Type::Number(_) => {
            shape.kind = ValueShapeKind::Scalar;
        }
    }
    shape
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze_items(src: &str) -> (DestinationOwnershipPlan, GeneralizedDestinationOwnershipPlan) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let legacy = crate::analysis::destination_ownership::analyze(&items);
        let generalized = build(&items, &legacy);
        (legacy, generalized)
    }

    #[test]
    fn generalized_schema_covers_callable_parameters_and_results() {
        let (_, plan) = analyze_items(
            "defn identity(value: Int) -> Int { term value; };\n",
        );
        let callable = plan
            .callables
            .values()
            .next()
            .expect("callable plan");
        assert_eq!(callable.parameters.len(), 1);
        assert_eq!(
            callable.result.destination,
            GeneralResultDestination::SsaValue
        );
        let parameter = &callable.parameters[0];
        assert_eq!(parameter.obligation, ParamObligation::Value);
        assert!(parameter.ty.is_some());
        assert!(plan.values.contains_key(&parameter.value));
        assert!(plan.values.contains_key(callable.result.value.as_ref().expect("result value")));
    }

    #[test]
    fn unknown_calls_are_conservative() {
        let (_, plan) = analyze_items(
            r#"
defn call_unknown() -> Int {
    let value: Int = unknown_fn(1);
    term value;
};
"#,
        );
        assert_eq!(plan.conservative_count() > 0, true);
        let call = plan.call_sites.values().next().expect("call site");
        assert!(matches!(call.proof, ProofStatus::Conservative { .. }));
        assert!(call.result.is_some());
        let result = call.result.as_ref().expect("result use");
        assert!(plan.values.contains_key(&result.value));
        assert!(plan
            .values
            .contains_key(&call.arguments[0].value));
    }

    #[test]
    fn loop_carried_assignments_create_loop_plans() {
        let (legacy, plan) = analyze_items(
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
        assert!(legacy
            .callables_by_owner
            .values()
            .flat_map(|facts| facts.values.values())
            .any(|fact| fact
                .sinks
                .contains_key(&crate::analysis::destination_ownership::SinkKind::LoopCarrier)));
        assert!(!plan.loops.is_empty());
        let loop_plan = plan.loops.values().next().expect("loop plan");
        assert!(!loop_plan.carrier_values.is_empty());
        assert!(loop_plan
            .carrier_values
            .iter()
            .all(|value| value.requires_backedge_copy));
    }

    #[test]
    fn exported_callables_create_boundary_plans() {
        let (_, plan) = analyze_items(
            "export defn add(a: Int, b: Int) -> Int { term a + b; };\n",
        );
        let boundary = plan.boundaries.values().next().expect("boundary plan");
        assert_eq!(boundary.kind, BoundaryKind::Export);
        assert_eq!(boundary.conversion, BoundaryConversion::BoxAtBoundary);
        assert_eq!(boundary.ownership, OwnershipClass::Unknown);
    }

    #[test]
    fn production_plan_equals_direct_rebuild() {
        let src = r#"
defn helper(v: Int) -> Int { term v; };
defn caller(v: Int) -> Int { term helper(v); };
"#;
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let legacy = crate::analysis::destination_ownership::analyze(&items);
        let direct = build(&items, &legacy);
        assert_eq!(legacy.generalized, direct);
        assert!(!legacy.generalized.call_sites.is_empty());
    }

    #[test]
    fn generalized_plan_is_deterministic() {
        let src = r#"
defn helper(v: Int) -> Int { term v; };
defn caller(v: Int) -> Int { term helper(v); };
"#;
        let (_, first) = analyze_items(src);
        let (_, second) = analyze_items(src);
        assert_eq!(first, second);
        assert_eq!(first.debug_snapshot(), second.debug_snapshot());
    }
}
