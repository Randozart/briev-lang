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

use crate::ast::{EnumVariant, TopLevel, Type};
use crate::analysis::tail_carrier::{
    build_tail_carrier_plan, CarrierArgClass as LegacyCarrierArgClass, CarrierInfo,
    CarrierParam as LegacyCarrierParam, TailCarrierPlan,
};
use crate::analysis::destination_ownership::{
    CallableKind, DestinationClass as LegacyDestinationClass,
    DestinationOwnershipPlan, EscapeKind, ParamObligation, ResultDestination,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecursiveCarrierArgClass {
    InlineNested,
    ExistingCarrier,
    /// Present only as an internal invariant guard; approved carrier plans
    /// never contain an unsupported recursive arm.
    Unsupported,
}

/// One recursive-call parameter that must live in a stable carrier image.
#[derive(Debug, Clone, PartialEq)]
pub struct RecursiveCarrierValue {
    pub parameter: SemanticValueId,
    pub param_index: usize,
    pub param_name: String,
    pub result_local: String,
    pub sret_callee: String,
    pub arg_classes: Vec<RecursiveCarrierArgClass>,
    pub carrier: DestinationId,
    pub class: GeneralDestinationClass,
    pub requires_backedge_copy: bool,
    pub proof: ProofStatus,
}

/// Stable carriers for a direct self-tail-recursive callable.
#[derive(Debug, Clone, PartialEq)]
pub struct RecursiveCarrierPlan {
    pub callable: CallableId,
    pub carriers: Vec<RecursiveCarrierValue>,
    pub proof: ProofStatus,
}

/// Loop-engine storage decision for one carried field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopFieldRepresentation {
    /// Loop-local SSA phi; no post-loop state commitment.
    Phi,
    /// Stable state carrier; committed at loop convergence.
    StateCarrier,
}

/// One carried field's loop-engine destination.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopFieldPlan {
    pub field: String,
    pub storage: crate::analysis::materialization::StorageClass,
    pub representation: LoopFieldRepresentation,
    pub proof: ProofStatus,
}

/// Unified loop-engine destination plan for a bounded reactive node.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopEnginePlan {
    pub node: String,
    pub counter: String,
    pub fields: BTreeMap<String, LoopFieldPlan>,
    pub proof: ProofStatus,
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

/// One declared enum variant (constructor) in semantic form.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariantPlan {
    pub name: String,
    /// `None` for a unit variant; `Some(Tuple)` for multi-field payloads.
    pub payload: Option<Type>,
}

/// Declared enum shape. Physical word counts remain the backend's
/// `EnumLayoutPlanner` responsibility; this record carries the semantic
/// constructor set and payload types without guessing layout.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDefinitionPlan {
    pub name: String,
    pub type_params: Vec<String>,
    pub variants: Vec<EnumVariantPlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumResultRepresentation {
    InlineLocal,
    CallerDestination,
    Boxed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructorAgreement {
    AllConstructors,
    ContainsNonConstructor,
    Unknown,
}

/// Per-callable enum result plan. The plan records agreement and destination;
/// D3 migration consumes it, while unmigrated callables stay on the current
/// emitter path.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumCallablePlan {
    pub callable: CallableId,
    pub result_type: Option<Type>,
    pub representation: EnumResultRepresentation,
    pub agreement: ConstructorAgreement,
    pub hidden_destination: bool,
    pub proof: ProofStatus,
}

/// One declared object/struct field.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectFieldPlan {
    pub name: String,
    pub ty: Type,
}

/// Declared object/struct shape. Physical layout stays in the backend.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectDefinitionPlan {
    pub name: String,
    pub type_params: Vec<String>,
    pub fields: Vec<ObjectFieldPlan>,
}

/// One observed object construction and its escape state.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectValuePlan {
    pub value: SemanticValueId,
    pub ty: Type,
    pub object: String,
    pub class: GeneralDestinationClass,
    pub escapes: BTreeSet<EscapeKind>,
    pub requires_materialization: bool,
    pub proof: ProofStatus,
}

/// Sink categories covered by the D5 container/state lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DestinationSinkKind {
    State,
    Collection,
    ObjectField,
}

impl DestinationSinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DestinationSinkKind::State => "state",
            DestinationSinkKind::Collection => "collection",
            DestinationSinkKind::ObjectField => "object-field",
        }
    }
}

/// One state/collection/object-field destination. Copy elimination is a later
/// D5 slice; every observed sink starts conservative.
#[derive(Debug, Clone, PartialEq)]
pub struct DestinationSinkPlan {
    pub destination: DestinationId,
    pub value: SemanticValueId,
    pub kind: DestinationSinkKind,
    pub class: GeneralDestinationClass,
    pub requires_copy: bool,
    pub proof: ProofStatus,
}

/// A tuple construction destination. Tuple element types are not yet known by
/// R1, so `element_types` remains a typed-hole rather than guessing layout.
#[derive(Debug, Clone, PartialEq)]
pub struct TupleDestinationPlan {
    pub destination: DestinationId,
    pub value: SemanticValueId,
    pub element_count: usize,
    pub class: GeneralDestinationClass,
    pub requires_materialization: bool,
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
    pub enums: BTreeMap<String, EnumDefinitionPlan>,
    pub enum_callables: BTreeMap<CallableId, EnumCallablePlan>,
    pub objects: BTreeMap<String, ObjectDefinitionPlan>,
    pub object_values: BTreeMap<SemanticValueId, ObjectValuePlan>,
    pub sinks: BTreeMap<DestinationId, DestinationSinkPlan>,
    pub tuples: BTreeMap<DestinationId, TupleDestinationPlan>,
    pub recursive_carriers: BTreeMap<CallableId, RecursiveCarrierPlan>,
    pub loop_engines: BTreeMap<String, LoopEnginePlan>,
}

impl GeneralizedDestinationOwnershipPlan {
    /// Deterministic debug snapshot. Derived `Debug` over ordered maps is
    /// stable for a given compiler version and is sufficient for D1 evidence.
    pub fn debug_snapshot(&self) -> String {
        format!("{self:#?}")
    }

    /// Object types that may be stack/scalar lowered in each callable: every
    /// observed object value of that type in the owner is proven non-escaping.
    /// A single escaping alias removes the type for that owner.
    pub fn stack_object_types_by_owner(&self) -> BTreeMap<String, BTreeSet<String>> {
        let mut groups: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
        for value in self.object_values.values() {
            let owner = value.value.callable.name.clone();
            groups
                .entry(owner)
                .or_default()
                .entry(value.object.clone())
                .and_modify(|safe| *safe &= !value.requires_materialization)
                .or_insert(!value.requires_materialization);
        }
        groups
            .into_iter()
            .filter_map(|(owner, objects)| {
                let types: BTreeSet<String> = objects
                    .into_iter()
                    .filter_map(|(object, safe)| safe.then_some(object))
                    .collect();
                (!types.is_empty()).then_some((owner, types))
            })
            .collect()
    }

    /// Callables whose enum result uses the hidden caller destination. This
    /// is the plan-driven replacement for `sret_safe_defns`; only callables
    /// already proven safe/atomic by the enum representation analysis appear.
    pub fn enum_hidden_result_callables(&self) -> BTreeSet<String> {
        self.enum_callables
            .values()
            .filter(|plan| {
                plan.representation == EnumResultRepresentation::CallerDestination
                    && plan.hidden_destination
            })
            .map(|plan| plan.callable.name.clone())
            .collect()
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
    plan.enums = build_enum_definitions(items);
    plan.enum_callables = build_enum_callables(items, legacy, &plan.enums);
    mark_enum_value_shapes(&mut plan.values, &plan.enums);
    plan.objects = build_object_definitions(items);
    plan.object_values = build_object_values(legacy, &plan.objects);
    insert_object_facts(&mut plan.values, &plan.object_values);
    mark_object_value_shapes(&mut plan.values, &plan.objects);
    insert_legacy_value_facts(legacy, &mut plan.values);
    plan.sinks = build_sink_plans(legacy);
    plan.tuples = build_tuple_plans(legacy);
    plan.recursive_carriers = build_recursive_carriers(items, legacy);
    populate_hidden_tail_carriers(&mut plan.callables, &plan.recursive_carriers);
    plan
}

fn build_recursive_carriers(
    items: &[TopLevel],
    legacy: &DestinationOwnershipPlan,
) -> BTreeMap<CallableId, RecursiveCarrierPlan> {
    let legacy_plan = build_tail_carrier_plan(items);
    legacy
        .callables
        .iter()
        .filter_map(|(name, summary)| {
            let info = legacy_plan.info(name)?;
            let callable = callable_id(name, summary.kind);
            let plan = recursive_carrier_plan(&callable, info);
            Some((callable, plan))
        })
        .collect()
}

fn recursive_carrier_plan(
    callable: &CallableId,
    info: &CarrierInfo,
) -> RecursiveCarrierPlan {
    let carriers = info
        .carriers
        .iter()
        .map(|carrier| recursive_carrier_value(callable, carrier))
        .collect();
    RecursiveCarrierPlan {
        callable: callable.clone(),
        carriers,
        proof: ProofStatus::Proven {
            reasons: BTreeSet::from([ProofReason::StableCarrierLifetime]),
        },
    }
}

fn recursive_carrier_value(
    callable: &CallableId,
    carrier: &LegacyCarrierParam,
) -> RecursiveCarrierValue {
    let arg_classes = carrier
        .arg_classes
        .iter()
        .map(|class| match class {
            LegacyCarrierArgClass::InlineNested => RecursiveCarrierArgClass::InlineNested,
            LegacyCarrierArgClass::ExistingCarrier => RecursiveCarrierArgClass::ExistingCarrier,
            LegacyCarrierArgClass::Unsupported => RecursiveCarrierArgClass::Unsupported,
        })
        .collect();
    RecursiveCarrierValue {
        parameter: semantic_value(
            callable,
            SemanticValueKind::Parameter,
            &format!("param/{}/{}", carrier.param_index, carrier.param_index),
            0,
        ),
        param_index: carrier.param_index,
        param_name: carrier.param_name.clone(),
        result_local: carrier.result_local.clone(),
        sret_callee: carrier.sret_callee.clone(),
        arg_classes,
        carrier: DestinationId {
            owner: callable.name.clone(),
            path: format!("recursive/{}/{}", carrier.param_index, carrier.param_name),
        },
        class: GeneralDestinationClass::StableCarrier,
        requires_backedge_copy: true,
        proof: ProofStatus::Proven {
            reasons: BTreeSet::from([ProofReason::StableCarrierLifetime]),
        },
    }
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

/// Attach frontend loop-engine plans to the generalized destination plan.
///
/// `LoopShape` owns carried-field liveness; `LoopMaterializationPlan` owns the
/// Local/Machine split. This adapter records both without re-walking bodies.
pub fn attach_loop_engine_plans(
    plan: &mut GeneralizedDestinationOwnershipPlan,
    shapes: &std::collections::HashMap<String, crate::analysis::loop_shape::LoopShape>,
    materialization: &std::collections::HashMap<
        String,
        crate::analysis::materialization::LoopMaterializationPlan,
    >,
) {
    plan.loop_engines = shapes
        .iter()
        .map(|(node, shape)| {
            let materialized = materialization.get(node);
            let fields = shape
                .carried_fields
                .iter()
                .map(|field| {
                    let storage = materialized
                        .and_then(|plan| plan.fields.get(field))
                        .copied()
                        .unwrap_or_else(|| {
                            if shape.local_fields.contains(field) {
                                crate::analysis::materialization::StorageClass::Local
                            } else {
                                crate::analysis::materialization::StorageClass::Machine
                            }
                        });
                    let (representation, proof) = match storage {
                        crate::analysis::materialization::StorageClass::Local => (
                            LoopFieldRepresentation::Phi,
                            ProofStatus::Proven {
                                reasons: BTreeSet::from([ProofReason::NonEscaping]),
                            },
                        ),
                        _ => (
                            LoopFieldRepresentation::StateCarrier,
                            ProofStatus::Proven {
                                reasons: BTreeSet::from([ProofReason::StableCarrierLifetime]),
                            },
                        ),
                    };
                    (
                        field.clone(),
                        LoopFieldPlan {
                            field: field.clone(),
                            storage,
                            representation,
                            proof,
                        },
                    )
                })
                .collect();
            let engine = LoopEnginePlan {
                node: node.clone(),
                counter: shape.counter.clone(),
                fields,
                proof: ProofStatus::Proven {
                    reasons: BTreeSet::from([ProofReason::StableCarrierLifetime]),
                },
            };
            (node.clone(), engine)
        })
        .collect();
}

/// Reconstruct the backend's current carrier ABI view from the unified plan.
/// D6 migrates the backend to this method so the tail-carrier walker has one
/// consumer and no second source of truth.
pub fn tail_carrier_plan_from_generalized(
    plan: &GeneralizedDestinationOwnershipPlan,
) -> TailCarrierPlan {
    let defns = plan
        .recursive_carriers
        .values()
        .map(|recursive| {
            let carriers = recursive
                .carriers
                .iter()
                .map(|carrier| LegacyCarrierParam {
                    param_index: carrier.param_index,
                    param_name: carrier.param_name.clone(),
                    result_local: carrier.result_local.clone(),
                    sret_callee: carrier.sret_callee.clone(),
                    arg_classes: carrier
                        .arg_classes
                        .iter()
                        .map(|class| match class {
                            RecursiveCarrierArgClass::InlineNested => {
                                LegacyCarrierArgClass::InlineNested
                            }
                            RecursiveCarrierArgClass::ExistingCarrier => {
                                LegacyCarrierArgClass::ExistingCarrier
                            }
                            RecursiveCarrierArgClass::Unsupported => {
                                LegacyCarrierArgClass::Unsupported
                            }
                        })
                        .collect(),
                })
                .collect();
            (
                recursive.callable.name.clone(),
                CarrierInfo { carriers },
            )
        })
        .collect();
    TailCarrierPlan { defns }
}

fn insert_legacy_value_facts(
    legacy: &DestinationOwnershipPlan,
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
    legacy: &DestinationOwnershipPlan,
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

fn destination_sink_kind(
    kind: crate::analysis::destination_ownership::SinkKind,
) -> Option<DestinationSinkKind> {
    match kind {
        crate::analysis::destination_ownership::SinkKind::StateStore => {
            Some(DestinationSinkKind::State)
        }
        crate::analysis::destination_ownership::SinkKind::CollectionInsert => {
            Some(DestinationSinkKind::Collection)
        }
        crate::analysis::destination_ownership::SinkKind::ObjectFieldStore => {
            Some(DestinationSinkKind::ObjectField)
        }
        _ => None,
    }
}

fn build_tuple_plans(
    legacy: &DestinationOwnershipPlan,
) -> BTreeMap<DestinationId, TupleDestinationPlan> {
    legacy
        .callables_by_owner
        .values()
        .flat_map(|facts| facts.values.values())
        .filter(|fact| {
            fact.origins
                .contains(&ValueOrigin::Composite)
        })
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

fn build_object_definitions(items: &[TopLevel]) -> BTreeMap<String, ObjectDefinitionPlan> {
    items
        .iter()
        .filter_map(object_definition)
        .map(|plan| (plan.name.clone(), plan))
        .collect()
}

fn object_definition(item: &TopLevel) -> Option<ObjectDefinitionPlan> {
    match item {
        TopLevel::TypeDef(td) => {
            if td.coll || td.parent.is_some() || td.protocol.is_some() {
                return None;
            }
            let fields: Vec<ObjectFieldPlan> = td
                .body
                .slots
                .iter()
                .filter(|slot| !slot.name.starts_with("__variant_"))
                .map(|slot| ObjectFieldPlan {
                    name: slot.name.clone(),
                    ty: slot.ty.clone(),
                })
                .collect();
            if fields.is_empty() {
                return None;
            }
            Some(ObjectDefinitionPlan {
                name: td.name.clone(),
                type_params: td.type_params.iter().map(|p| p.name.clone()).collect(),
                fields,
            })
        }
        TopLevel::StaticStruct(s) => Some(ObjectDefinitionPlan {
            name: s.name.clone(),
            type_params: s.type_params.iter().map(|p| p.name.clone()).collect(),
            fields: s
                .fields
                .iter()
                .map(|(name, ty)| ObjectFieldPlan {
                    name: name.clone(),
                    ty: ty.clone(),
                })
                .collect(),
        }),
        TopLevel::Export(export) => object_definition(export.inner.as_ref()),
        _ => None,
    }
}

fn build_object_values(
    legacy: &DestinationOwnershipPlan,
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

fn build_enum_definitions(items: &[TopLevel]) -> BTreeMap<String, EnumDefinitionPlan> {
    items
        .iter()
        .filter_map(enum_definition)
        .map(|plan| (plan.name.clone(), plan))
        .collect()
}

fn enum_definition(item: &TopLevel) -> Option<EnumDefinitionPlan> {
    match item {
        TopLevel::Enum(e) => Some(EnumDefinitionPlan {
            name: e.name.clone(),
            type_params: e.type_params.iter().map(|p| p.name.clone()).collect(),
            variants: e.variants.iter().map(enum_variant_plan).collect(),
        }),
        TopLevel::TypeDef(td) => {
            let variants: Vec<EnumVariantPlan> = td
                .body
                .slots
                .iter()
                .filter_map(|slot| {
                    let name = slot.name.strip_prefix("__variant_")?;
                    Some(EnumVariantPlan {
                        name: name.to_string(),
                        payload: (!matches!(slot.ty, Type::Void)).then(|| slot.ty.clone()),
                    })
                })
                .collect();
            if variants.is_empty() {
                return None;
            }
            Some(EnumDefinitionPlan {
                name: td.name.clone(),
                type_params: td.type_params.iter().map(|p| p.name.clone()).collect(),
                variants,
            })
        }
        TopLevel::Export(export) => enum_definition(export.inner.as_ref()),
        _ => None,
    }
}

fn enum_variant_plan(variant: &EnumVariant) -> EnumVariantPlan {
    match variant {
        EnumVariant::Unit(name) => EnumVariantPlan {
            name: name.clone(),
            payload: None,
        },
        EnumVariant::Tuple(name, types) => EnumVariantPlan {
            name: name.clone(),
            payload: enum_payload(types),
        },
        EnumVariant::Struct(name, fields) => {
            let types: Vec<Type> = fields.iter().map(|(_, ty)| ty.clone()).collect();
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
    items: &[TopLevel],
    legacy: &DestinationOwnershipPlan,
    enums: &BTreeMap<String, EnumDefinitionPlan>,
) -> BTreeMap<CallableId, EnumCallablePlan> {
    let representation = crate::analysis::enum_repr::build_enum_representation_plan(items);
    let sret_safe: BTreeSet<String> =
        crate::analysis::enum_repr::sret_safe_defns(items, &representation)
            .into_iter()
            .collect();
    let shapes = callable_shapes(items, legacy);
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
    summary: &crate::analysis::destination_ownership::CallableSummary,
    class: Option<crate::analysis::enum_repr::EnumRepresentation>,
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
    match class.unwrap_or(crate::analysis::enum_repr::EnumRepresentation::Boxed) {
        crate::analysis::enum_repr::EnumRepresentation::ReturnedSmallCandidate
            if sret_safe =>
        {
            EnumCallablePlan {
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
            }
        }
        crate::analysis::enum_repr::EnumRepresentation::ReturnedSmallCandidate => {
            EnumCallablePlan {
                callable: callable.clone(),
                result_type: Some(result_type.clone()),
                representation: EnumResultRepresentation::Boxed,
                agreement: ConstructorAgreement::AllConstructors,
                hidden_destination: false,
                proof: ProofStatus::Conservative {
                    reason: FallbackReason::RecursiveOrCyclic,
                },
            }
        }
        crate::analysis::enum_repr::EnumRepresentation::InlineLocalCandidate => {
            EnumCallablePlan {
                callable: callable.clone(),
                result_type: Some(result_type.clone()),
                representation: EnumResultRepresentation::InlineLocal,
                agreement: ConstructorAgreement::Unknown,
                hidden_destination: false,
                proof: ProofStatus::Proven {
                    reasons: BTreeSet::from([ProofReason::NonEscaping]),
                },
            }
        }
        crate::analysis::enum_repr::EnumRepresentation::Boxed => EnumCallablePlan {
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
    fn enum_constructor_return_selects_caller_destination() {
        let (_, plan) = analyze_items(
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
        assert_eq!(callable.agreement, ConstructorAgreement::AllConstructors);
        assert!(callable.hidden_destination);
        let result_value = plan
            .callables
            .values()
            .find(|plan| plan.callable.name == "make")
            .and_then(|plan| plan.result.value.clone())
            .expect("result value");
        assert_eq!(
            plan.values[&result_value].shape.kind,
            ValueShapeKind::Enum
        );
    }

    #[test]
    fn non_constructor_enum_return_stays_boxed() {
        let (_, plan) = analyze_items(
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
        assert_eq!(callable.agreement, ConstructorAgreement::Unknown);
        assert!(!callable.hidden_destination);
    }

    #[test]
    fn exported_enum_return_keeps_public_boxed_abi() {
        let (_, plan) = analyze_items(
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
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let legacy = crate::analysis::destination_ownership::analyze(&items);
        let representation = crate::analysis::enum_repr::build_enum_representation_plan(&items);
        let expected: BTreeSet<String> =
            crate::analysis::enum_repr::sret_safe_defns(&items, &representation)
                .into_iter()
                .collect();
        assert_eq!(legacy.generalized.enum_hidden_result_callables(), expected);
    }

    #[test]
    fn object_definition_and_escape_are_recorded() {
        let (_, plan) = analyze_items(
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
        let (_, plan) = analyze_items(
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
        assert_eq!(
            value.class,
            GeneralDestinationClass::ScalarReplacedObject
        );
        assert_eq!(
            value.proof,
            ProofStatus::Proven {
                reasons: BTreeSet::from([ProofReason::NonEscaping]),
            }
        );
    }

    #[test]
    fn borrowed_object_argument_is_conservatively_escaping() {
        let (_, plan) = analyze_items(
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
        assert!(escaped
            .escapes
            .contains(&EscapeKind::UnknownCallLifetime));
    }

    #[test]
    fn stack_object_types_require_all_aliases_non_escaping() {
        let (_, local) = analyze_items(
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

        let (_, escaping) = analyze_items(
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
        let (_, plan) = analyze_items(
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
        let (_, plan) = analyze_items(
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
        let src = r#"
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
"#;
        let (_, plan) = analyze_items(src);
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
        assert_eq!(recursive.carriers[0].class, GeneralDestinationClass::StableCarrier);
        let callable_plan = &plan.callables[callable];
        assert_eq!(callable_plan.hidden_abi.tail_carriers, 1);

        let items = {
            let tokens = crate::lexer::tokenize(src).expect("tokenize");
            let mut parser = crate::parser::Parser::new(tokens, src);
            parser.parse_program().expect("parse")
        };
        assert_eq!(
            tail_carrier_plan_from_generalized(&plan),
            crate::analysis::tail_carrier::build_tail_carrier_plan(&items)
        );
    }

    #[test]
    fn loop_engine_plans_join_shape_and_materialization() {
        let src = r#"
let count: Int = 0;
let acc: Int = 0;
let scratch: Int = 0;
node run [count < 4][count == 4] {
    acc = acc + count;
    scratch = count * 2;
    count = count + 1;
};
"#;
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let graph = crate::analysis::transition_graph::ReactorTransitionGraph::build(
            &items,
            &None,
            &vec![],
        );
        let shapes = crate::analysis::loop_shape::build_loop_shapes(&graph, &items, 4);
        let materialization =
            crate::analysis::materialization::build_materialization_plans(&shapes);
        let mut rebuilt = crate::analysis::destination_ownership::analyze(&items).generalized;
        crate::analysis::destination_model::attach_loop_engine_plans(
            &mut rebuilt,
            &shapes,
            &materialization,
        );
        let engine = rebuilt.loop_engines.get("run").expect("loop engine");
        assert_eq!(engine.counter, "count");
        assert!(engine
            .fields
            .values()
            .any(|field| field.representation == LoopFieldRepresentation::StateCarrier));
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
