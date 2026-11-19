# P4: `g0_fsum_approx` OpenCL — portable workgroup reduce for the `approx` fold lane

**2026-11-19, scoped; implementation design added 2026-11-19 after the backend
research pass (this is the scoping doc, per "write the plan before
plan-driven work").**

## Step 0 — sync precondition (researched 2026-11-19)

`origin/main` advanced 5 commits past `094ead0a8` while P1–P4 were in flight:
head `a659c0195` = #133 typechecker fix (every call argument inferred — closes
the silent miscompile), `ir_verify` call-arity + representation-class check,
webstack test/gate repairs, two docs commits (compiler-health program +
open-PR triage). **Zero SPIR-V/accel files touched** (confirmed by
`git diff --name-only 094ead0a8..origin/main`) and no census/memcheck files, so
the P3 ratchet state is unaffected by the merge. Merge via
`scripts/sync-main.sh` **before** starting P4, then re-land both heads.

## The defect

`g0_fsum_approx` prints `WRONG` on the NEO/OpenCL lane while `g0_fsum_exact` and
both levelzero lanes are correct. The `approx` keyword licenses the reassociated
warp-slice fold lane, which the SPIR-V backend lowers to a hardware
`OpGroupNonUniformFAdd` (the `SubgroupFAdd#` intrinsic) at **Subgroup** scope.
The row kernel launches `LocalSize 32`. Subgroup scope is only equivalent to
"all 32 invocations" when the device's subgroup width == 32 — true on
levelzero (measured 32) but **not** on NEO's OpenCL (subgroup width 16 on Xe),
where each of the two 16-wide subgroups reduces its own half and both write the
row → `G0_SUM_BAD` high / `GPU_SUM` wrong.

## Options considered

| option | status | why |
|--------|--------|-----|
| hardware `Subgroup` scope (current) | broken on NEO | device subgroup width 16 ≠ LocalSize 32 |
| hardware `Workgroup` scope | **REFUTED 2026-11-18** | `G0_SUM_BAD=220`, `GPU_SUM=0 CPU_SUM=0` — the hardware workgroup-scope reduce is unsupported/no-op on this driver (BUGS.md 2026-11-18) |
| device-width `LocalSize` (query subgroup width at dispatch) | not chosen | needs a dispatch-side device query + a per-width local size; the reduce is still hardware-subgroup, so the kernel shape has to match the device |
| **portable workgroup shared-memory reduce** | **chosen (P4)** | device-independent; correct at any subgroup width |

## The fix (P4): shared-memory + ControlBarrier + lane-0 store

Replace the hardware `SubgroupFAdd#` reduce, **in the 32-lane row fold kernel
only**, with a portable workgroup reduction:

1. Declare a workgroup `OpVariable` (shared) float scratch array, sized to the
   kernel's local size (32 for the row fold).
2. Each lane: `shared[LocalInvocationId] = partial;` then `OpControlBarrier
   (Workgroup, Workgroup, Workgroup)`.
3. Tree fold: for `offset` in `[16, 8, 4, 2, 1]`:
   `if (lane < offset) shared[lane] += shared[lane + offset];`
   `OpControlBarrier (Workgroup)` between each level.
4. Lane 0 (or every lane, uniformly after the final barrier) reads
   `shared[0]` — the full 32-lane sum — and that value is what the store uses
   (`lane-0 store`).

Result: a workgroup-wide sum that is correct regardless of the device's
subgroup width. No hardware `OpGroupNonUniform*`.

## Implementation design (2026-11-19, research complete)

All required machinery **already exists** in the SPIR-V backend — P4 is
plumbing, not new codegen surface:

| need | where it already lives |
|------|------------------------|
| module-global workgroup array | GEMM pattern, `kernel.rs:480-507` + `gemm.rs:509`: `type_array` → `ptr_class(StorageClass::Workgroup)` → `emit_global(Op::Variable)` |
| `OpControlBarrier(Workgroup)` | already emitted at `gemm.rs:626, 704, 3130` |
| lane index | the synthesized body already computes `lane = GetGlobalId#(0) & 31`; the lowerer also carries `local_id_var` (BuiltIn `LocalInvocationId`, `lower.rs:66`, threaded at `kernel.rs:914`; in the cooperative interface at `kernel.rs:1051`) |
| structured value-selection in expression context | `emit_bool_selection` (`lower.rs:1479`): `OpSelectionMerge` + `branch_conditional` + phi merge — proven for nested arms |
| kernel local size | `LocalSize = 32` for cooperative kernels (`kernel.rs:1058-1061`) → the butterfly is a **fixed 5-step** unroll (offsets 16,8,4,2,1), no runtime loop |

**Containment: no new intrinsics, no statement-vocabulary changes, no PTX
impact.** The PTX backend synthesizes its own cooperative bodies
(`ptx/general.rs:64-295`) and lowers `SubgroupFAdd#` itself
(`ptx/general.rs:2563`); the P4 statements never reach it. GEMM/coopmat
emits its own `OpGroupNonUniform` directly (`gemm.rs:3841`, 256-lane
workgroups) and its own shared arrays — untouched by design. Shuffles/ballot/
broadcast are other consumer lanes — untouched.

### The mechanism (one `// 2026-11-19 (P4)` block)

1. **Scratch declaration** — at the cooperative+reduction site in
   `emit_kernel` (where `synthesize_reduce_stmts` /
   `synthesize_softmax_stmts` run, ~`kernel.rs:600-650`, where `builder` is
   in scope): emit the module-global workgroup array `briev_wg_red[32]` of
   the fold's element type (`f32` when `fold_is_float`, else int — the same
   flag already selects the intrinsic family), mirroring the GEMM `shared_a`
   pattern.
2. **Plumbing** — add `wg_scratch: Option<Word>` to the lowering struct
   (next to `local_id_var`, `lower.rs:66`) and set it at the same site; the
   GEMM path leaves it `None`.
3. **The butterfly** — in `emit_subgroup_reduce` (float) and
   `emit_subgroup_int_reduce` (int), when `wg_scratch` is `Some`, replace the
   `OpGroupNonUniform*` emission with a shared helper
   `emit_wg_butterfly(op)`:
   ```
   lane = GetGlobalId#(0) & 31                  (re-emit the proven form)
   scratch[lane] = acc                           (OpAccessChain + store)
   OpControlBarrier(Wg, Wg)
   for o in [16, 8, 4, 2, 1]:                   (compile-time unroll)
       scratch[lane] = sel(lane < o,
                            scratch[lane] OP scratch[lane + o],
                            scratch[lane])        // sel = emit_bool_selection shape
       OpControlBarrier(Wg, Wg)
   r = scratch[0]                                 // every lane reads the uniform value
   ```
   - **Dead-slot analysis makes the sel default type-agnostic**: after step
     `o`, slots `[o..31]` are never read again, so the no-op arm stores
     `scratch[lane]` back to itself — no per-op sentinels (0/-inf/inf)
     needed for add/mul/min/max. All four ops share one helper body.
   - Barriers are on the **non-divergent** path (all 32 lanes hit each one);
     only the store is guarded — no divergent-barrier hazard.
   - Per-kernel call count: fold lane = 1 butterfly; softmax = 2 (FMax phase
     then FAdd phase). Each butterfly re-stores every slot before its tree, so
     the two calls cannot interfere.
   - Store-back `scratch[lane]` on inactive lanes is a same-value no-op write
     to a dead slot — cheaper and simpler than a divergent `if`.
4. **Comment repair (Rule 16)**: `lower.rs:~1042-1050` carries a stale
   2026-11-18 comment block that says "WORKGROUP scope, not SUBGROUP … the
   code means" immediately above `Scope::Subgroup` — the refuted/reverted
   workgroup experiment left the comment contradicting the code. Rewrite it to
   record: the 2026-11-18 hardware-Workgroup refutation (BAD=220), its
   revert, and the P4 portable-butterfly decision.

### Associativity licence table (Rule 25 — no reassociation without a licence)

| intrinsic | butterfly? | licence |
|-----------|------------|---------|
| `SubgroupFAdd#` (float) | yes | the `approx` node declaration (gate 3, `kernel.rs:92-93`) — this IS the P4 lane |
| `SubgroupFMax#` / `SubgroupFMin#` | yes | exactly associative + commutative (bit-exact) — safe on every lane (`g0_minmax`) |
| `SubgroupIAdd#` / `SubgroupIMul#` / `SMin` / `SMax` | yes | wrapping int arithmetic is exactly associative (gate 3, `kernel.rs:615`) — bit-exact (`g0_sum`) |
| `SubgroupFMul#` (float product) | **no — keeps `OpGroupNonUniform`** | floating-point `×` is not associative; no current PTL lane exercises a float-product fold (PTL fixtures: sum/minmax/fsum), and the `approx` licence does not cover it. Revisit only when a licensed lane demands it |

The `exact` lane emits **no** subgroup op at all (BUGS.md: "`exact` (no
subgroup op)") — structurally untouched; `g0_fsum_exact` remains a
regression gate, not an affected lane.

### The danger, restated precisely

The one shared surface is the **emitter** (`emit_subgroup_reduce` /
`emit_subgroup_int_reduce`). GEMM isolation is by *emission site*, not by
emitter: GEMM never calls these functions (it emits `OpGroupNonUniform`
directly at `gemm.rs:3841`), and the butterfly only activates when
`wg_scratch` is `Some`, which only the cooperative-reduction site sets. A
blanket unconditional change is what would have broken GEMM's 256-lane
shape — the `Option` gate is the isolation.

## Verification (the gates)

On PTL, OpenCL lane (`BRIEV_ACCEL_DEVICE=opencl`), via `brievc run`
(in-process; the C driver is baked into the binary — archive-only swaps are
inert, P1 lesson). PTL's default shell is **fish** — drive every PTL command
through a script file. Full PTL gate inventory (`benchmarks/intel_ptl/g0.sh`):

- **THE fix**: `g0_fsum_approx` OpenCL → `G0_SUM_BAD=0` and
  `GPU_SUM==CPU_SUM` (was WRONG); licensed lane `gpu_time ≤ 0.106 ms` (P4
  budget; interleaved A/B — single-run splits are device-idle artifacts).
- **Bit-exact regressions** (all three drivers vulkan/levelzero/opencl):
  `g0_fsum_exact`, `g0_sum` (int IAdd), `g0_minmax` (FMax/FMin).
- **GEMM not broken**: `g0_gemm` + `g2_gemm_tiled` (`CANARY_BAD=0`,
  `GPU_SUM==CPU_SUM`) on vulkan.
- **No driver-lane regression**: `g0_elem` on the same loops.
- Local: unit test asserting the cooperative fold kernel's emitted SPIR-V
  contains the WG `OpVariable` + ≥6 `OpControlBarrier` and contains **no**
  `OpGroupNonUniform*` on the fold lane; spirv-val clean; `bash
  scripts/gate.sh quick` green.
- The `exact`-lane and GEMM-lane IR must be byte-stable vs pre-P4 (the
  `// 2026-11-19 (P4)` block is the only emitter diff).

## Rollback

The hardware reduce is one `builder.emit(Op::GroupNonUniformFAdd, ...)` in
`emit_subgroup_reduce`; a new `// 2026-11-19 (P4)` block marks the portable
shared-memory path. Reverting that block restores the hardware form. The GEMM
path is untouched by design.
