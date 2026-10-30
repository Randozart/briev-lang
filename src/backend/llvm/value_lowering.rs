// ── Shared value lowering ─────────────────────────────────────────────
// 2026-10-30 (Phase D2, plan
// docs/plans/2026-10-30-generalized-value-destination-ownership.md): one
// contract for representation crossings. This module is deliberately
// backend-facing but not yet called by any emitter. Phase D3+ replaces
// ad-hoc enum/object/call-site lowering with these helpers one lane at a
// time.
//
// The helpers consume the generalized frontend plan; they never infer
// ownership from LLVM registers or Briev type names. Unknown input stays on
// the conservative copy/box path.
//
// Undo: delete this module and its `pub mod` line. No legacy emitter imports
// it yet.

use crate::analysis::destination_model::{
    BoundaryConversion, BoundaryPlan, CallPlan, CallableId, CallablePlan,
    DestinationId, GeneralDestinationClass, GeneralValueFact,
    GeneralizedDestinationOwnershipPlan, LoopPlan, ProgramPoint, ProofReason, ProofStatus,
    SemanticValueId,
};
use std::fmt::Write;

/// Read-only view of the generalized plan for one lowering scope.
///
/// The context borrows the whole program plan and narrows lookups to the
/// current callable, loop, and boundary. Construction is cheap; callers may
/// build one per function or per statement without cloning the plan.
#[derive(Clone, Copy)]
pub struct ValueLoweringContext<'a> {
    plan: &'a GeneralizedDestinationOwnershipPlan,
    callable: Option<&'a CallablePlan>,
    loop_plan: Option<&'a LoopPlan>,
    boundary: Option<&'a BoundaryPlan>,
}

impl<'a> ValueLoweringContext<'a> {
    pub fn new(plan: &'a GeneralizedDestinationOwnershipPlan) -> Self {
        ValueLoweringContext {
            plan,
            callable: None,
            loop_plan: None,
            boundary: None,
        }
    }

    pub fn plan(&self) -> &'a GeneralizedDestinationOwnershipPlan {
        self.plan
    }

    pub fn with_callable(mut self, callable: &CallableId) -> Self {
        self.callable = self.plan.callables.get(callable);
        self
    }

    pub fn with_loop(mut self, loop_id: &DestinationId) -> Self {
        self.loop_plan = self.plan.loops.get(loop_id);
        self
    }

    pub fn with_boundary(mut self, boundary: &DestinationId) -> Self {
        self.boundary = self.plan.boundaries.get(boundary);
        self
    }

    pub fn callable(&self) -> Option<&'a CallablePlan> {
        self.callable
    }

    pub fn loop_plan(&self) -> Option<&'a LoopPlan> {
        self.loop_plan
    }

    pub fn boundary(&self) -> Option<&'a BoundaryPlan> {
        self.boundary
    }

    pub fn value(&self, value: &SemanticValueId) -> Option<&'a GeneralValueFact> {
        self.plan.values.get(value)
    }

    pub fn call_site(&self, point: &ProgramPoint) -> Option<&'a CallPlan> {
        self.plan.call_sites.get(point)
    }

    /// Destination class for a value. Missing facts are `Unknown`, never an
    /// implicit scalar/inline guess.
    pub fn destination_class(&self, value: &SemanticValueId) -> GeneralDestinationClass {
        self.value(value)
            .map(|fact| fact.destination)
            .unwrap_or(GeneralDestinationClass::Unknown)
    }

    /// Proof status for a value. Missing facts are `Unknown`.
    pub fn proof(&self, value: &SemanticValueId) -> ProofStatus {
        self.value(value)
            .map(|fact| fact.proof.clone())
            .unwrap_or(ProofStatus::Unknown)
    }

    /// Decide how a source representation crosses into a destination.
    pub fn conversion(
        &self,
        source: GeneralDestinationClass,
        destination: GeneralDestinationClass,
        proof: &ProofStatus,
    ) -> ConversionPlan {
        plan_conversion(source, destination, proof)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionKind {
    Move,
    Copy,
    MaterializeBox,
    Unbox,
    StoreToCallerDestination,
    CopyToStableCarrier,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversionPlan {
    pub source: GeneralDestinationClass,
    pub destination: GeneralDestinationClass,
    pub kind: ConversionKind,
    pub requires_allocation: bool,
    pub requires_copy: bool,
    pub proof: ProofStatus,
}

/// Pure conversion decision. Emitters consume the decision; they do not
/// reinterpret ownership.
pub fn plan_conversion(
    source: GeneralDestinationClass,
    destination: GeneralDestinationClass,
    proof: &ProofStatus,
) -> ConversionPlan {
    let proven = is_proven(proof);
    let same_class = source == destination;
    let kind = match (source, destination) {
        (GeneralDestinationClass::Boxed, GeneralDestinationClass::ScalarValue)
        | (GeneralDestinationClass::Boxed, GeneralDestinationClass::CallerDestination) => {
            ConversionKind::Unbox
        }
        (_, GeneralDestinationClass::Boxed) if !same_class => ConversionKind::MaterializeBox,
        (GeneralDestinationClass::Unknown, _) | (_, GeneralDestinationClass::Unknown) => {
            ConversionKind::Copy
        }
        _ if same_class && proven => ConversionKind::Move,
        (GeneralDestinationClass::ScalarValue, GeneralDestinationClass::CallerDestination)
            if proven => ConversionKind::StoreToCallerDestination,
        (_, GeneralDestinationClass::StableCarrier) if !same_class => {
            ConversionKind::CopyToStableCarrier
        }
        _ => ConversionKind::Copy,
    };
    let requires_allocation = matches!(kind, ConversionKind::MaterializeBox);
    let requires_copy = matches!(
        kind,
        ConversionKind::Copy
            | ConversionKind::MaterializeBox
            | ConversionKind::Unbox
            | ConversionKind::CopyToStableCarrier
    );
    ConversionPlan {
        source,
        destination,
        kind,
        requires_allocation,
        requires_copy,
        proof: proof.clone(),
    }
}

fn is_proven(proof: &ProofStatus) -> bool {
    matches!(proof, ProofStatus::Proven { .. })
}

/// Convert a frontend boundary plan into the same lowering decision shape as
/// an internal value crossing. The boundary plan owns the choice; this helper
/// only maps it without adding heuristics.
pub fn plan_boundary_conversion(boundary: &BoundaryPlan) -> ConversionPlan {
    let kind = match boundary.conversion {
        BoundaryConversion::None | BoundaryConversion::ZeroCopyWithLifetimeProof => {
            ConversionKind::Move
        }
        BoundaryConversion::BoxAtBoundary => ConversionKind::MaterializeBox,
        BoundaryConversion::UnboxAtBoundary => ConversionKind::Unbox,
        BoundaryConversion::CopyAtBoundary | BoundaryConversion::WidthConvert => {
            ConversionKind::Copy
        }
    };
    ConversionPlan {
        source: boundary.internal_class,
        destination: boundary.external_class,
        kind,
        requires_allocation: kind == ConversionKind::MaterializeBox,
        requires_copy: matches!(
            kind,
            ConversionKind::Copy | ConversionKind::Unbox | ConversionKind::MaterializeBox
        ),
        proof: boundary.proof.clone(),
    }
}

/// One destination memory image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRange<'a> {
    pub pointer: &'a str,
    pub bytes: u64,
    pub alignment: u32,
}

/// Emit the exact copy used by a copy-bearing conversion.
///
/// This is the single memory-copy spelling for value lowering. It is a raw
/// operand helper: callers supply already-resolved destination/source pointers
/// and the byte count from layout facts. No ownership proof is re-derived here.
pub fn emit_copy_to_destination(
    out: &mut String,
    indent: &str,
    destination: MemoryRange<'_>,
    source: MemoryRange<'_>,
) {
    debug_assert_eq!(destination.bytes, source.bytes);
    let _ = writeln!(
        out,
        "{indent}call void @llvm.memcpy.p0.p0.i64(ptr align {} {}, ptr align {} {}, i64 {}, i1 false)",
        destination.alignment,
        destination.pointer,
        source.alignment,
        source.pointer,
        destination.bytes
    );
}

/// Emit zero-fill for a fresh destination image.
pub fn emit_zero_destination(
    out: &mut String,
    indent: &str,
    destination: MemoryRange<'_>,
) {
    let bytes = destination.bytes.to_string();
    let _ = writeln!(
        out,
        "{indent}call void @llvm.memset.p0.i64(ptr align {} {}, i8 0, i64 {}, i1 false)",
        destination.alignment, destination.pointer, bytes
    );
}

/// Emit a conversion that needs only an in-place move. Returns `false` when
/// the plan requires allocation or a representation change, so callers cannot
/// accidentally treat a copy as a move.
pub fn emit_move_if_exact(plan: &ConversionPlan, out: &mut String, indent: &str) -> bool {
    if plan.kind != ConversionKind::Move {
        return false;
    }
    let _ = writeln!(out, "{indent}; move: destination already owns the image");
    true
}

pub fn proof_has(proof: &ProofStatus, reason: ProofReason) -> bool {
    matches!(proof, ProofStatus::Proven { reasons } if reasons.contains(&reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::destination_model::{
        SemanticValueKind, ValueShape, ValueShapeKind,
    };
    use crate::ast::Type;
    use std::collections::BTreeSet;

    fn proven() -> ProofStatus {
        ProofStatus::Proven {
            reasons: BTreeSet::from([ProofReason::NonOverlapping]),
        }
    }

    #[test]
    fn context_reads_conservative_value_facts() {
        let mut plan = GeneralizedDestinationOwnershipPlan::default();
        let value = SemanticValueId {
            callable: CallableId {
                namespace: crate::analysis::destination_model::CallableNamespace::Definition,
                name: "f".to_string(),
            },
            kind: SemanticValueKind::Local,
            path: "stmt/0".to_string(),
            incarnation: 0,
        };
        plan.values.insert(
            value.clone(),
            GeneralValueFact {
                id: value.clone(),
                ty: None,
                shape: ValueShape {
                    kind: ValueShapeKind::Unknown,
                    ty: Type::Void,
                    payload_words: None,
                    inline_words: None,
                    boxed_fallback: true,
                    recursive: false,
                    external_abi: false,
                },
                ownership: crate::analysis::destination_model::OwnershipClass::Unknown,
                destination: GeneralDestinationClass::Boxed,
                may_overlap: true,
                proof: ProofStatus::Unknown,
            },
        );
        let context = ValueLoweringContext::new(&plan);
        assert_eq!(context.destination_class(&value), GeneralDestinationClass::Boxed);
        assert_eq!(context.proof(&value), ProofStatus::Unknown);
        let missing = SemanticValueId { path: "missing".to_string(), ..value };
        assert_eq!(context.destination_class(&missing), GeneralDestinationClass::Unknown);
    }

    #[test]
    fn unknown_ownership_forces_copy() {
        let decision = plan_conversion(
            GeneralDestinationClass::ScalarValue,
            GeneralDestinationClass::ScalarValue,
            &ProofStatus::Unknown,
        );
        assert_eq!(decision.kind, ConversionKind::Copy);
        assert!(decision.requires_copy);
        assert!(!decision.requires_allocation);
    }

    #[test]
    fn proven_same_class_is_move() {
        let decision = plan_conversion(
            GeneralDestinationClass::CallerDestination,
            GeneralDestinationClass::CallerDestination,
            &proven(),
        );
        assert_eq!(decision.kind, ConversionKind::Move);
        assert!(!decision.requires_copy);
    }

    #[test]
    fn boxed_destination_allocates() {
        let decision = plan_conversion(
            GeneralDestinationClass::CallerDestination,
            GeneralDestinationClass::Boxed,
            &proven(),
        );
        assert_eq!(decision.kind, ConversionKind::MaterializeBox);
        assert!(decision.requires_allocation);
        assert!(decision.requires_copy);
    }

    #[test]
    fn boxed_source_unboxes_when_consumed() {
        let decision = plan_conversion(
            GeneralDestinationClass::Boxed,
            GeneralDestinationClass::CallerDestination,
            &proven(),
        );
        assert_eq!(decision.kind, ConversionKind::Unbox);
        assert!(decision.requires_copy);
    }

    #[test]
    fn boundary_plan_maps_to_box_conversion() {
        let boundary = BoundaryPlan {
            boundary: DestinationId {
                owner: "f".to_string(),
                path: "boundary/export".to_string(),
            },
            kind: crate::analysis::destination_model::BoundaryKind::Export,
            internal_class: GeneralDestinationClass::CallerDestination,
            external_class: GeneralDestinationClass::Boxed,
            conversion: BoundaryConversion::BoxAtBoundary,
            ownership: crate::analysis::destination_model::OwnershipClass::Unknown,
            proof: ProofStatus::Conservative {
                reason: crate::analysis::destination_model::FallbackReason::ExternalBoundary,
            },
        };
        let conversion = plan_boundary_conversion(&boundary);
        assert_eq!(conversion.kind, ConversionKind::MaterializeBox);
        assert!(conversion.requires_allocation);
        assert!(conversion.requires_copy);
    }

    #[test]
    fn copy_helper_emits_exact_memcpy() {
        let mut out = String::new();
        emit_copy_to_destination(
            &mut out,
            "  ",
            MemoryRange { pointer: "%dst", bytes: 24, alignment: 8 },
            MemoryRange { pointer: "%src", bytes: 24, alignment: 8 },
        );
        assert_eq!(
            out,
            "  call void @llvm.memcpy.p0.p0.i64(ptr align 8 %dst, ptr align 8 %src, i64 24, i1 false)\n"
        );
    }

    #[test]
    fn zero_helper_emits_exact_memset() {
        let mut out = String::new();
        emit_zero_destination(
            &mut out,
            "",
            MemoryRange { pointer: "%dst", bytes: 16, alignment: 8 },
        );
        assert_eq!(
            out,
            "call void @llvm.memset.p0.i64(ptr align 8 %dst, i8 0, i64 16, i1 false)\n"
        );
    }

    #[test]
    fn move_helper_refuses_copy_decision() {
        let decision = plan_conversion(
            GeneralDestinationClass::Unknown,
            GeneralDestinationClass::Unknown,
            &proven(),
        );
        let mut out = String::new();
        assert!(!emit_move_if_exact(&decision, &mut out, ""));
        assert!(out.is_empty());
    }
}
