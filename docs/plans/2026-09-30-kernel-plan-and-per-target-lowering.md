# Target-Independent `KernelPlan` + Per-Target Lowering (2026-09-30)

**Status:** active. Phase 0 in progress.
**Doctrine:** `docs/architecture/abv-gpu-doctrine.md` §4 (per-vendor
*projections* of one plan), `proof-vs-shape.md` (Rule 24: the plan is the
general machinery; shapes are temporal), `backend-contracts.md`
(analysis-once → plan-once), `gpu-backend-strategy.md` (route evaluation),
and **`docs/architecture/derivation-not-recognition.md`** — the governing
obligation: the programmer declares intent, the compiler **derives**, and
**recognition is the cardinal sin**.

## 1. Goal and refined thesis

There is a target-independent kernel **shape**: a GEMM is a tiling +
staging pipeline + accumulate; a softmax is a reduction tree; a decode is
a split reduction. Briev's contracts (disjointness, associativity,
independence, linearity) are what let the compiler *prove* a shape legal
and *rewrite* it — which an opaque thread model cannot. Vendor
instructions (`mma.sync`, `cp.async`, `ldmatrix`, `wgmma`, `tcgen05`,
TMA; coopmat/subgroup ops) are **lowerings of canonical ops**, never
independent codepaths.

Refinement: the *shape space* and the *rewrite rules* are
target-independent; *shape selection* and *instruction selection* are
functions of a **target capability profile**. So: one intent → prove →
select shape per capability → lower per target.

## 2. Non-goals

- Do NOT detach the frontends (doctrine §4 forbids independent codepaths).
- `KernelPlan` is compiler-internal, never author-visible.
- CPU/LLVM backends are future lowerings (Section 12); this plan covers
  GPU (PTX + SPIR-V) first.
- No vocabulary (Rule 23): ops are ISA primitives, never algorithm names.
  The Rule 24 review applies to every op added.

## 3. Current state (evidence)

| Layer | Today | Where |
|---|---|---|
| Structure facts | `KernelShape`, `ReductionInfo`, `DeferredNormalization` | `analysis/accel.rs:88,130,169` |
| | `GemmPlan`, `GpuSchedule`/`ChainFusion`/`Fusion` | `backend/spirv/gemm.rs:39`; `analysis/gpu_schedule.rs:23,121` |
| Shape selection | `gpu_strategy::{select, Strategy, GpuHardware}` — **PTX lane only** | `analysis/gpu_strategy.rs:18,52` |
| Emitters | **independent codepaths re-deriving from shape** | `spirv/kernel.rs:36`, `spirv/gemm.rs`, `ptx/general.rs`, `ptx/tensor.rs` |
| Kernel list + runner | one merged list, one `RunnerKernel` per node, two blobs | `compile.rs:1740-1795`; `spirv/runner.rs:46` |
| Runtime | `BrievKernelDesc` (dual blob), dispatch geometry | `accel_rt.rs:62`; `spirv/runner.rs:1150-1235` |
| Capability surface | `BackendCapabilities` (48 flags) + `CAPABILITIES` | `backend/capabilities.rs`; `spirv/mod.rs:39` |

**Gap:** no shared kernel IR; the emitters disagree by construction.
Observed consequences: tensor dual-image IMA (`BUGS.md` 2026-09-30),
Vulkan ignoring `gpu_strategy`, the split needing per-lane hand-writing.

## 4. Architecture

```
frontend facts/proofs      (target-independent)  KernelShape, detectors, GpuSchedule, proofs
      │
      ▼  plan construction
KernelPlan                 (structure + capability-parameterized shape)
      │
      ▼  Lowering impl (instruction selection + dispatch geometry)
LoweredNode { kernels: [KernelBlob], dispatch: DispatchProgram }   PTX | SPIR-V | …
      │
      ▼  runner / desc / runtime (per-lane kernel sets)
```

## 5. Phase 0 — the `KernelPlan` IR (this commit)

`src/analysis/kernel_plan.rs`. ISA-neutral ops, attached proofs, explicit
fragment layouts, and a `TargetProfile`. **Serializable** (serde) plus a
stable textual `dump()` for golden tests and A/B harnesses.

```rust
pub struct KernelPlan {
    pub node: String,
    pub work: WorkItem,
    pub ops: Vec<PlanOp>,
    pub shape: PlanShape,
    pub proofs: PlanProofs,
}
pub enum WorkItem { Block { threads: u32 }, ThreadStrided { stride: u32 } }
pub struct PlanShape { pub tile_m: u64, pub tile_n: u64, pub stages: u64 }
pub struct PlanProofs {
    pub disjoint_workitems: bool,   // counter proven unique per item
    pub associative_reduce: bool,   // fold merge is assoc/comm
    pub single_writer: bool,
}

pub struct MemRef { pub buf: String, pub space: MemSpace, pub elem_bytes: u32 }
pub enum MemSpace { Global, Shared, Register }

/// Explicit fragment-layout lattice (not deferred): a fragment is a
/// (rows, cols, elem) tile with a per-lane ownership rule. The lowering
/// maps this to ldmatrix / coopmat / subgroup ops.
pub struct FragLayout { pub rows: u64, pub cols: u64, pub elem_bytes: u32,
                        pub cooperative: bool }   // workgroup (coopmat) vs warp (mma)

pub enum PlanOp {
    Tile       { region: String, tile_m: u64, tile_n: u64 },
    Stage      { depth: u64 },
    AsyncCopy  { src: MemRef, dst: MemRef, bytes: u64 },
    LoadMatrix { src: MemRef, frag: FragLayout },
    Mma        { a: FragLayout, b: FragLayout, acc: FragLayout },
    Reduce     { op: ReduceOp, span: Expr, tree: ReduceTree, frag: Option<FragLayout> },
    Broadcast  { src: Expr, scope: Scope },
    Barrier    { scope: Scope },
    Store      { dst: MemRef },
}
pub enum ReduceOp { Add, Max, SoftmaxNormalize }
pub enum ReduceTree { Linear, Split { factor: u64 } }   // Split = split-K (the 5a/L3/L4 rewrite)
pub enum Scope { Warp, Workgroup }
```

Capability profile (extends `GpuHardware` compute params + construct
flags) — **one profile per target+generation**:

```rust
pub struct TargetProfile {
    pub compute: GpuHardware,          // peak, dram, l2, smem, regs, sm_count
    pub warp_width: u32,               // 32 NVIDIA / SubgroupSize
    pub mma: Vec<FragLayout>,          // available fragment shapes
    pub async_copy: AsyncKind,         // None | CpAsync | Tma
    pub ldmatrix: bool,
    pub coopmat: bool,
    pub subgroup_size: Option<u32>,
    pub max_grid: [u64; 3],
}
```

## 6. Phase 1 — plan construction (consolidation)

`KernelPlan` is built from existing analysis; **no codegen change** →
byte-identical ship.
- `KernelShape` + detectors → `Reduce`/`Tile`/`Proofs`.
- `GemmPlan` → `Tile` + `Mma` + `Stage`/`AsyncCopy`.
- `gpu_strategy::select` → `PlanShape`, now via `TargetProfile` for
  **all** targets (fixes the Vulkan-follows-plan gap).
- `GpuSchedule` → plan ordering/fusion (unchanged semantics).
- `capabilities.rs` gates constructs at lowering.

Gate: golden inspect/dump test for every `.abv` node; old vs new dump
reviewed; suite unchanged.

## 7. Phase 2 — per-target `Lowering`

```rust
pub trait GpuLowering {
    fn target(&self) -> TargetKind;
    fn lower(&self, plan: &KernelPlan, p: &TargetProfile) -> Result<LoweredNode, String>;
}
pub struct LoweredNode { pub kernels: Vec<KernelBlob>, pub dispatch: DispatchProgram }
pub struct KernelBlob { pub name: String, pub domain: KernelDomain,
                        pub bytes: Vec<u8>, pub geometry: DispatchGeometry }
```
- **PTX lowering**: adapters over `ptx/general.rs` + `ptx/tensor.rs`
  first (strangler), then refactor those to consume only `plan.ops`.
- **SPIR-V lowering**: adapters over `spirv/kernel.rs` + `spirv/gemm.rs`.
- **Runner/desc (item (B))**: `RunnerKernel` gains `owner` + `domain`; the
  runner emits a **lane-conditional launch sequence per node**; the
  compile.rs by-name merge becomes a projection. This **absorbs** the
  tensor dual-image special case and **deletes** S1's `split` field +
  `split_dispatch`.
- **The split** = the `ReduceTree::Split` rewrite (licensed by the
  existing associativity proof), lowered by each target — so the decode
  attention (`5a`), GEMM 2048³ (`L3`), and small-K (`L4`) fall out of one
  mechanism, on every lane. **No throwaway lane-conditional patch.**

## 8. Phase 3 — retire existence proofs (Rule 24)

At parity: retire the hand emitters / loan matchers (fused-attention
family; the hand deferred-region emitter's gate is
`2026-09-20-metaprogrammed-composites.md` Stage 2). Recorded in the loan
ledger.

## 9. Migration & parity gates

- Strangler: plan + lowerings beside the current path; config switch per
  node; **default old** until parity.
- Parity = behavior-identical + both-lane correctness + no suite
  regression. Golden: `.abv` corpus old-vs-new byte-diff (annotate
  expected diffs for genuine refactors).
- On-device: `m3_attention_harness.sh`, `softmax_gate.sh`, GEMM all-ones,
  high-REPS microbenches (clock-aware).
- Per commit: `cargo test --lib`, no new warnings, Praetor baseline-diff.

## 10. Docs updated in the same commits

New `docs/architecture/kernel-plan.md`; amend `abv-gpu-doctrine.md` §4,
`backend-contracts.md`, `gpu-backend-strategy.md`, `proof-vs-shape.md`,
`backend-type-dispatch.md`; this plan; amend
`2026-09-30-general-reduction-split.md` (split = plan rewrite; S1 `split`
field retired).

## 11. Risks

- `Mma`/`FragLayout` warp-vs-workgroup mapping — the critical design
  (explicit lattice in Phase 0 mitigates).
- Strangler discipline vs big-bang — mitigated by byte-identical gates.
- Capability/generation drift (sm_80/86/90/100) — profile carries it.
- Vocabulary creep — Rule 23/24 review per op.

## 12. Sequencing

1. **Phase 0** IR + `TargetProfile` + serde/dump + golden inspect test.
   **DONE** (`1edbf037`, refined `828395d1`).
2. **Phase 1** construction, GEMM first, byte-identical.
   - 1a **DONE** (`bb655c3a`): `KernelPlan::from_shape` for the
     `ReductionInfo` nodes (Dot/Softmax) + golden dump.
   - 1b(a) **DONE** (`2b3fbd44`): the deferred region's reduce span
     (`reduce_end` = the accumulator loop's range end) is now an analysis
     fact; `from_shape` builds the deferred `SoftmaxNormalize` plan.
   - 1b(b) **DONE** (`fa78ad0b`): `analysis::gemm_shape` owns the
     structural matcher (relocated from the backend; `fold_consts` moved
     to the frontend). `from_shape` enriches GEMM nodes with
     `Tile/Stage/AsyncCopy/LoadMatrix/Mma/Store`; `gemm_h.abv` output is
     byte-identical to pre-relocation.
   - observability **DONE** (`e71742eb`): `BRIEV_DUMP_PLANS` prints every
     eligible node's plan.
3. **Phase 2** PTX lowering adapters → (B) runner/desc projection.
   - 2.1 **DONE** (`a61cda7c`): `RunnerKernel.owner`/`domain` +
     `emit_kernel_node` per-lane grouping (single-Shared fast path,
     byte-identical).
   - 2.2 **DONE** (`b89b750c`): compile.rs carries PTX companions as
     `CudaOnly` projections; the split-enabled build emits
     `fattn__combine` + the lane branch.
   - 2.3 **DONE** (`68e5acfa`): primary-as-`CudaOnly` for a split node
     was already in `deferred_primary_identity`; the missing half was the
     S4 workspace gate (`enforce_split_workspace`) — without it
     `ptx_deferred_split` corrupted memory. With the gate, the knob is
     **correct and enableable**: in-source `### ptx_deferred_split: 1;`
     on a workspace-sized fixture (`examples/gpu/softmax_composite_knob.abv`)
     emits `sfused__partial` + `sfused__combine` and passes
     `softmax_gate.sh` on BOTH lanes (CUDA 5.91e-06, Vulkan 2.06e-05);
     the multi-path split factor is per-kernel (partial S=8, SPIR-V full
     S=1, combine S=1) and both grids validated on device.
   - 2.4 **DONE** (2026-10-01): the strangler seam — new module
     `backend::gpu_lowering` (`GpuLowering` trait, `GeneralNodeCtx`,
     `PtxGeneralLowering`) and the general family's emission extracted to
     `ptx::emit_general_node` (legacy arm `plan: None`, adapter arm
     `plan: Some`, ONE body). The `ptx_plan_lowering` knob (default 0,
     D30-wired) routes flag-1 builds through the adapter; the parity test
     `plan_lowering_parity_with_legacy` pins both arms node-for-node,
     blob-for-blob (it flushed out the `compile_cubin` shared-workdir race
     — BUGS.md). Stage-1 plan consumption: node identity + the
     `disjoint_workitems` proof, gated by `check_profile` (kind + warp 32)
     and `check_family` (no `Mma`). Flag-off builds are byte-identical;
     suite 2814.
   - 2.5 **DONE** (2026-10-01): the remaining lowering adapters —
     `PtxTensorLowering` (the GEMM branch extracted to
     `ptx::emit_gemm_node`, then decomposed under Praetor's fn ≤100 /
     cog ≤15 / cyclo ≤15 / params ≤6 caps into `gemm_check_matrix_types`,
     `gemm_stage_plan`, `gemm_select_grid`, `gemm_codegen`, `gemm_blob`,
     `gemm_runner_kernel` + the `GemmTierIn`/`GemmKernelOut` parts
     structs) and `SpirvLowering` (the SPIR-V loop extracted to
     `spirv::runner::emit_node_kernel`, named to avoid the pre-existing
     transaction emitter `emit_kernel_node`). The `spirv_plan_lowering`
     knob (mirror of `ptx_plan_lowering`, default 0) routes flag-1
     builds; plan admission is now ONE shared helper
     (`gpu_lowering::admission_gates`) called by all three emitters, and
     flag-1 tests share `config_tuning::SettingsGuard::install_with`.
     Parity: `plan_lowering_parity_with_legacy_gemm` (tensor, naive f32
     64³) and `spirv::tests::plan_lowering_parity_with_legacy` — flag 0
     vs 1, kernel-for-kernel, blob-for-blob; gate tests
     `spirv_plan_arm_gates_wrong_node` + the existing PTX pair. Flag-off
     builds byte-identical (runner.c + .spv); suite 2817; Praetor: no
     new violation rows (metric drift on the pre-existing
     `build_ptx_kernels` rows is improvement: cog 126→34, fn 425→190).
   - 2.6 **DONE** (2026-10-01): the split intent is plan-level —
     `ReduceTree::Split` (item 4 below).
   - **NEXT**: delete S1's `split` field, then L1 primitive audit / D14
     remainder / B4 per the daily-use sweep umbrella.
4. **Split as `ReduceTree::Split`** → device-validate (unblocks 5a).
   **Device validation DONE** (`68e5acfa`: declared `split<8>` fixture +
   knob fixture, both lanes). **PLAN IR DONE** (2026-10-01): the split
   decision now rides in the plan. Analysis (`kernel_plan.rs`) owns the
   full decision chain moved verbatim from `backend::ptx` —
   `fold_count`, `split_eligible`, the `split<N>` modifier parse
   (`declared_split_factor`), plus `deferred_split_tree` which records
   the INTENT at construction: source (`split<N>`) > model
   (`reduction_split_factor_for`, hardware-parametric), gated by the
   deferred facts (`reduce_end`/`normalize_end`, pinned equal to the
   emitter matcher by `deferred_facts_agree_between_plan_and_matcher`)
   and the emitter preconditions — ineligible or unresolvable ⇒
   `ReduceTree::Linear`, never a split the emitter could not lower.
   `KernelPlan::split_tree_factor()` is the accessor. The PTX lowering
   consumes it (`deferred_split_for` gains `SplitFactors { declared,
   plan }`; gates/enablement/warnings still run on both arms from the
   same legacy inputs — a plan factor can restate, never enable),
   config knobs (`ptx_deferred_region`/`ptx_deferred_split`) and the
   workspace gate remain DOWN-SELECTS at lowering (the plan never reads
   them — Rule 2), and SPIR-V lowers `Split` linearly for now (documented
   capability gap on `SpirvLowering::lower`, no silent semantic change).
   Parity: model path (`plan_lowering_parity_model_split_knob`), refusal
   path (`plan_lowering_parity_declared_split_refusal`), workspace
   refusal on both arms, the existing declared-split parity now flows
   through the plan factor. Flag-off byte-identical (runner.c + .spv);
   suite 2822; warnings 19; Praetor no new rows (numeric drift only);
   `softmax_gate.sh` s8 + knob fixtures both lanes PASS (5.91e-06 /
   2.06e-05, matching the recorded device baseline). Remaining: delete
   S1's `RunnerKernel.split` field (contract B runner/desc projection).
5. SPIR-V lowering; Vulkan consumes `gpu_strategy`.
6. Phase 3 retirements.
7. (Future) CPU/LLVM lowerings.

## Decisions (resolved 2026-09-30)

1. **Op set**: full, explicit (Section 5) — no deferral.
2. **Serialization**: serde + a stable textual `dump()` — inspectable and
   golden-testable.
3. **`FragLayout`**: designed explicitly now (with `cooperative`), not
   deferred.
4. **Split / 5a**: folded into the plan as a rewrite — the throwaway
   lane-conditional patch is NOT taken.
5. **Scope**: GPU (PTX + SPIR-V) first; CPU/LLVM as later lowerings.

## Governance — derivation, not recognition (2026-09-30)

This plan is governed by `docs/architecture/derivation-not-recognition.md`.
Binding consequences:

1. `KernelPlan` is a **derivation target** — the record of the shape the
   compiler *invented* for the declared intent. It is **not** a catalogue of
   recognised shapes, and it is internal (never a user surface).
2. Plan construction and every lowering branch only on **structural facts
   and proofs**, never on an algorithm name or a one-algorithm op sequence
   (the **recognition gate**, §7 of the doctrine).
3. Every matcher admitted as an interim (the deferred-region detector, the
   GEMM structural matcher) is admissible **as a fact derivation** and
   carries a Rule 24 **retirement gate**.
4. The **coverage ledger** (declared construct × proof class → exploiting
   pass → test) is the auditable form of "optimise everything declared"; the
   **acceptance test** (delete stdlib algorithms; year-two algorithm hits
   the ceiling, zero compiler changes) is the bar.
5. The strategic prize this plan enables is **fact-exploitation passes**
   (disjointness→vectorisation, associativity→split, lifetime→reuse,
   bounds→unroll); the plan + lowerings are the substrate they write into.

