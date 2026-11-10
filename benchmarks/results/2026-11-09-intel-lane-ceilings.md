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
