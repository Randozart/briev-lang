# KernelPlan + per-target lowering

*2026-09-30 → 2026-10-01. The layer between proof-carrying frontend facts
and per-target emitters (plan
[`2026-09-30-kernel-plan-and-per-target-lowering.md`](../plans/2026-09-30-kernel-plan-and-per-target-lowering.md)).*

## Why

The compiler keeps the eternal (Rule 24): proofs, rewrite rules licensed
by proofs, and one ISA-neutral decision record. Algorithm shapes and
vendor syntax stay in the language/stdlib. `KernelPlan` is that record —
**derived** from facts (`KernelShape`, the reduction/deferred detectors,
`analysis::gemm_shape`), never from algorithm names (Rule 23). Each target
then **lowers** the plan by selecting its own instructions
(`abv-gpu-doctrine.md` §4: `mma.sync`, `cp.async`, `ldmatrix`, coopmat
and friends are lowerings, never independent codepaths).

## The IR — `src/analysis/kernel_plan.rs`

- **`KernelPlan`** — one per eligible node: `node`, `work`
  (`WorkItem::PerItem | Strided` — block/stride is a *lowering* choice,
  never a plan fact), `ops: Vec<PlanOp>`, `shape: PlanShape`
  (tile/stages from the cost model), `proofs: PlanProofs`.
- **`PlanOp`** — ISA-neutral primitives: `Tile`, `Stage`, `AsyncCopy`,
  `LoadMatrix`, `Mma`, `Reduce { op, span, tree, frag }`, `Broadcast`,
  `Barrier`, `Store`. Every op must be realizable by every lowering.
- **`ReduceTree`** — `Linear` today; `Split { factor }` is the split
  rewrite (5a decode / L3 split-K / L4 small-K) licensed by
  `proofs.associative_reduce` — one mechanism, every lane (Phase 2
  item 4).
- **`PlanProofs`** — the eternal properties *carried, not re-derived*:
  `disjoint_workitems` (uniqueness → no barrier), `associative_reduce`
  (fold may split), `single_writer`.
- **`TargetProfile` / `TargetKind`** — target + generation capabilities
  (warp width, mma fragments, async-copy kind, ldmatrix/coopmat,
  subgroup size, grid limits). Constructors: `ptx_sm86()`,
  `spirv_vulkan()`. The plan is target-*parameterized*; its structure is
  shared.

**Construction** — `KernelPlan::from_shape(name, shape, items, consts,
hw)`:

- proofs from structural facts (`shape.eligible`, reduction presence,
  `write_buffers.len() <= 1`);
- deferred-normalize → `Reduce { SoftmaxNormalize }`
  (`shape.deferred_normalize`, analysis-proven);
- GEMM → `Tile/Stage/AsyncCopy/LoadMatrix/Mma/Store` enrichment via
  `analysis::gemm_shape` (the same matcher `GemmPlan::match_stmts`
  wraps — one decision shared by every lowering).

**Observability** — `BRIEV_DUMP_PLANS=1` prints every eligible node's
plan on the build path; plans are serde-serializable and
`KernelPlan::dump` is the stable textual form for golden tests.

**Known ambiguity**: a plain softmax reduction and a deferred region both
plan as `Reduce { SoftmaxNormalize }` — the deferred *shape* fact
(`shape.deferred_normalize`) disambiguates, so backends must probe it
until the op sets diverge.

## The lowering seam — `src/backend/gpu_lowering.rs`

```rust
pub trait GpuLowering {
    fn target(&self) -> TargetKind;
    fn lower(&self, plan: &KernelPlan, p: &TargetProfile)
        -> Result<LoweredNode, String>;
}
pub struct GeneralNodeCtx<'a> {   // material the emitters need beside the
    name, shape, program,         // plan — the plan is ISA-neutral and
    layout, universe,             // carries no AST
    int_bits, irr_free,
}
```

Adapters (all landed 2026-10-01, Phase 2.4):

- **`PtxGeneralLowering`** — pre-flight `check_profile` (profile must BE
  the target; the warp-synchronous families need warp 32),
  `check_family(plan, tensor: false)` (no `Mma`), then
  `ptx::emit_general_node(ctx, Some(plan))`.
- **`PtxTensorLowering`** — same pre-flight with `check_family(plan,
  true)` (requires `Mma`), derives `GemmPlan::match_stmts` on the arm,
  then `ptx::emit_gemm_node(&GemmNodeCtx, gemm, Some(plan))` — ONE body,
  two arms, like the general family. `GemmPlan` (field names, m/n/k) is
  structural material the plan does not yet carry — derived by the same
  matcher on both arms.
- **`SpirvLowering`** — one adapter for every eligible node (the SPIR-V
  lane's `emit_kernel` hook covers all families via cooperative/tiled/
  tensor flags, so no family gate): `spirv::runner::emit_node_kernel(
  &SpirvNodeCtx, Some(plan))`.
- Plan admission is ONE shared helper — `gpu_lowering::admission_gates`
  (node identity + `proofs.disjoint_workitems`); both PTX emitters and
  the SPIR-V emitter call it (a `None` plan skips gates: the legacy arm).
- **`LoweredNode { kernels, warnings }`** — warnings are the D28
  size-modifier notices; `KernelBlob` is currently `type KernelBlob =
  RunnerKernel` and narrows to the §7 `{name, domain, bytes, geometry}`
  shape when contract (B) (runner/desc projection) lands.

## Strangler state (Phase 2.4, 2026-10-01)

`ptx::emit_general_node(ctx, plan: Option<&KernelPlan>)` — ONE body,
two arms:

- `plan: None` — the legacy inline path (default, byte-identical);
- `plan: Some(..)` — plan admission gates: identity (`plan.node` must be
  this node) + `proofs.disjoint_workitems`, then the same emission.

Routing: `build_ptx_kernels` reads the **`ptx_plan_lowering`** knob
(`config_tuning`, default **0**, D30-writable via `### ptx_plan_lowering:
1;`) ONCE and gates both PTX families (general → `emit_general_node`,
GEMM branch → `emit_gemm_node`); `build_kernels` (SPIR-V) reads the
mirror **`spirv_plan_lowering`** knob the same way. Flag 1 constructs
`KernelPlan::from_shape` per eligible node and lowers through the trait;
flag 0 never touches the plan.

**Parity is the license to flip the default** (three tests, one
contract): `ptx::tests::plan_lowering_parity_with_legacy` (general,
`split<8>` deferred fixture), `plan_lowering_parity_with_legacy_gemm`
(tensor, naive f32 64³ GEMM), and `spirv::tests::
plan_lowering_parity_with_legacy` — flag 0 vs flag 1 pinned kernel-for-
kernel, blob-for-blob. Plan-gate coverage: `plan_arm_gates
_wrong_node_and_missing_proof` (PTX) and `spirv_plan_arm_gates
_wrong_node` (SPIR-V); trait-level coverage: `gpu_lowering` tests
(profile/kind/warp gates, family refusal). Test installs share
`config_tuning::SettingsGuard::install_with` (Drop restores the full
settings generation).

The same parity test flushed out the `compile_cubin` shared-workdir race
(BUGS.md, fixed: `cubin_workdir(pid, call_seq)` — unique dir per call).

### Stage-1 plan consumption (honest gaps)

The adapter gates on identity + proofs but does not yet *derive emission
decisions* from `plan.ops`. Documented until the corresponding items
land:

1. deferred dispatch still probes `shape.deferred_normalize` (op-set
   ambiguity, see above);
2. warp-slicing has no plan fact yet;
3. the split factor lives in `deferred_split_for` (source > config >
   model) until item 4 populates `ReduceTree::Split` at construction;
4. the tensor family now lowers through `PtxTensorLowering` — but the
   adapter still derives `GemmPlan` itself; when the plan carries gemm
   fields (§7), the derivation moves to plan construction (still one
   matcher, both arms).

## Sequencing pointer

See plan §12: 2.4 DONE (this seam) → SPIR-V adapter + tensor adapter
DONE (2026-10-01) → `ReduceTree::Split` (item 4) → delete S1's `split`
field → Phase 3 retirements (Rule 24 existence proofs retire when
general machinery reaches their numbers).
