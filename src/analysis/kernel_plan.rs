//! Target-independent kernel plan + target capability profile.
//!
//! 2026-09-30 (plan `2026-09-30-kernel-plan-and-per-target-lowering.md`,
//! Phase 0): the layer between the proof-carrying frontend facts
//! (`KernelShape`, the reduction/deferred detectors, `GemmPlan`,
//! `GpuSchedule`) and the per-target emitters. One `KernelPlan` is
//! *derived* from proofs that are structural and eternal (Rule 24);
//! each target then **lowers** it by selecting its own instructions.
//! Vendor instructions (`mma.sync`, `cp.async`, `ldmatrix`, coopmat,
//! subgroup ops) are lowerings — never independent codepaths
//! (`abv-gpu-doctrine.md` §4).
//!
//! ISA-neutral by construction (Rule 23): every op is a primitive any
//! lowering can realize — no algorithm names.
//!
//! Serializable (serde) + a stable textual [`KernelPlan::dump`] so plans
//! are inspectable, golden-testable, and usable by the A/B harnesses.

use crate::analysis::gemm_shape::detect_gemm_shape;
use crate::analysis::gpu_strategy::GpuHardware;
use crate::ast::{Expr, TopLevel};
use serde::{Deserialize, Serialize};

/// Which memory an operand lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemSpace {
    Global,
    Shared,
    Register,
}

/// A reference to a buffer region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemRef {
    pub buf: String,
    pub space: MemSpace,
    pub elem_bytes: u32,
}

/// An abstract tensor fragment: a `rows × cols` tile of `elem_bytes`
/// elements plus its lane-ownership class. `cooperative = true` is a
/// workgroup-owned fragment (SPIR-V coopmat); `false` is per-warp
/// (PTX `mma.sync` + `ldmatrix`). The lowering maps a fragment to its
/// target's load/mma primitives — the plan never names an instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FragLayout {
    pub rows: u64,
    pub cols: u64,
    pub elem_bytes: u32,
    pub cooperative: bool,
}

impl FragLayout {
    pub fn new(rows: u64, cols: u64, elem_bytes: u32, cooperative: bool) -> Self {
        Self {
            rows,
            cols,
            elem_bytes,
            cooperative,
        }
    }
}

/// How the work items are claimed. Abstract — the block size / stride is a
/// *lowering* choice (from the target profile), never a plan fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkItem {
    /// One work item owns one unit (block-per-workitem dispatch).
    PerItem,
    /// A flat grid-stride over the items.
    Strided,
}

/// Tiling/pipeline shape selected by the cost model for a target profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanShape {
    pub tile_m: u64,
    pub tile_n: u64,
    pub stages: u64,
}

/// The structural proofs that license the plan's rewrites (Rule 24 — the
/// eternal properties, carried not re-derived).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanProofs {
    /// The work-item counter is proven unique per item → writes are
    /// disjoint across items, no barrier needed for correctness.
    pub disjoint_workitems: bool,
    /// The reduction fold is associative + commutative → it may be split
    /// across CTAs (`ReduceTree::Split`) and recombined.
    pub associative_reduce: bool,
    /// Single-writer: no other statement assigns the reduced target.
    pub single_writer: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReduceOp {
    Add,
    Max,
    /// max → exp-sum → normalize (the deferred/online softmax algebra).
    SoftmaxNormalize,
}

/// Reduction structure. `Split { factor }` is the split-reduction rewrite
/// (5a decode / L3 GEMM split-K / L4 small-K) — licensed by
/// [`PlanProofs::associative_reduce`], one mechanism for every target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReduceTree {
    Linear,
    Split { factor: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    Warp,
    Workgroup,
}

/// An ISA-neutral plan op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanOp {
    /// Define the CTA tile region for subsequent ops.
    Tile {
        region: String,
        tile_m: u64,
        tile_n: u64,
    },
    /// Declare a software pipeline of `depth` stages over the tile ring.
    Stage { depth: u64 },
    /// Asynchronous global→shared copy (cp.async / TMA / subgroup copy).
    AsyncCopy {
        src: MemRef,
        dst: MemRef,
        bytes: u64,
    },
    /// Load a matrix fragment from memory into registers.
    LoadMatrix { src: MemRef, frag: FragLayout },
    /// Multiply-accumulate `acc += a·b`.
    Mma {
        a: FragLayout,
        b: FragLayout,
        acc: FragLayout,
    },
    /// Reduce over a span (resolved length — the plan is post-const-fold).
    Reduce {
        op: ReduceOp,
        span: u64,
        tree: ReduceTree,
        frag: Option<FragLayout>,
    },
    /// Broadcast a value within a scope (warp shuffle / shared).
    Broadcast { src: String, scope: Scope },
    /// A synchronization point within a scope.
    Barrier { scope: Scope },
    /// Store a fragment/result to memory.
    Store { dst: MemRef },
}

/// The target-independent kernel plan for one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelPlan {
    pub node: String,
    pub work: WorkItem,
    pub ops: Vec<PlanOp>,
    pub shape: PlanShape,
    pub proofs: PlanProofs,
}

/// 2026-10-01 (plan §12 item 4): resolve a non-negative expression
/// through the module consts — the shared resolver for plan spans and
/// the split intent (was the `resolve` closure in `from_shape`).
fn resolve_nonneg(e: &Expr, consts: &std::collections::HashMap<String, Expr>) -> Option<u64> {
    match e {
        Expr::Decimal(v) if *v >= 0 => Some(*v as u64),
        Expr::Identifier(n) => match consts.get(n) {
            Some(Expr::Decimal(v)) if *v >= 0 => Some(*v as u64),
            _ => None,
        },
        _ => None,
    }
}

/// Fold a work-item count expression through the module consts.
/// 2026-10-01 (item 4): moved from `backend::ptx` verbatim (the plan's
/// split model needs the same fold; DRY — one implementation). Error
/// strings kept verbatim for diagnostic byte-identity.
pub(crate) fn fold_count(
    shape: &crate::analysis::accel::KernelShape,
    consts: &std::collections::HashMap<String, Expr>,
) -> Result<i64, String> {
    let e = shape.count_expr.clone().unwrap_or(Expr::Decimal(0));
    match e {
        Expr::Decimal(n) => Ok(n),
        Expr::Identifier(s) => match consts.get(&s) {
            Some(Expr::Decimal(n)) => Ok(*n),
            _ => Err(format!("ptx general: count '{}' is not a constant", s)),
        },
        Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, l, r) => {
            let lv = fold_side(&l, consts)?;
            let rv = fold_side(&r, consts)?;
            Ok(lv * rv)
        }
        other => Err(format!(
            "ptx general: count expression {:?} not foldable",
            other
        )),
    }
}

fn fold_side(e: &Expr, consts: &std::collections::HashMap<String, Expr>) -> Result<i64, String> {
    match e {
        Expr::Decimal(n) => Ok(*n),
        Expr::Identifier(s) => match consts.get(s) {
            Some(Expr::Decimal(n)) => Ok(*n),
            _ => Err(format!("ptx general: count operand '{}' is not a constant", s)),
        },
        other => Err(format!("ptx general: count operand {:?} not foldable", other)),
    }
}

/// Can the deferred region `(kv, dim)` be split by `s`? (emitter
/// preconditions: 32-divisible slices, a combine block `dim ≤ 1024`).
/// 2026-10-01 (item 4): moved from `backend::ptx` verbatim — the plan's
/// split intent applies the SAME preconditions the emitter enforces.
pub(crate) fn split_eligible(kv: i64, dim: i64, s: u64) -> bool {
    s > 1
        && dim > 0
        && dim <= 1024
        && dim % 32 == 0
        && kv % s as i64 == 0
        && (kv / s as i64) % 32 == 0
}

/// The declared `<N>`-valued node modifier (D14 shape vocabulary:
/// `tile`/`stage`/`unroll`/`split`/`vector`/`swizzle`/`fragment`).
/// 2026-10-01 (D14 remainder): the ONE parse for every shape modifier —
/// the lowering reads the decision, the analysis owns the declaration.
pub(crate) fn declared_modifier(
    items: &[TopLevel],
    name: &str,
    key: &str,
) -> Option<u64> {
    for item in items {
        match item {
            TopLevel::Transaction(t) if t.name == name => {
                return declared_modifier_on(t, key);
            }
            TopLevel::SyncGroup { item, .. } => {
                if let TopLevel::Transaction(t) = item.as_ref() {
                    if t.name == name {
                        return declared_modifier_on(t, key);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn declared_modifier_on(t: &crate::ast::top::Transaction, key: &str) -> Option<u64> {
    let expr = t
        .modifiers
        .iter()
        .find(|m| m.name == key)
        .and_then(|m| m.value.as_ref())?;
    match expr {
        Expr::Decimal(n) if *n > 0 => Some(*n as u64),
        _ => None,
    }
}

/// The declared `split<N>` reduction-split factor of a node (D1/D2),
/// if any. `N > 1` (1 is no split).
/// 2026-10-01 (item 4): the modifier parse moved here from
/// `backend::ptx::node_split_modifier` so plan construction and the
/// lowering read ONE declaration (DRY, rule 17/18).
pub(crate) fn declared_split_factor(items: &[TopLevel], name: &str) -> Option<u32> {
    // 2026-10-01 (D14 remainder): migrated onto `declared_modifier` —
    // one parse for every shape modifier (rule 17/18).
    declared_modifier(items, name, "split")
        .filter(|n| *n > 1)
        .map(|n| n as u32)
}

/// 2026-10-01 (plan §12 item 4): the split INTENT recorded in the plan —
/// source (`split<N>`) > model (`reduction_split_factor_for`), gated by
/// the deferred proof and the emitter preconditions (`split_eligible`).
/// Config knobs are LOWERING decisions: they may down-select
/// Split → Linear at emission, never up-invent; workspace capacity is a
/// materialization fact and stays at lowering. The plan therefore
/// records what the cost model wants (Rule 2: most efficient default).
/// `resolve` mirrors the plan-span resolver; an unfolded count degrades
/// to `Linear` here — the emitter propagates the same fold error later.
fn deferred_split_tree(
    declared: Option<u32>,
    count: Option<i64>,
    dn: &crate::analysis::accel::DeferredNormalization,
    consts: &std::collections::HashMap<String, Expr>,
    hw: &GpuHardware,
) -> ReduceTree {
    let (Some(count), Some(kv), Some(dim)) = (
        count,
        resolve_nonneg(&dn.reduce_end, consts),
        resolve_nonneg(&dn.normalize_end, consts),
    ) else {
        return ReduceTree::Linear;
    };
    let kv = kv as i64;
    let dim = dim as i64;
    let s = match declared {
        Some(n) => n as u64,
        None => crate::analysis::gpu_strategy::reduction_split_factor_for(
            count as u64,
            kv as u64,
            hw,
        ),
    };
    if split_eligible(kv, dim, s) {
        ReduceTree::Split { factor: s }
    } else {
        ReduceTree::Linear
    }
}

impl KernelPlan {
    /// Derive the plan for an eligible kernel shape (Phase 1).
    ///
    /// This first increment covers the reduction nodes whose span is
    /// analysis-derivable (`ReductionInfo`, both `Dot` and `Softmax`). The
    /// deferred-region span (`softmax_fused!`) is currently detected in the
    /// backend emitter; relocating that fact into analysis (and the GEMM
    /// `Tile`/`Mma` enrichment) is the next Phase-1 increment — see
    /// `docs/plans/2026-09-30-kernel-plan-and-per-target-lowering.md`.
    /// The deferred-softmax lane's plan op (2026-10-03: extracted with the
    /// other lanes to keep `from_shape` within its complexity budget): the
    /// softmax-normalize reduce carrying the split INTENT.
    #[allow(clippy::too_many_arguments)]
    fn deferred_reduce_ops(
        dn: &crate::analysis::accel::DeferredNormalization,
        split_factor: Option<u32>,
        count: Option<i64>,
        consts: &std::collections::HashMap<String, Expr>,
        hw: &GpuHardware,
    ) -> Vec<PlanOp> {
        // The reduction span: the accumulator loop's range end, folded
        // (the same resolution `from_shape`'s `resolve` closure applies).
        let span = resolve_nonneg(&dn.reduce_end, consts).unwrap_or(0);
        vec![
            PlanOp::Reduce {
                op: ReduceOp::SoftmaxNormalize,
                span,
                tree: deferred_split_tree(split_factor, count, dn, consts, hw),
                frag: None,
            },
            PlanOp::Barrier {
                scope: Scope::Workgroup,
            },
        ]
    }

    /// The cooperative channel's plan ops (2026-10-03, the declared dot
    /// rung): the linear reduce + the workgroup barrier.
    fn cooperative_reduce_ops(
        red: &crate::analysis::accel::ReductionInfo,
        span: u64,
    ) -> Vec<PlanOp> {
        use crate::analysis::accel::ReductionKind;
        let op = match red.kind {
            ReductionKind::Dot => ReduceOp::Add,
            ReductionKind::Softmax => ReduceOp::SoftmaxNormalize,
        };
        vec![
            PlanOp::Reduce {
                op,
                span,
                tree: ReduceTree::Linear,
                frag: None,
            },
            PlanOp::Barrier {
                scope: Scope::Workgroup,
            },
        ]
    }

    /// The GEMM lane's plan ops (extracted 2026-10-03 to keep `from_shape`
    /// within its complexity budget): the cost-model tile shape + the
    /// tile/stage/async-copy/fragment/mma/store sequence.
    fn gemm_lane_ops(
        g: &crate::analysis::gemm_shape::GemmShape,
        hw: &GpuHardware,
    ) -> (PlanShape, Vec<PlanOp>) {
        // GEMM: tile + stage + async copies + fragment loads + mma + store.
        // Shape selection goes through the shared cost model.
        let st = crate::analysis::gpu_strategy::select(g.m as u64, g.n as u64, g.k as u64, hw);
        let (tm, tn, stages) = st
            .map(|s| (s.tile_m, s.tile_n, s.stages))
            .unwrap_or((128, 128, 3));
        let plan_shape = PlanShape {
            tile_m: tm,
            tile_n: tn,
            stages,
        };
        let a = MemRef {
            buf: g.a_field.clone(),
            space: MemSpace::Global,
            elem_bytes: 2,
        };
        let b = MemRef {
            buf: g.b_field.clone(),
            space: MemSpace::Global,
            elem_bytes: 2,
        };
        let mut ops: Vec<PlanOp> = Vec::new();
        ops.push(PlanOp::Tile {
            region: g.a_field.clone(),
            tile_m: tm,
            tile_n: tn,
        });
        ops.push(PlanOp::Stage { depth: stages });
        ops.push(PlanOp::AsyncCopy {
            src: a.clone(),
            dst: MemRef {
                buf: format!("{}_smem", g.a_field),
                space: MemSpace::Shared,
                elem_bytes: 2,
            },
            bytes: tm * g.k as u64 * 2,
        });
        ops.push(PlanOp::AsyncCopy {
            src: b.clone(),
            dst: MemRef {
                buf: format!("{}_smem", g.b_field),
                space: MemSpace::Shared,
                elem_bytes: 2,
            },
            bytes: g.k as u64 * tn * 2,
        });
        let fa = FragLayout::new(16, 16, 2, false);
        let fb = FragLayout::new(16, 8, 2, false);
        let fcy = FragLayout::new(16, 16, 4, false);
        ops.push(PlanOp::LoadMatrix { src: a, frag: fa });
        ops.push(PlanOp::LoadMatrix { src: b, frag: fb });
        ops.push(PlanOp::Mma {
            a: fa,
            b: fb,
            acc: fcy,
        });
        ops.push(PlanOp::Store {
            dst: MemRef {
                buf: g.y_field.clone(),
                space: MemSpace::Global,
                elem_bytes: 4,
            },
        });
        (plan_shape, ops)
    }

    pub fn from_shape(
        name: &str,
        shape: &crate::analysis::accel::KernelShape,
        items: &[TopLevel],
        consts: &std::collections::HashMap<String, Expr>,
        hw: &GpuHardware,
    ) -> KernelPlan {
        use crate::analysis::accel::ReductionKind;
        let resolve = |e: &Expr| resolve_nonneg(e, consts);
        let has_deferred = shape.deferred_normalize.is_some();
        let has_reduce = shape.reduction.is_some();
        // 2026-10-03 (declared matmul retirement): the plan records what
        // the compiler will DO — the tensor channel follows the
        // declaration, so an undeclared body's plan has no gemm lane
        // (Rules 23/24).
        let gemm = crate::analysis::gemm_shape::declared_matmul(shape)
            .then(|| detect_gemm_shape(shape, items))
            .flatten();
        let proofs = PlanProofs {
            disjoint_workitems: shape.eligible,
            associative_reduce: has_reduce || has_deferred || gemm.is_some(),
            single_writer: shape.write_buffers.len() <= 1,
        };
        let work = if has_reduce || has_deferred || gemm.is_some() {
            WorkItem::PerItem
        } else {
            WorkItem::Strided
        };
        let mut ops = Vec::new();
        let mut plan_shape = PlanShape {
            tile_m: 1,
            tile_n: 1,
            stages: 1,
        };
        if let Some(g) = &gemm {
            let (gemm_shape, gemm_ops) = Self::gemm_lane_ops(g, hw);
            plan_shape = gemm_shape;
            ops.extend(gemm_ops);
        } else if let Some(dn) = &shape.deferred_normalize {
            // Deferred/softmax region (takes precedence, as in the emitter):
            // the reduction span is the accumulator loop's range end.
            // 2026-10-01 (item 4): the reduce op carries the split
            // INTENT (`ReduceTree::Split`) instead of hardcoded Linear —
            // see `deferred_split_tree`.
            ops.extend(Self::deferred_reduce_ops(
                dn,
                declared_split_factor(items, name),
                fold_count(shape, consts).ok(),
                consts,
                hw,
            ));
        } else if let Some(red) = &shape.reduction {
            // 2026-10-03 (declared dot rung): the plan records the
            // COOPERATIVE channel's intent — which follows the declaration
            // (is_cooperative_shape). An undeclared body's plan records no
            // cooperative reduce op; the general family lowers it.
            if crate::analysis::accel::is_cooperative_shape(shape) {
                ops.extend(Self::cooperative_reduce_ops(
                    red,
                    resolve(&red.inner).unwrap_or(0),
                ));
            }
        }
        if gemm.is_none() {
            if let Some(buf) = shape.write_buffers.first() {
                ops.push(PlanOp::Store {
                    dst: MemRef {
                        buf: buf.clone(),
                        space: MemSpace::Global,
                        elem_bytes: 4,
                    },
                });
            }
        }
        KernelPlan {
            node: name.to_string(),
            work,
            ops,
            shape: plan_shape,
            proofs,
        }
    }

    /// A stable, line-oriented textual dump for golden tests and A/B
    /// harnesses. Format is part of the test contract — extend, don't
    /// reorder.
    /// 2026-10-01 (plan §12 item 4): the plan's split factor —
    /// `Some(S)` when the reduce op carries [`ReduceTree::Split`],
    /// `None` for linear. The lowering consumes this instead of
    /// recomputing the source/model decision (plan as decision record).
    pub fn split_tree_factor(&self) -> Option<u64> {
        self.ops.iter().find_map(|op| match op {
            PlanOp::Reduce {
                tree: ReduceTree::Split { factor },
                ..
            } => Some(*factor),
            _ => None,
        })
    }

    pub fn dump(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("node {}\n", self.node));
        match self.work {
            WorkItem::PerItem => s.push_str("work per_item\n"),
            WorkItem::Strided => s.push_str("work strided\n"),
        }
        s.push_str(&format!(
            "shape tile={}x{} stages={}\n",
            self.shape.tile_m, self.shape.tile_n, self.shape.stages
        ));
        let p = &self.proofs;
        s.push_str(&format!(
            "proofs disjoint={} associative={} single_writer={}\n",
            p.disjoint_workitems, p.associative_reduce, p.single_writer
        ));
        for op in &self.ops {
            s.push_str(&dump_op(op));
            s.push('\n');
        }
        s
    }
}

/// 2026-09-30 (Phase 1 observability): build + dump the plan for every
/// eligible accel node, sorted by name (deterministic). The caller supplies
/// the module consts (frontend helper) and the device profile, so this
/// stays within the analysis layer.
pub fn dump_program_plans(
    items: &[TopLevel],
    entries: &std::collections::HashMap<String, crate::analysis::accel::AccelEntry>,
    consts: &std::collections::HashMap<String, Expr>,
    hw: &GpuHardware,
) -> String {
    let mut names: Vec<&String> = entries
        .iter()
        .filter(|(_, e)| e.shape.eligible)
        .map(|(n, _)| n)
        .collect();
    names.sort();
    let mut out = String::new();
    for n in names {
        let p = KernelPlan::from_shape(n, &entries[n].shape, items, consts, hw);
        out.push_str(&p.dump());
        out.push('\n');
    }
    out
}

fn dump_mem(m: &MemRef) -> String {
    let sp = match m.space {
        MemSpace::Global => "global",
        MemSpace::Shared => "shared",
        MemSpace::Register => "reg",
    };
    format!("{}:{sp}:{}", m.buf, m.elem_bytes)
}

fn dump_frag(f: &FragLayout) -> String {
    format!(
        "{}x{}x{}{}",
        f.rows,
        f.cols,
        f.elem_bytes,
        if f.cooperative { "c" } else { "w" }
    )
}

fn dump_op(op: &PlanOp) -> String {
    match op {
        PlanOp::Tile {
            region,
            tile_m,
            tile_n,
        } => format!("op tile {region} {tile_m}x{tile_n}"),
        PlanOp::Stage { depth } => format!("op stage depth={depth}"),
        PlanOp::AsyncCopy { src, dst, bytes } => format!(
            "op async_copy {} -> {} bytes={bytes}",
            dump_mem(src),
            dump_mem(dst)
        ),
        PlanOp::LoadMatrix { src, frag } => {
            format!("op load_matrix {} {}", dump_mem(src), dump_frag(frag))
        }
        PlanOp::Mma { a, b, acc } => format!(
            "op mma a={} b={} acc={}",
            dump_frag(a),
            dump_frag(b),
            dump_frag(acc)
        ),
        PlanOp::Reduce {
            op,
            span,
            tree,
            frag,
        } => {
            let o = match op {
                ReduceOp::Add => "add",
                ReduceOp::Max => "max",
                ReduceOp::SoftmaxNormalize => "softmax",
            };
            let t = match tree {
                ReduceTree::Linear => "linear".to_string(),
                ReduceTree::Split { factor } => format!("split({factor})"),
            };
            let f = frag.as_ref().map(dump_frag).unwrap_or_else(|| "-".into());
            format!("op reduce {o} span={span} tree={t} frag={f}")
        }
        PlanOp::Broadcast { src, scope } => {
            let sc = match scope {
                Scope::Warp => "warp",
                Scope::Workgroup => "wg",
            };
            format!("op broadcast {src} scope={sc}")
        }
        PlanOp::Barrier { scope } => {
            let sc = match scope {
                Scope::Warp => "warp",
                Scope::Workgroup => "wg",
            };
            format!("op barrier scope={sc}")
        }
        PlanOp::Store { dst } => format!("op store {}", dump_mem(dst)),
    }
}

/// Which lowering a profile describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetKind {
    Ptx,
    Spirv,
}

/// How a target performs asynchronous global→shared copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AsyncKind {
    None,
    /// Ampere+ `cp.async`.
    CpAsync,
    /// Hopper+ TMA bulk copy.
    Tma,
}

/// A target+generation capability profile. Shape selection and the
/// lowerings consume it, so the plan is target-*parameterized* while its
/// structure is shared (`gpu-model.md`, `abv-gpu-doctrine.md` §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetProfile {
    pub kind: TargetKind,
    /// Compute/memory parameters (shared `GpuHardware` — the plan's shape
    /// cost model).
    pub compute: GpuHardware,
    /// Lanes per warp (NVIDIA 32) / subgroup size.
    pub warp_width: u32,
    /// Fragment shapes the target's mma/coopmat primitive accepts.
    pub mma: Vec<FragLayout>,
    pub async_copy: AsyncKind,
    pub ldmatrix: bool,
    /// Workgroup cooperative matrix (SPIR-V `OpCooperativeMatrix*`).
    pub coopmat: bool,
    pub subgroup_size: Option<u32>,
    pub max_grid: [u64; 3],
}

impl TargetProfile {
    /// The PTX/CUDA profile for sm_86 (RTX 3060) — the calibration target.
    pub fn ptx_sm86() -> Self {
        let hw = GpuHardware::SM86;
        Self {
            kind: TargetKind::Ptx,
            compute: hw,
            warp_width: 32,
            mma: vec![
                FragLayout::new(16, 8, 2, false),
                FragLayout::new(16, 16, 2, false),
            ],
            async_copy: AsyncKind::CpAsync,
            ldmatrix: true,
            coopmat: false,
            subgroup_size: None,
            max_grid: [2_147_483_647, 65_535, 65_535],
        }
    }

    /// The portable SPIR-V/Vulkan profile: workgroup cooperative matrices,
    /// no async global→workgroup copy (`gpu-backend-strategy.md` §3.1), no
    /// `ldmatrix` (subgroup ops instead), subgroup size 32.
    pub fn spirv_vulkan() -> Self {
        let hw = GpuHardware::SM86;
        Self {
            kind: TargetKind::Spirv,
            compute: hw,
            warp_width: 32,
            mma: vec![FragLayout::new(16, 16, 2, true)],
            async_copy: AsyncKind::None,
            ldmatrix: false,
            coopmat: true,
            subgroup_size: Some(32),
            max_grid: [65_535, 65_535, 65_535],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plan() -> KernelPlan {
        KernelPlan {
            node: "fattn".into(),
            work: WorkItem::PerItem,
            shape: PlanShape {
                tile_m: 128,
                tile_n: 256,
                stages: 3,
            },
            proofs: PlanProofs {
                disjoint_workitems: true,
                associative_reduce: true,
                single_writer: true,
            },
            ops: vec![
                PlanOp::Tile {
                    region: "k".into(),
                    tile_m: 128,
                    tile_n: 256,
                },
                PlanOp::Reduce {
                    op: ReduceOp::SoftmaxNormalize,
                    span: 4096,
                    tree: ReduceTree::Split { factor: 6 },
                    frag: Some(FragLayout::new(128, 1, 4, false)),
                },
                PlanOp::Barrier {
                    scope: Scope::Workgroup,
                },
                PlanOp::Store {
                    dst: MemRef {
                        buf: "a_out".into(),
                        space: MemSpace::Global,
                        elem_bytes: 4,
                    },
                },
            ],
        }
    }

    #[test]
    fn serde_round_trips() {
        let p = sample_plan();
        let json = serde_json::to_string(&p).expect("serialize");
        let back: KernelPlan = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(p, back, "KernelPlan must round-trip through JSON");
    }

    #[test]
    fn dump_is_stable_and_readable() {
        let d = sample_plan().dump();
        assert!(d.contains("node fattn"), "{d}");
        assert!(d.contains("work per_item"), "{d}");
        assert!(d.contains("shape tile=128x256 stages=3"), "{d}");
        assert!(d.contains("proofs disjoint=true associative=true single_writer=true"), "{d}");
        assert!(d.contains("op tile k 128x256"), "{d}");
        assert!(
            d.contains("op reduce softmax span=4096 tree=split(6) frag=128x1x4w"),
            "{d}"
        );
        assert!(d.contains("op barrier scope=wg"), "{d}");
        assert!(d.contains("op store a_out:global:4"), "{d}");
    }

    #[test]
    fn from_shape_builds_deferred_plan() {
        use crate::analysis::accel::{DeferredNormalization, KernelShape};
        let mut consts = std::collections::HashMap::new();
        consts.insert("NKV".to_string(), Expr::Decimal(4096));
        consts.insert("D".to_string(), Expr::Decimal(128));
        let shape = KernelShape {
            index_var: "h".into(),
            count_expr: Some(Expr::Decimal(32)),
            kernel_stmts: vec![],
            host_stmts: vec![],
            read_buffers: vec!["q".into(), "k".into(), "v".into(), "o1".into()],
            write_buffers: vec!["a_out".into()],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: None,
            deferred_normalize: Some(DeferredNormalization {
                denominator: "l".into(),
                acc_buf: "o1".into(),
                out_buf: "a_out".into(),
                normalize_var: "d".into(),
                normalize_end: Expr::Identifier("D".into()),
                reduce_end: Expr::Identifier("NKV".into()),
            }),
        };
        let d = KernelPlan::from_shape("fattn", &shape, &[], &consts, &GpuHardware::SM86).dump();
        assert!(d.contains("node fattn"), "{d}");
        // 2026-10-01 (item 4): the plan records the split INTENT —
        // no declared modifier, so the model decides: count 32, KV 4096,
        // SM86 (112 CTAs targeted → ceil(112/32) = 4) = split(4).
        assert!(
            d.contains("op reduce softmax span=4096 tree=split(4)"),
            "deferred span + model split intent: {d}"
        );
        assert!(d.contains("op store a_out:global:4"), "{d}");
        let p = KernelPlan::from_shape("fattn", &shape, &[], &consts, &GpuHardware::SM86);
        assert_eq!(p.split_tree_factor(), Some(4), "accessor reads the intent");
    }

    /// 2026-10-01 (item 4): the split intent degrades to Linear when an
    /// emitter precondition fails (dim must be a warp multiple) — the
    /// plan never records a split the emitter could not lower.
    #[test]
    fn from_shape_refuses_ineligible_split() {
        use crate::analysis::accel::{DeferredNormalization, KernelShape};
        let mut consts = std::collections::HashMap::new();
        consts.insert("NKV".to_string(), Expr::Decimal(4096));
        consts.insert("D".to_string(), Expr::Decimal(100));
        let shape = KernelShape {
            index_var: "h".into(),
            count_expr: Some(Expr::Decimal(32)),
            kernel_stmts: vec![],
            host_stmts: vec![],
            read_buffers: vec!["q".into()],
            write_buffers: vec!["a_out".into()],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: None,
            deferred_normalize: Some(DeferredNormalization {
                denominator: "l".into(),
                acc_buf: "o1".into(),
                out_buf: "a_out".into(),
                normalize_var: "d".into(),
                normalize_end: Expr::Identifier("D".into()),
                reduce_end: Expr::Identifier("NKV".into()),
            }),
        };
        let p = KernelPlan::from_shape("fattn", &shape, &[], &consts, &GpuHardware::SM86);
        assert_eq!(p.split_tree_factor(), None, "D=100 is not warp-multiple");
        assert!(p.dump().contains("tree=linear"), "{}", p.dump());
    }

    /// 2026-10-01 (D14 remainder): the ONE modifier parse serves every
    /// D14 shape name — `unroll<N>` reads through the same helper.
    #[test]
    fn declared_modifier_reads_any_shape_name() {
        use crate::ast::{Annotation, Transaction};
        let mk = |mods: Vec<Annotation>| {
            vec![TopLevel::Transaction(Transaction {
                name: "u".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: crate::ast::top::Contract::new(Expr::Decimal(1), Expr::Decimal(1)),
                body: vec![],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                span: None,
                modifiers: mods,
                doc: None,
            })]
        };
        let declared = mk(vec![Annotation {
            name: "unroll".into(),
            value: Some(Expr::Decimal(2)),
        }]);
        assert_eq!(declared_modifier(&declared, "u", "unroll"), Some(2));
        assert_eq!(declared_modifier(&declared, "u", "split"), None);
        assert_eq!(declared_modifier(&declared, "other", "unroll"), None);
        let zero = mk(vec![Annotation {
            name: "unroll".into(),
            value: Some(Expr::Decimal(0)),
        }]);
        assert_eq!(declared_modifier(&zero, "u", "unroll"), None, "N must be > 0");
        let none = mk(vec![]);
        assert_eq!(declared_modifier(&none, "u", "unroll"), None);
    }

    /// 2026-10-01 (item 4): the declared `split<N>` modifier is read by
    /// plan construction (source > model).
    #[test]
    fn declared_split_factor_reads_source() {
        use crate::ast::{Annotation, Transaction};
        let mk = |mods: Vec<Annotation>| {
            vec![TopLevel::Transaction(Transaction {
                name: "red".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: crate::ast::top::Contract::new(Expr::Decimal(1), Expr::Decimal(1)),
                body: vec![],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                span: None,
                modifiers: mods,
                doc: None,
            })]
        };
        let declared = mk(vec![Annotation {
            name: "split".into(),
            value: Some(Expr::Decimal(8)),
        }]);
        assert_eq!(declared_split_factor(&declared, "red"), Some(8));
        assert_eq!(declared_split_factor(&declared, "other"), None);
        let none = mk(vec![]);
        assert_eq!(declared_split_factor(&none, "red"), None);
        let one = mk(vec![Annotation {
            name: "split".into(),
            value: Some(Expr::Decimal(1)),
        }]);
        assert_eq!(declared_split_factor(&one, "red"), None, "1 is no split");
    }

    #[test]
    fn profiles_are_serializable_and_distinct() {
        let ptx = TargetProfile::ptx_sm86();
        let spirv = TargetProfile::spirv_vulkan();
        assert_eq!(ptx.kind, TargetKind::Ptx);
        assert_eq!(spirv.kind, TargetKind::Spirv);
        assert!(ptx.ldmatrix && !spirv.ldmatrix);
        assert!(spirv.coopmat && !ptx.coopmat);
        assert!(spirv.mma.iter().all(|f| f.cooperative));
        assert!(ptx.mma.iter().all(|f| !f.cooperative));
        let json = serde_json::to_string(&ptx).expect("serialize");
        assert!(json.contains("\"Ptx\""), "{json}");
    }

    #[test]
    fn from_shape_builds_reduction_plan() {
        use crate::analysis::accel::{KernelShape, ReductionInfo, ReductionKind};
        let mut consts = std::collections::HashMap::new();
        consts.insert("K".to_string(), Expr::Decimal(4096));
        // 2026-10-03 (declared dot rung): the shape DECLARES — the marker
        // the dot! expansion writes; the twin below stays unmarked.
        let make = || -> KernelShape {
            KernelShape {
                index_var: "i".into(),
                count_expr: Some(Expr::Decimal(64)),
                kernel_stmts: vec![crate::ast::Statement::Let {
                    name: "acc".into(),
                    names: vec![],
                    ty: Some(crate::ast::Type::Custom("Float".into())),
                    expr: Some(Expr::Decimal(0)),
                    modifiers: vec![crate::ast::top::Annotation {
                        name: "declared_composite".into(),
                        value: Some(Expr::Identifier("dot".into())),
                    }],
                }],
                host_stmts: vec![],
                read_buffers: vec!["a".into()],
                write_buffers: vec!["y".into()],
                scalar_ins: vec![],
                eligible: true,
                reasons: vec![],
                work_cols: None,
                reduction: Some(ReductionInfo {
                    inner: Expr::Identifier("K".into()),
                    kind: ReductionKind::Dot,
                    row_buf: "a".into(),
                    col_buf: "x".into(),
                    out_buf: "y".into(),
                }),
                deferred_normalize: None,
            }
        };
        let undeclared = {
            let mut s = make();
            s.kernel_stmts = vec![];
            s
        };
        let pu = KernelPlan::from_shape("dot", &undeclared, &[], &consts, &GpuHardware::SM86);
        assert!(
            !pu.dump().contains("op reduce"),
            "undeclared reduction plans have no cooperative reduce op: {}",
            pu.dump()
        );
        let shape = make();
        let p = KernelPlan::from_shape("dot", &shape, &[], &consts, &GpuHardware::SM86);
        let d = p.dump();
        assert!(d.contains("node dot"), "{d}");
        assert!(d.contains("work per_item"), "{d}");
        assert!(
            d.contains("proofs disjoint=true associative=true single_writer=true"),
            "{d}"
        );
        assert!(d.contains("op reduce add span=4096 tree=linear"), "{d}");
        assert!(d.contains("op barrier scope=wg"), "{d}");
        assert!(d.contains("op store y:global:4"), "{d}");
    }
}
