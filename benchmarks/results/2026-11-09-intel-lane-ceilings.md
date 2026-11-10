# Intel lane ceilings + compiler efficiency — results (2026-11-09)

Plan: `docs/plans/2026-11-09-intel-lane-ceilings-and-probe-cost.md`.

## W1 — probe-gate emission (DONE)

The output-equality gate compared one basic block per element. Replaced with a
runtime loop per write buffer (same full element-for-element comparison).

| metric | before | after |
|---|---|---|
| `g0_elem.bv` IR | 641 MB / 13.6M lines | 204 KB / 5810 lines |
| `g0_elem.bv` build+link (local) | >45 min (2700 s timeout) | 78 s |
| `g0_gemm.bv` build+link (local) | — (same class) | 27 s |
| gate semantics | every element, tol/exact | unchanged |
| g0_elem output | BAD=0 GPU_SUM=261566496 CPU_SUM=261566496 | identical |

`g0_elem` requires `ulimit -s 65536` (pre-existing, BUGS.md 2026-11-06).

## Gate matrix on PTL (2026-11-09)

Vulkan (required): elem, gemm 512³, sum, minmax, fsum_approx, fsum_exact — all
PASS (`BAD=0`, GPU == CPU, compiled on device).

Level Zero (required, promoted): elem, sum, minmax, fsum_approx, fsum_exact —
all PASS with `compiled on 'levelzero'` and host-identical results. GEMM: CPU
fallback, self-check ok (documented: Vulkan-only until G3 — L0/NEO cannot
consume the Vulkan image, IGC requires OpenCL.std).

OpenCL/NEO (reported): elem, sum, minmax, fsum_approx, fsum_exact — correct
when they run; the exact-fold kernel intermittently hangs in `clFinish`
(BUGS.md 2026-11-09, ~1/3 under load). Never silent: a hang is printed, a
wrong result fails.

Stage F fast smoke (`g1_lane_smoke.abv`): both lanes correct, repeated.

## Fixes along the way

- `--accel=force` accepted (both spellings) and supplied by `g0.sh`; the
  retired per-fixture `!> accel: force;` policy now lives in the gate flags
  (migration audit hole from the attr commit).
- `g0.sh`: `grep -c`/`grep -o` no longer abort the script under `set -e`;
  a wrong lane result fails, a lane hang is reported.
- L0/NEO receive ONLY the OpenCL projection; kernels without one mark the
  chain dead (clean CPU fallback) instead of feeding IGC the Vulkan module.
- OpenCL flavor uses `OpenCL.std` extended instructions (fmin/fmax/s_min/
  s_max/fma/exp/sqrt/fabs); GLSL.std.450 is now impossible on that flavor
  (IGC rejected the min/max fold lane: "Expects OpenCL.std").
- `BRIEV_ACCEL_EXPLAIN=1` prints per-body eligibility/decision and the reason
  a node has no OpenCL projection.

## W2 — ceilings (in progress)

Four-number matrix pending: Vulkan ceiling (hand GLSL), OpenCL ceiling
(hand OpenCL C), Briev-Vulkan, Briev-OpenCL; roofline bounds first. No lane
verdict is drawn from generated-kernel numbers (emitter quality ≠ lane
ceiling).

## W2a — device facts + roofline bounds (2026-11-09)

PTL iGPU: `Intel(R) Arc(TM) B390`, 96 compute units @ 2.5 GHz, 58 GiB shared
(LPDDR5X, exact bandwidth not reported by clinfo/vulkaninfo), Vulkan 1.4.354
(Mesa 26.2.1), NEO 26.31.39395.

Roofline bounds for the fixed kernel set (assuming ~120 GB/s effective and
~3.8 TFLOPS FP32 SIMD — both to be measured with a bandwidth/FLOPS probe;
XMX ceilings are separate and need a cooperative-matrix reference):

| kernel | work | bound |
|---|---|---|
| elementwise self-add, 1Mi f32 | 8 MB traffic | ~67 µs |
| reduce, 4096 rows × K=1024 f32 | 16 MB read | ~133 µs |
| GEMM 512³ | 268 MFLOP, 3×1 MB | ~71 µs (SIMD compute) |
| GEMM 4096³ | 137 GFLOP | ~36 ms (SIMD compute; XMX changes the ceiling) |

**Important:** current Briev `.bv` gate numbers are NOT usable as lane
numbers — the fixtures fold the counted loops (`txn folded to O(1)`) and the
`# gpu_time` lines are dispatch-timestamp-only (Vulkan driver), absent on
L0/NEO. A fair four-number matrix requires W2b/W2c: real dispatches at the
fixed shapes, wall-clock over repetitions, identical harness for both lanes,
plus hand-written OpenCL C and GLSL references.

Next: W2b (OpenCL C ceiling harness under `benchmarks/intel_ptl/ceiling/`),
then W2c (GLSL) if W2b shows the OpenCL ceiling is competitive.

## W2b — OpenCL C ceiling references (device time, 2026-11-09)

Hand-written OpenCL C, NEO 26.31 on Arc B390; device time from profiling
events (median of 20 after 3 warmups; GEMM 4096³ median of 3). Correctness
vs a double-precision CPU reference (sampled rows for GEMM) PASS on every row.

| shape | f32 | f16 (f32 accumulate) |
|---|---|---|
| copy 1Mi (2 arrays) | 0.0654 ms / 128.2 GB/s | 0.0285 ms / 147.0 GB/s |
| elem 1Mi (3 arrays) | 0.0800 ms / 157.3 GB/s | 0.0472 ms / 133.3 GB/s |
| reduce 4096×1024 | 0.1729 ms / 97.0 GB/s | 0.0966 ms / 86.9 GB/s |
| GEMM 512³ | 0.4213 ms / 637.2 GFLOP/s | 0.2325 ms / 1154.6 GFLOP/s |
| GEMM 4096³ | 94.19 ms / 1459.2 GFLOP/s | 84.40 ms / 1628.4 GFLOP/s |

Reading: measured memory bandwidth ≈ 130–160 GB/s; the SIMD f32 GEMM ceiling
is ~1.46 TFLOP/s and f16-storage/f32-accumulate ~1.63 TFLOP/s. These are the
lane's achievable numbers for these kernel forms, not vendor tensor-core
peaks (XMX needs a cooperative-matrix reference — G3).

Harness: `benchmarks/intel_ptl/ceiling/{cl_api.h,ceiling_harness.c,run.sh}`.

## W2d — Briev-side numbers: blocked by reactor folding (gap)

Const-bound `.bv`/`.abv` programs are precomputed (`txn folded to O(1)`), and
the reactor then dispatches with a folded/zero work count: `# gpu_time` is
either absent (L0/OpenCL) or implausibly small (Vulkan 0.001 ms for 1Mi
elements). Runtime env bounds do not help: `get_env_int!` folds at compile
time when the variable is present during compilation.

Consequence: Briev lane numbers must come from the **blob harness** (launch
the Briev OpenCL/Vulkan image directly with a known state layout), exactly as
`benchmarks/gpu/gemm_bench.c` does for Vulkan. That is the next W2 step; the
device-time plumbing added to the L0/OpenCL drivers is the prerequisite and is
in place (event-based, matching Vulkan's `# gpu_time`).

## Time-plumbing findings

- OpenCL events require the opaque-pointer `cl_event` type; the driver carried
  a struct typedef and queried before `clFinish`. Fixed (profiling queue,
  finish-then-query).
- NEO reports `timerResolution = 52` as **nanoseconds per tick** (not
  cycles/sec): `ms = ticks * resolution / 1e6`. Also fixed the L0 kernel
  timestamp field order (`global.start, global.end, context.start, context.end`).

## W2d — first Briev-side measurements (2026-11-09, PROVISIONAL)

Harness: `BRIEV_ACCEL_BENCH_ITERS=N` repeats the driver launch N times inside
one reactor dispatch and prints `# bench_time` (average wall per launch).
Drivers print `# gpu_time` (event/timestamp device time) and the dispatch
geometry. Ceiling rows above are the reference.

| shape | ceiling (CL C, device) | Briev-L0 gpu/bench | Briev-NEO gpu/bench | Briev-Vulkan bench |
|---|---|---|---|---|
| elem 1Mi f32 | 0.080 ms | 0.201 / 1.32 ms | 0.025 / 2.57 ms | 0.33 ms |
| elem 1Mi f16 (native) | 0.047 ms | 0.106 / 0.97 ms | 0.019 / 1.97 ms | 0.30 ms |
| reduce 4096×1024 f32 | 0.173 ms | 0.277 / 1.81 ms | 0.039 / 3.28 ms | 0.69 ms |
| reduce 4096×1024 f16 | 0.097 ms | 0.126 / 1.24 ms | 0.029 / 2.30 ms | 0.49 ms |
| GEMM 512³ f32 | 0.421 ms | CPU fallback (Vulkan-only) | CPU fallback | 0.31 ms |
| GEMM 512³ f16 | 0.233 ms | 0.330 / 1.37 ms | 0.417 / 0.72 ms | 1.16 ms |
| GEMM 4096³ f32 | 94.2 ms | CPU fallback | CPU fallback | 40.6 ms |
| GEMM 4096³ f16 | 84.4 ms | 982.7 / 997 ms | 115.1 / 135 ms | 460 ms |

Reading so far:
- **Elementwise/reduce**: Briev-L0 device time is ~1.3–2.2× the hand OpenCL C
  ceiling; NEO **beats** the reference (the C reference is a simple form).
  Wall adds 1–3 ms of per-launch submission/copy — the residency gap (W3).
- **f16 native arithmetic is real** (`OpFAdd %half`): elem f16 is ~2× faster
  than f32 on L0 (0.106 vs 0.201 ms).
- **f32 GEMM on Intel lanes** is a documented capability boundary (Vulkan
  image only) — NA, not a lane result.
- **f16 GEMM 4096³** is 12× off the ceiling on L0/NEO and 5× on Vulkan: the
  `matmul_f16!` path (explicit casts, f32 accumulate) is not reaching the
  tensor/cooperative lowering at this shape. Attribution item for W3.
- `g0_sum` (Int fold, serial form) shows 41.9 ms wall on Vulkan: the fold lane
  did not fire for that build; device timestamp said 0.003 ms — a clear signal
  the Vulkan timestamp path is broken, not the kernel.

### Timing-method caveat (OPEN)

Vulkan `# gpu_time` is not trustworthy in this stack: a 1Mi elementwise kernel
reports 0.000–0.001 ms while wall is 0.33 ms, and g0_sum reports 0.003 ms while
wall is 41.9 ms (the serial reduce really does ~4B ops). OpenCL/L0 event
timings are consistent with the ceilings. Until the Vulkan timestamp path is
fixed (query-pool reset/submit scope), Vulkan Briev numbers are **wall-only**
and marked provisional. Briev GEMM 4096 f16 wall (460 ms) vs device-timestamp
(8.8 ms) is the same defect.

Next: fix Vulkan timestamp measurement; then attribute the L0 ~2× elementwise
gap (byte-offset state addressing / index math) and the f16-GEMM
tensor-lowering gap; implement residency for L0/NEO (wall → device).

## W2d CORRECTED — Vulkan ticks fixed (2026-11-09)

Vulkan timestamps are device TICKS; the driver treated them as ns. Now scaled
by `VkPhysicalDeviceLimits::timestampPeriod` (**52.0833 ns/tick** on B390).
Device times are credible and consistent with wall for large kernels.

| shape | ceiling (CL C) | Briev-Vulkan | Briev-L0 | Briev-NEO | ratio Vk / L0 / NEO vs ceiling |
|---|---|---|---|---|---|
| elem 1Mi f32 | 0.080 | **0.076** | 0.205 | **0.025** | 0.95x / 2.6x / 0.31x |
| elem 1Mi f16 native | 0.047 | **0.046** | 0.104 | **0.019** | 0.98x / 2.2x / 0.40x |
| reduce 4096×1024 f32 | 0.173 | 0.353 | **0.166** | (flaky) | 2.0x / 0.96x / — |
| reduce 4096×1024 f16 | 0.097 | 0.232 | 0.133 | **0.029** | 2.4x / 1.4x / 0.30x |
| GEMM 512³ f32 | 0.421 | **0.123** | CPU fallback | CPU fallback | **0.29x** / — / — |
| GEMM 512³ f16 | 0.233 | 0.957 | 0.516 | 0.563 | 4.1x / 2.2x / 2.4x |
| GEMM 4096³ f32 | 94.2 | **39.7** | CPU fallback | CPU fallback | **0.42x** / — / — |
| GEMM 4096³ f16 | 84.4 | 459.6 | 983.6 | 115.2 | 5.4x / 11.7x / 1.36x |

Attribution:
- **Vulkan f32 GEMM beats the hand OpenCL C reference** at both 512³ (0.29x)
  and 4096³ (0.42x): the derived tensor/vec4 path works; the C reference is a
  simple 64×64 scalar-accumulate kernel, so the lane's real ceiling is above
  it (a GLSL/XMX reference is W2c).
- **Elementwise f32/f16: Vulkan at parity with the C ceiling**; L0 ~2–2.6x
  slower (byte-offset state addressing / index math / no vec4); NEO **~2.5–5x
  faster** than the simple C reference (its compiler vectorizes the same
  source form; the reference is the weak link, not the lane).
- **f16 GEMM is the outlier**: Vulkan 4–5x, L0 11.7x, NEO 1.4x off. The
  `matmul_f16!` body (explicit casts, f32 accumulate) does not reach the
  tensor/coopmat lowering; it takes the general kernel. Highest-value W3 item.
- f32 GEMM on Intel lanes remains the documented Vulkan-only boundary.
- Wall in `.bv` paths is dominated by the full-copy launch (e.g. g0_elem
  8 ms wall vs 0.028 ms device): L0/NEO residency is the other W3 item.
- Remaining timing caveat: NEO occasionally hangs/times out on a gate
  (BUGS.md exact-fold hang); a NA row means the run did not return.

INDEX: matrix now usable for f32/f16 elem, reduce, f16 GEMM, Vulkan f32 GEMM.
Next: f16 GEMM tensor path, L0/NEO residency, and the GLSL ceiling reference.

## f16 GEMM root cause (2026-11-09)

The f16 GEMM rows miss the tensor/cooperative channel for a mechanical reason:
`mark_declared` writes the **composite name** into the `declared_composite`
marker, and `declared_matmul` gates on `"matmul"`. `matmul_f16!` marks
`"matmul_f16"` → general kernel (Vulkan 0.84–0.96 ms at 512³ vs 0.123 ms for
the f32 tensor path; 459 ms at 4096³ vs 39.7 ms).

Two clean fixes (pick one, not a name list in analysis):
1. Give `$defn` a declared kind (`declared_kind: matmul`) that `mark_declared`
   writes instead of the macro name; `matmul_f16` declares it. One mechanism,
   no analysis name matching.
2. Fold `matmul_f16` into `matmul!` by allowing `as acc_ty` in the composite
   expander (type-param substitution in cast position) and eliding identity
   casts; then the single declaration carries storage + accumulator types.

Also committed alongside this investigation:
- alias canonicalization: `Float16` (alias fact) compares as `Half` in binary
  ops, `types_compatible`, and bindings (one format, two spellings).
- integer/float literal admission at a float target when exactly representable
  (`let acc: Half = 0;` works; `4.1` still rejected) — enables the canonical
  `matmul!(..., Half)` body.

## f16 GEMM: declaration kind fixed; kernel quality still open (2026-11-09)

Fix landed: composites declare their specialization kind
(`attr declared_kind: matmul;`), `mark_declared` writes the kind (not the
macro name) and finds the first `let`, and the shape matcher unwraps
declared widen/narrow casts. `matmul_f16!` now reaches the GEMM channel:
Vulkan emits `OpCooperativeMatrixKHR` + v4half workgroup tiles (previously a
LocalSize-256 naive kernel).

**But the f16 kernel still loses badly to the f32 tiled path on this device:**

| GEMM 512³ f16 | 1.05 ms (coopmat) | 1.70 ms (tiled, coopmat off) | f32 tiled: 0.123 ms | CL C ceiling: 0.233 ms |
|---|---|---|---|---|
| GEMM 4096³ f16 | 371 ms (coopmat) | 459 ms (tiled) | f32 tiled: 39.7 ms | CL C ceiling: 84.4 ms |

A/B via `spirv_coopmat: 0|1` (`/tmp/cfg_nocm`), same binary/config otherwise.
So: (a) the coopmat tier is the better f16 path here, but still ~9× behind the
f32 tiled kernel; (b) the Mesa/ANV coopmat path is not the hardware XMX
ceiling (0.74 TF vs the f32 SIMD path's 6.9 TF). This is a strategy fact, not
a lane verdict: the emitter needs an f16-storage/f32-accumulate tiled kernel
(like the C reference, 84 ms) and coopmat selection must follow measured
device facts. L0/NEO now correctly report no OpenCL projection for GEMM (the
documented Vulkan-only boundary) instead of running a general f16 kernel.

Tracked as W3: (1) f16 tiled microkernel with vec4/v4half staging and f32
accumulation; (2) strategy-fact coopmat selection; (3) re-run the f16 GEMM row.

## W3 — Level Zero residency (2026-11-09)

L0's staging is host-USM (device-visible), so residency is free: exposed
`mapped` (the staging pointer), `launch_dev`, `launch_dev2d` (dispatch only,
no copies), and reused ONE timestamp event per kernel (per-dispatch event-pool
creation cost ~2 ms and masked the win).

Effect (device gpu_time / launch wall, BRIEV_ACCEL_BENCH_ITERS=20):

| shape (L0) | device | wall before | wall after |
|---|---|---|---|
| elem 1Mi f32 | 0.122 ms | 1.56 ms | **0.143 ms** |
| elem 1Mi f16 | 0.100 ms | 1.02 ms | **0.113 ms** |
| reduce 4096×1024 f32 | 0.159 ms | 1.83 ms | **0.166 ms** |
| reduce 4096×1024 f16 | 0.134 ms | 1.78 ms | **0.140 ms** |

With residency L0 elementwise is 1.5x the CL-C ceiling (0.122 vs 0.080) and
reduce **beats** the simple C reference (0.159 vs 0.173). `.bv` gates whose
host nodes touch arrays still take the full-copy path by design (residency
requires kernel-pinned fields), so their wall remains copy-bound.

OpenCL/NEO still full-copies (persistent buffer + map is the remaining W3
item). f16 GEMM on L0/NEO remains the documented CPU-fallback boundary.
