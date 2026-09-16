# Shape-driven GPU strategy selection — the analysis that picks the efficient kernel per shape

**2026-09-16.** The goal: the compiler picks the most efficient codegen
strategy (tile, stage count, load path, warp geometry) for EVERY GPU shape
automatically — the "maximum efficient default" made real. Imitate cuBLAS's
best properties (big tiles, multi-stage pipelines, runtime autotune) with
Briev's best properties (analysis-driven frontend dispatch, the tensor
pipeline primitives, the output-equality correctness gate). No hand-tuned
config, no per-benchmark match arms.

## Why this plan

- Every shape is efficient in SOME use case: small shapes win with small
  tiles (parallelism), large with big tiles (intensity), thin-K with no
  pipeline, deep-K with a pipeline. The selection is a cost model over a
  candidate strategy space — uniform, not a lookup table.
- Today only ONE shape-derived decision exists (`select_mw_nw`,
  `src/backend/ptx/mod.rs:1352` — divisibility walk with hardcoded measured
  preference). Everything else (stages, warp_mh, load path, fused tile) is
  config knobs or hardcoded.
- The kv-staged experiment (2026-09-16) proved the load path is secondary:
  a wash (+9% @512², −10% @1024²). The dominant gap is tile size — a 16-row
  fused block gives zero Kt/V reuse across m-tiles (64× redundant Kt/V
  re-reads at 1024² vs cuBLAS's 128×128 tile reusing 8×).
- Reframe from the fusion: the 2-kernel composition (0.577 ms @512²) is only
  3% behind the 1-kernel fusion (0.559 ms) — the S' HBM round-trip is cheap.
  The attention gap is that BOTH our GEMMs run ~1 TF at 512³ while cuBLAS
  gets ~8.5 TF. Same tile problem, no fusion needed to see it.

## The calibration ground truth

cuBLAS decompiled map (RTX 3060, 7 shapes → 4 kernels, 2026-09-16):

| Shape | cuBLAS kernel | Tile | Stages |
|-------|---------------|------|--------|
| 64³   | CUTLASS WMMA 32×32 | 32×32 | 1 |
| 128³  | CUTLASS WMMA 32×32 | 32×32 | 2 |
| 256³  | cuBLAS-native 64×64 | 64×64 | 5 |
| 512³  | cuBLAS-native 96×128 | 96×128 | 4 |
| 1024³ | cuBLAS-native 128×128 | 128×128 | 1 |
| 2048³ | cuBLAS-native 128×128 | 128×128 | 1 |
| 4096³ | CUTLASS mma 256×128 | 256×128 | 3 |

CAUTION: cuBLAS autotunes at runtime — its exact stage counts (1024³=1
stage) are empirical search artifacts, not formula outputs. Calibration
targets the tile/stage FAMILY per shape, not exact counts.

## Stage 0 — Measure + calibrate (no codegen changes)

### 0a. Baseline measurement
Map OUR tensor GEMM TF across {64³,128³,256³,512³,1024³,2048³,4096³} and the
attention composition at {512²,1024²} vs cuBLAS, via existing harnesses
(`benchmarks/gpu/ptx_gemm_bench.c`, `cublas_attn_bench.cu`). Answers: is the
~1 TF at 512³ a tile/occupancy issue or a wiring gap? May reveal something
simpler than a cost model (a fix, not a selector).

### 0b. Cost model — `src/analysis/gpu_strategy.rs`
- `GpuHardware` table per target (measured sm_86 constants from the ledger:
  102 TF f16acc, 360 GB/s, 3 MB L2, 100 KB smem/SM, 48 KB static cap,
  65,536 regs/SM, 28 SMs). Lives in config (measured, not guessed).
- `candidate_strategies(m,n,k)` → pruned set: tiles {32×32…256×128} × stages
  (from K-depth) × load path, filtered by divisibility, smem ≤ cap, threads
  ≤ 256, CTA count ≥ SM fill.
- `estimate_time()` → roofline (flops/peak, bytes/BW) + L2-aware traffic
  (the "L2 sharing dominates" lesson) + occupancy waves.
- `select()` → min estimated time.

### 0c. Calibration test
Model predictions for the 7 map shapes must match cuBLAS's tile/stage
family (tolerated mismatch documented — cuBLAS's empirical residue). Iterate
constants until green. The cost function is Briev's, not cuBLAS's — the map
is the validation, not the target.

## Stage 1 — Wire the selector into the tensor GEMM dispatch

Replace hardcoded `select_mw_nw` + auto-stages in `build_ptx_kernels`
(mod.rs:1730-1785) with `gpu_strategy::select`. Gate: 4096³ stays 35.5 TF,
small shapes improve, lib tests green (update the pinned shape→(mw,nw)
tests). The "imitate cuBLAS" core: Briev's pipeline primitives + cuBLAS's
tile selection, chosen by analysis.

## Stage 2 — Attention at big tiles (direction gated on 0a)

- If composition hits cuBLAS-comparable after Stage 1: fusion is a +3%
  nicety — keep the 1-kernel, don't chase it.
- If S' round-trip is the residual: the k-chunked fused kernel
  (S'-preserving, chunk over kn with the T-slab on-chip) or the documented
  reassociation `Q·(Kt·V)` — decided by measurement, respecting f16
  numerics (interpreter is the reference).

## Stage 3 — Runtime autotune (the hybrid, cuBLAS's mechanism)

Extend the `accel_probe` machinery: emit top-k candidates from the Stage 0
model, probe at first launch with the output-equality gate (the
`briev_accel_gpu_ok` pattern), commit the winner to a per-shape cache.
"Frontend proposes, runtime disposes" — the empirical residual no static
model can predict. Precedent: `src/analysis/accel.rs` Probe lane +
`lib/runtime/briev_accel_rt.c:710` (`briev_accel_probe`).

## Stage 4 — Cross-kernel validation

Rectangular GEMM (M≠N≠K), bias-add chain (prove generality beyond
attention), gemv — all through the selector.

## Docs

- This plan.
- New `docs/architecture/gpu-strategy-selection.md` (the model, the
  calibration, the frontend-proposes/runtime-disposes principle).
- `docs/architecture/agent-reference.md` §6 if a new standing principle
  emerges.
- `docs/plans/2026-09-16-panel-pipeline-generalization.md` Step 3 status.
- BUGS.md for defects found.

## Verification (per stage)

1. Stage 0: calibration test green (7-shape family match); baseline table
   recorded (Briev vs cuBLAS per shape).
2. Stage 1: 4096³ no regression (35.5 TF); `cargo test --lib` green; Praetor
   on changed files.
3. Stage 3: probe correctness gate + cached winner; no flapping (margin).
4. Full suite + on-device harnesses at each stage.

## Honest framing

This turns the two biggest wins — the staged pipeline (35.5 TF) and the
analysis discipline — into ONE compiler property: the strategy chosen from
shape evidence, calibrated against the competitor, resolved by measurement
at runtime where static cannot predict. Parity-or-beyond is the yardstick.