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
//! currency of both arms. First increment landed 2026-10-01: the `split`
//! field is gone, its knowledge in the `geometry` slot
//! (`spirv::runner::DispatchGeometry`). PTX has one adapter per family —
//! [`PtxGeneralLowering`] (flag-routed via `ptx_plan_lowering`) and
//! [`PtxTensorLowering`] (the `GemmPlan` branch) — because the PTX
//! builder routes families through separate emission paths; the SPIR-V
//! lane's single adapter follows with its own knob. See the plan's §12
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
/// profile must BE the target, and the PTX/SPIR-V families this seam
/// serves emit 32-wide warp-synchronous code (lane reductions, warp
/// slices, subgroup ops), so a profile declaring a different warp width
/// is out of surface for them. Capabilities: declare before emitting.
fn check_profile(plan: &KernelPlan, kind: TargetKind, p: &TargetProfile) -> Result<(), String> {
    if p.kind != kind {
        return Err(format!(
            "plan '{}' targets a different lane — Fix: lower it through the lowering whose target matches.",
            plan.node
        ));
    }
    if p.warp_width != 32 {
        return Err(format!(
            "plan '{}' needs 32-wide warps (the PTX/SPIR-V families emit warp-synchronous code) but the profile declares {} — Fix: use a warp-32 profile.",
            plan.node, p.warp_width
        ));
    }
    Ok(())
}

/// Plan admission for every strangler arm (2026-10-01, Phase 2.4): a
/// `Some(plan)` must BE this node's plan and must carry the
/// disjoint-work-item proof — emission never starts without both. `None`
/// is the legacy arm (flag 0): no plan, no gates.
pub(crate) fn admission_gates(
    name: &str,
    plan: Option<&KernelPlan>,
) -> Result<(), String> {
    let Some(plan) = plan else {
        return Ok(());
    };
    if plan.node != name {
        return Err(format!(
            "node '{}': plan is for node '{}' — Fix: build the plan from this node's shape.",
            name, plan.node
        ));
    }
    if !plan.proofs.disjoint_workitems {
        return Err(format!(
            "node '{}': the plan carries no disjoint-work-item proof (overlapping work items would race) — Fix: make the iteration disjoint.",
            name
        ));
    }
    Ok(())
}

/// The family contract: each adapter realizes exactly one family — the
/// general lowering refuses matrix plans, the tensor lowering refuses
/// plans without them. Rejecting here (before any emission material is
/// touched) is "declare before emitting" applied to families.
fn check_family(plan: &KernelPlan, tensor: bool) -> Result<(), String> {
    let has_mma = plan.ops.iter().any(|o| matches!(o, PlanOp::Mma { .. }));
    if tensor && !has_mma {
        return Err(format!(
            "plan '{}' carries no matrix ops — the tensor lowering only lowers the GEMM family — Fix: route it to the general lowering.",
            plan.node
        ));
    }
    if !tensor && has_mma {
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
        check_family(plan, false)?;
        let (kernels, warnings) = crate::backend::ptx::emit_general_node(&self.ctx, Some(plan))?;
        Ok(LoweredNode {
            kernels,
            warnings,
        })
    }
}

/// Everything the PTX tensor-family emission needs beside the plan —
/// same rule as [`GeneralNodeCtx`]: the plan is ISA-neutral, the material
/// travels with it. (`int_bits` is not part of this family: the tensor
/// tier's emitters take fixed-width fragment types.)
pub struct GemmNodeCtx<'a> {
    pub name: String,
    pub shape: &'a KernelShape,
    pub program: &'a [TopLevel],
    pub layout: &'a SsboLayout,
    pub universe: &'a TypeUniverse,
    pub schedule: &'a crate::analysis::gpu_schedule::GpuSchedule,
    pub irr_free: bool,
}

/// PTX lowering for the tensor (GEMM) family: the naive tier and the
/// mma/`mma.sync` tier behind `ptx::emit_gemm_node`. Emits exactly one
/// kernel per node (epilogue fusions folded in by the schedule).
pub struct PtxTensorLowering<'a> {
    pub ctx: GemmNodeCtx<'a>,
}

impl GpuLowering for PtxTensorLowering<'_> {
    fn target(&self) -> TargetKind {
        TargetKind::Ptx
    }

    fn lower(&self, plan: &KernelPlan, p: &TargetProfile) -> Result<LoweredNode, String> {
        check_profile(plan, TargetKind::Ptx, p)?;
        check_family(plan, true)?;
        // The legacy branch already routed on `GemmPlan::match_stmts` —
        // the same structural matcher `from_shape` enriches from, so the
        // derivation cannot fail here; Err only on a plan/shape mismatch.
        let gemm = crate::backend::spirv::gemm::GemmPlan::match_stmts(self.ctx.shape, self.ctx.program)
            .ok_or_else(|| {
                format!(
                    "plan '{}' claims matrix ops but the node's body does not match the GEMM structure — Fix: rebuild the plan from this node's shape.",
                    plan.node
                )
            })?;
        let (kernel, warnings) = crate::backend::ptx::emit_gemm_node(&self.ctx, gemm, Some(plan))?;
        Ok(LoweredNode {
            kernels: vec![kernel],
            warnings,
        })
    }
}

/// Everything the SPIR-V lowering needs beside the plan. The SPIR-V
/// lane has ONE emission hook for every eligible node (`spirv::kernel::
/// emit_kernel` — cooperative/tiled/tensor are flags inside it), so
/// there is one adapter and no family gate: `kplans`/`reuse_map` are the
/// image/alias material the hook binds.
pub struct SpirvNodeCtx<'a> {
    pub name: String,
    pub shape: &'a KernelShape,
    pub program: &'a [TopLevel],
    pub universe: &'a TypeUniverse,
    pub kplans: Vec<crate::analysis::image_storage::ImageStoragePlan>,
    pub reuse_map: Option<&'a std::collections::HashMap<String, String>>,
    pub int_bits: u64,
}

/// SPIR-V lowering for every eligible node — one adapter, one emission
/// hook (the mirror of the PTX lane's two-family split; plan §12 item 5).
pub struct SpirvLowering<'a> {
    pub ctx: SpirvNodeCtx<'a>,
}

impl GpuLowering for SpirvLowering<'_> {
    fn target(&self) -> TargetKind {
        TargetKind::Spirv
    }

    /// 2026-10-01 (plan §12 item 4): capability note — a plan whose
    /// reduce op carries `ReduceTree::Split` lowers LINEARLY on this
    /// lane: SPIR-V emission does not yet realize the partial/combine
    /// combine split (the PTX `dispatch_with_split` has no SPIR-V
    /// analogue). The intent is recorded, admission/gates run, but the
    /// emission degrades to one full-image kernel — a capability gap,
    /// never a silent semantic change. Closing it (a SPIR-V split
    /// lowering) is the same general-lowering work, not a new codepath.
    fn lower(&self, plan: &KernelPlan, p: &TargetProfile) -> Result<LoweredNode, String> {
        check_profile(plan, TargetKind::Spirv, p)?;
        let kernel = crate::backend::spirv::runner::emit_node_kernel(&self.ctx, Some(plan))?;
        Ok(LoweredNode {
            kernels: vec![kernel],
            warnings: Vec::new(),
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
        let e = check_family(&p, false).unwrap_err();
        assert!(e.contains("tensor family") && e.contains("Fix:"), "{e}");
        assert!(check_family(&plan("g"), false).is_ok());
        // The tensor lowering is the mirror image: it refuses plans
        // without matrix ops.
        let e = check_family(&plan("g"), true).unwrap_err();
        assert!(e.contains("no matrix ops") && e.contains("general lowering"), "{e}");
        assert!(check_family(&p, true).is_ok());
    }
}
