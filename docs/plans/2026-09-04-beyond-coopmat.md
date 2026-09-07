# Beyond-vendor-lowering GPU campaign

**2026-09-04.** Realizes the doctrine in
`docs/architecture/abv-gpu-doctrine.md`: one `.abv` program, peak on
every probed device, Briev-owned codegen only (no nvcc, no cuBLAS in
the compiler path). Locked by the user ("the answer is always a").

## Baseline (recorded 2026-09-04, ledger row 2026-09-04b)

| cell | number |
|------|--------|
| 4096³ smem-R4 (default tier) | 8.3ms best window = 16.5 TFLOP/s |
| 4096³ direct loads | 8.6ms best window ≈ parity |
| 2048³ smem-R4 | 0.905ms = 19.0 TFLOP/s |
| 4096²×512 smem-R4 | 1.504ms = 11.4 TFLOP/s |
| 8192³ smem-R4 | ~84.8ms = 13.0 TFLOP/s (+23% vs direct) |
| R sweep | R2 0.637, **R4 = 1.00**, R8 0.628 |
| **Race target** | ggml-cuda 3.205ms = **42.0 TFLOP/s** (83% of 50.6 peak) |
| Key hypothesis | f16acc-vs-f32acc delta was only ~11% → the KHR lowering likely does NOT double-pump → portable ceiling ≈ 25.3 TFLOP/s |

Measurement discipline: in-process A/B only (alternating-order batched
submissions; self-A/B ratio 0.985–1.001). Unlocked clocks — ratios
within one run are the signal, absolute best-window numbers second.

## Stage 0 — Ceiling truth — **COMPLETE (2026-09-04, VERDICT: PORTABLE PATH WINS)**

Delivered (`emit_mma_ceiling_kernels` + `mma_ceiling_bench.c`,
commit 38f07e2c): fold-proof microkernel — 4×4 distinct (A_j,B_j)
fragments per iteration, runtime bound, NoContraction-decorated,
loaded fragments, varying row bases. The fold-proofing took three
driver optimizations to defeat (constant-operand mmas materialize
zeros; loop-invariant A·B hoisted/de-fused; reassociation over shared
operands — BUGS.md 2026-09-04).

**Measured** (RTX 3060, bound=10000, 4096 wgs, batched ×4): both
f16acc and f32acc = **50.1 ms/launch** — the window is **L2-load-bound**
(16 coopmat loads/iteration ≈ 6.7 TB/s) and the mma work
(5.4 TFLOP/launch) fits entirely inside it ⇒ **coopmat mma rate
≥ 107 TFLOP/s ≈ 100% of the F16-acc hardware peak (102 TF)**.

**Gate outcome: ceiling ≥ 40 TFLOP/s → the portable road wins; Stage 2
demoted to optional.** Two ledger corrections: the "50.6 TF FP16-acc
peak" label = the FP32-acc rate (true F16-acc peak = 102 TF); the
production GEMM's 16.5 TFLOP/s = pipeline-bound (loads+fills+barriers
eat ~3×), NOT vendor-capped. The ceiling number is a standing
per-driver-era measurement (doctrine §6); re-arm the specialized tiers
only if it regresses.

## Stage 1 — Portable extraction (~1 session, ships regardless)

Universal SPIR-V levers, each landing behind the in-process A/B, each
with a VERDICT row, losers reverted:

- **D1 — 2 panels per stage.** *(MEASURED 2026-09-04d: REJECTED — 2×
  slower, occupancy-bound; knob defaults to 1, infrastructure kept.)*
- **D2 — register-prefetch software pipeline.** *(MEASURED 2026-09-04g:
  REJECTED — the fused refill wins by 2–6% on clean rounds; the 32-reg
  carry across the mma + issue-slot contention beat the latency
  hiding. Knob defaults to 0; the fill split machinery kept.)* Issue
  panel k+2 global loads into registers during mma(k); store to smem
  after the mma batch; one barrier pair per iteration retained. The
  portable emulation of `cp.async` (SPIR-V has no async global→workgroup copy).
- **D3 — f16x2 fill loads.** *(MEASURED 2026-09-04d: LANDED — +24%,
  16.7→20.7 TFLOP/s; default on.)* The scalar fill loads one half per
  `OpLoad`; uint/ushort2 loads halve fill instruction count.
  - *D3b f16×4 quads (2026-09-04f): REJECTED — both forms (2×-unroll
    1.183×, true v4f16 view 1.133×) lose to the pairs mode; after D3
    the fill is transaction-bound, not instruction-bound.*
- **D4 — occupancy shaping.** LocalSize search (O5 rung) under the
  hardened harness — the R sweep showed the accumulator-register trade;
  LocalSize × workgroup count is the remaining axis. *(D5 K-panel
  stagger pre-measured 2026-09-05: REJECTED 1.148× — the phase-lock is
  L2-broadcast efficiency; D4 must INCREASE panel sharing
  (per-subgroup smem B slices), never desynchronize the panel clock.)*
  *(D4 measured 2026-09-05 at ~10ms era: S=2 REJECTED 7% slower —
  3 WGs/SM hurt L2 sharing. RETESTED 2026-09-07 at the 5.3ms era:
  VERDICT REVERSED — S=2 LANDED as default (+7-8%, 8 warps/SM hide
  the fill DRAM latency; S=4 rejected at 2 WGs/SM). The knob is
  era-conditional: occupancy pays once the pipeline is fill-latency
  bound.)*

Target: 16.5 → 20–22 TFLOP/s at 4096³ if the pipeline is the binding
constraint (Stage 0's cost-share probe says which).

**2026-09-07 session: 5.3 → 4.55 ms (23.3 → 30.2 TFLOP/s) at 4096³.** Three
rungs landed behind the in-process A/B: **D4-retest S=2** (default → 2,
+7-8% — the D4 rejection was era-conditional: at the 5.3ms baseline the
extra warps hide the fill DRAM latency), **1-barrier anti-WAR refill**
(the loop refill writes the OTHER stage, dropping the workgroup barrier
between the mma and the fused fill — +5-9%), and **D2 register-prefetch
at S=2** (+8-11% — the S=1 rejection was register-bound, and the S>1
load/store phases had a per-subgroup-B addressing bug, now fixed).
Null/rejected at the current era: single-buffer (occupancy not binding),
R=8/R=2 (R=4 optimal confirmed), quad fill, pps=2, S=4. The portable
path now runs against its structural limits: in-order fills,
workgroup barriers, m16n16 coopmat loads at the smem bandwidth minimum
(4-way = 512B/128B-per-cycle, NOT a fixable conflict).

## Stage 1.5 — Profiling & Fill Optimization (**ACTIVE**)

**2026-09-05.** Vulkan timestamp queries landed (commit a9313540).
Baseline measured: **5.3 ms GPU kernel time** (23.3 TFLOP/s) at 4096³.

**Root cause identified:** 891 integer div/mod per workgroup for fill
index decomposition. The 16 MMA ops (actual compute) are 0.3% of total
instructions; the fill address arithmetic is 32%.

| Instruction | Count | % |
|-------------|-------|---|
| IDiv/IMod | 891 | 19.5% |
| Bitwise (shift/and) | 193 | 4.2% |
| Select (A/B branch) | 386 | 8.4% |
| AccessChain | 409 | 8.9% |
| **MMA** | **16** | **0.3%** |

**O1 — Strength-reduce power-of-two div/mod:** Replace all `OpUDiv` by
power-of-two constants with `OpShiftRightLogical` and `OpUMod` with
`OpBitwiseAnd`. Eliminates ~800 instructions. See
`docs/plans/2026-09-05-kernel-profiling-analysis.md` for full plan.

**O2 — Linear fill addressing:** Restructure fill loop to compute base
address once per thread, then increment by constant stride. Eliminates
per-element tile/row/col decomposition. Next rung after O1.

Conservative estimate: O1+O2 → 3.4–4.0 ms → 29–35 TFLOP/s.

## Stage 2 — Briev PTX tier (**OPTIONAL escape hatch** — demoted by Stage 0)

Stage 0 measured the portable path at the hardware tensor peak; this
tier only re-arms if a future driver era regresses the coopmat ceiling
(re-run the microkernel per driver era, doctrine §6) or a workload
needs `ldmatrix`/`cp.async`-class scheduling the lowering cannot
express. The engineering notes below remain valid if it re-arms.

- **S1 — driver module** `lib/runtime/briev_dev_cuda.c`: cuInit /
  cuModuleLoadData / cuLaunchKernel in the existing driver-plugin
  shape (`briev_accel_rt.c` registration). Probe-driven selection
  alongside the Vulkan/OpenCL drivers; init-time diagnostic if the
  driver JIT (ptxas) is absent.
- **S2 — emitter** `src/backend/ptx/`: text PTX emission consuming
  `GemmPlan` + tier eligibility + dispatch predicates (analysis-once,
  backend-contracts §2). Surface declared in
  `src/backend/capabilities.rs`. `--backend` name: `ptx`.
- **S3 — tensor GEMM family first**: `mma.sync.aligned.m16n8k16`
  (f32-acc reference tier + f16-acc race tier), `ldmatrix` fragment
  loads, `cp.async` global→smem, warp-tile geometry from `GemmPlan`
  (64×64 default), fragment layout per lane. ggml's kernels are the
  open layout reference; per-op microtests (single mma, single
  ldmatrix) precede integration.
- **S4 — correctness gate**: identical contract vs the naive reference
  tier across the whole shape portfolio (2048³ / 4096³ / 8192³ /
  skinny-K / small shapes), gates as per tier (5e-3 f32-acc, 1e-2
  f16acc).
- **S5 — performance gate**: match the 42.0 TFLOP/s anchor at 4096³,
  then beat it (shape-specialized configs are the structural edge:
  our kernels specialize per program; ggml's dispatch tables cannot).
- **S6 — auto-tune loop**: `derive --stochastic` sweeps
  (tile × stages × warps) per device profile, winners cached in the
  target config (`config/targets.*`). Derived tile tables replace
  hand-written ones — the durable "smarter" (doctrine §4).

Docs to touch when S2 lands (same commit):
`docs/architecture/backend-contracts.md` (new charter row),
`docs/HANDOFF-2026-08-31-gpu.md` (status block),
`spec/SPEC.md` §9.8 (Tier 4 paragraph — written as planned already),
`AGENTS.md` reference index.

## Risks

- **PTX fragment layouts** (m16n8k16 lane mappings, ldmatrix
  addressing) are the classic source of silent wrong answers —
  mitigated: per-op microtests with known-value fragments before any
  integration.
- **Driver-JIT variance** (ptxas version changes codegen): pin the
  ceiling measurement per driver era; SASS inspection optional via
  `cuobjdump` when claiming scheduling properties (the LTO lesson,
  rule 20).
- **Scope creep in S3**: GEMM family ONLY until S5 passes. No other
  workload enters the PTX tier before the anchor race is won.
- **Stage 0 gate discipline**: if the ceiling lands between 25 and 40
  TFLOP/s, re-run the decision with the fill-cost share measured —
  the gate is a number, not a mood.

## Undo

Stage 1 experiments: config + emitter-local, reverted via git; losers
get VERDICT rows. Stage 2 adds new modules/driver files only — the
SPIR-V path remains byte-identical when `--backend spirv` is used and
when the probe selects it. Doctrine doc lives; only the tier table
rows change status.
