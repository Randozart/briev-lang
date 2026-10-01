// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Per-target lowering seam (KernelPlan Phase 2, plan
//! `2026-09-30-kernel-plan-and-per-target-lowering.md` §7).
//!
//! Strangler: [`GpuLowering`] is the trait each target implements over a
//! [`KernelPlan`]; the first adapter — [`PtxGeneralLowering`] — routes the
//! general (non-tensor) family through the existing PTX emitters with the
//! plan as the decision record and its proofs as the emission gate. The
//! legacy inline path in `backend::ptx::build_ptx_kernels` stays the
//! default (`ptx_plan_lowering: 0`) until parity licenses the flip; the
//! parity test (`plan_lowering_parity_with_legacy`) pins the two arms
//! node-for-node, blob-for-blob.
//!
//! Contract (B) — the runner/desc projection — will narrow [`KernelBlob`]
//! to the §7 `{name, domain, bytes, geometry}` shape and carry
//! `LoweredNode::dispatch`; until then the runner kernel is the honest
//! currency of both arms. The tensor family (op `Mma`) lowers through a
//! sibling adapter once the general arm proves out; see the plan's §12
//! sequencing.

use crate::analysis::accel::KernelShape;
use crate::analysis::kernel_plan::{KernelPlan, TargetKind, TargetProfile, PlanOp};
use crate::ast::TopLevel;
use crate::backend::spirv::runner::{RunnerKernel, SsboLayout};
use crate::type_universe::TypeUniverse;

/// A lowered kernel as the runner consumes it today. Interim: the full
/// [`RunnerKernel`]; contract (B) narrows this to the §7
/// `{name, domain, bytes, geometry}` blob.
pub type KernelBlob = RunnerKernel;

/// Everything one general-family node's lowering needs from the legacy
/// emitters. The plan is ISA-neutral by design and carries no AST — the
/// material (shape, program, layout) travels beside it; the plan supplies
/// the decision record and the proofs.
pub struct GeneralNodeCtx<'a> {
    pub name: String,
    pub shape: &'a KernelShape,
    pub program: &'a [TopLevel],
    pub layout: &'a SsboLayout,
    pub universe: &'a TypeUniverse,
    pub int_bits: u64,
    pub irr_free: bool,
}

/// The lowering result for one node: emitted kernels plus any size
/// warnings (D28) the lowering produced.
pub struct LoweredNode {
    pub kernels: Vec<KernelBlob>,
    pub warnings: Vec<String>,
}

/// §7: one lowering per target. Implementations own their material
/// (strangler: the legacy emitters still need AST/shape) and consume the
/// plan through `lower`.
pub trait GpuLowering {
    fn target(&self) -> TargetKind;

    fn lower(&self, plan: &KernelPlan, p: &TargetProfile) -> Result<LoweredNode, String>;
}

/// 2026-10-01: the pre-flight contract shared by every lowering — the
/// profile must BE the target, and the general PTX family emits 32-wide
/// warp-synchronous code (lane reductions, warp slices), so a profile
/// declaring a different warp width is out of surface for it.
/// Capabilities: declare before emitting.
fn check_profile(plan: &KernelPlan, kind: TargetKind, p: &TargetProfile) -> Result<(), String> {
    if p.kind != kind {
        return Err(format!(
            "plan '{}' targets a different lane — Fix: lower it through the lowering whose target matches.",
            plan.node
        ));
    }
    if p.warp_width != 32 {
        return Err(format!(
            "plan '{}' needs 32-wide warps (the general PTX family emits warp-synchronous code) but the profile declares {} — Fix: use a warp-32 profile.",
            plan.node, p.warp_width
        ));
    }
    Ok(())
}

/// The family contract: the general lowering realizes elementwise, loop,
/// and reduction plans — matrix multiplication lowers through the tensor
/// adapter. Rejecting here (before any emission material is touched) is
/// "declare before emitting" applied to families.
fn check_family(plan: &KernelPlan) -> Result<(), String> {
    if plan.ops.iter().any(|o| matches!(o, PlanOp::Mma { .. })) {
        return Err(format!(
            "plan '{}' carries matrix ops — the general lowering does not lower the tensor family — Fix: route it to the tensor lowering.",
            plan.node
        ));
    }
    Ok(())
}

/// PTX lowering for the general (non-tensor) family: elementwise/loop
/// kernels, reductions, and deferred-normalize regions.
pub struct PtxGeneralLowering<'a> {
    pub ctx: GeneralNodeCtx<'a>,
}

impl GpuLowering for PtxGeneralLowering<'_> {
    fn target(&self) -> TargetKind {
        TargetKind::Ptx
    }

    fn lower(&self, plan: &KernelPlan, p: &TargetProfile) -> Result<LoweredNode, String> {
        check_profile(plan, TargetKind::Ptx, p)?;
        check_family(plan)?;
        let (kernels, warnings) = crate::backend::ptx::emit_general_node(&self.ctx, Some(plan))?;
        Ok(LoweredNode {
            kernels,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::kernel_plan::{
        FragLayout, PlanProofs, PlanShape, ReduceOp, ReduceTree, WorkItem,
    };

    fn plan(node: &str) -> KernelPlan {
        KernelPlan {
            node: node.to_string(),
            work: WorkItem::PerItem,
            ops: vec![PlanOp::Reduce {
                op: ReduceOp::SoftmaxNormalize,
                span: 256,
                tree: ReduceTree::Linear,
                frag: None,
            }],
            shape: PlanShape {
                tile_m: 1,
                tile_n: 1,
                stages: 1,
            },
            proofs: PlanProofs {
                disjoint_workitems: true,
                associative_reduce: true,
                single_writer: true,
            },
        }
    }

    #[test]
    fn check_profile_rejects_wrong_kind_and_warp_width() {
        let p = TargetProfile::ptx_sm86();
        assert!(check_profile(&plan("n"), TargetKind::Ptx, &p).is_ok());
        let mut wrong = TargetProfile::ptx_sm86();
        wrong.kind = TargetKind::Spirv;
        let e = check_profile(&plan("n"), TargetKind::Ptx, &wrong).unwrap_err();
        assert!(e.contains("different lane"), "{e}");
        let mut wide = TargetProfile::ptx_sm86();
        wide.warp_width = 64;
        let e = check_profile(&plan("n"), TargetKind::Ptx, &wide).unwrap_err();
        assert!(e.contains("32-wide warps") && e.contains("64"), "{e}");
    }

    #[test]
    fn general_lowering_refuses_matrix_plans() {
        let mut p = plan("g");
        p.ops.push(PlanOp::Mma {
            a: FragLayout::new(16, 16, 2, false),
            b: FragLayout::new(16, 8, 2, false),
            acc: FragLayout::new(16, 16, 4, false),
        });
        let e = check_family(&p).unwrap_err();
        assert!(e.contains("tensor family") && e.contains("Fix:"), "{e}");
        assert!(check_family(&plan("g")).is_ok());
    }
}
