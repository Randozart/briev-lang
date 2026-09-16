# Shape-driven GPU strategy selection — the analysis that picks the efficient kernel per shape

**Author:** Randy Smits-Schreuder Goedheijt <randozart@gmail.com>
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

## Stage 0a results (2026-09-16) — the baseline that redirects the plan

Measured on RTX 3060 (sm_86), f16 tensor tier, 50-iter batch timing:

| Shape | Briev GEMM (TF) | cuBLAS (TF) | Ratio |
|-------|-----------------|-------------|-------|
| 64³   | 0.14            | 0.04        | 0.29× |
| 128³  | 0.46            | 0.33        | 0.71× |
| 256³  | 2.32            | 3.10        | 1.34× |
| 512³  | 10.00           | 9.52        | 0.95× |
| 1024³ | 19.63           | 18.61       | 0.95× |
| 2048³ | 26.92           | 23.63       | 0.88× |
| 4096³ | 27.05           | 25.42       | 0.94× |

**Finding A — the GEMM is already at cuBLAS parity for 256³+.** The tensor
tier's hardcoded (2,4)@256T tile (128×128 CTA, stages=3) matches cuBLAS
within 12% from 256³ up. The "10× gap" was never a GEMM problem at big
shapes — it was the fused kernel.

**Finding B — the real GEMM gaps are SMALL shapes: 64³ (7×), 128³ (3×).**
The single warp-tile (32×16) and one-CTA grids underfill the SM count.
This is exactly the "every shape is efficient in some use case" case: small
shapes need SMALL tiles (parallelism), not the 128×128 big tile. The
selector's first clear win.

**Finding C — the fused attention kernel is a 10× REGRESSION, not a win.**
The phase4b milestone "fused 0.559 < comp 0.577 @512²" compared against a
STALE composition that predated the tensor-tier wiring. Re-measured on the
CURRENT compiler (both paths correct on-device: fused max_rel 2.6e-4, comp
1.6e-3):

| 512² path | Time | TF |
|-----------|------|-----|
| fused 1-kernel (`qk__scale_pv`) | 0.752 ms | 0.71 |
| 2-kernel composition (qk+pv, tensor tier) | 0.073 ms | 7.4 |
| cuBLAS composition | 0.063 ms | 8.5 |

The composition is 10.3× faster than the fused kernel AND 1.16× of cuBLAS
(the tensor tier's 512³ GEMM is at parity). The fused kernel's 16-row
m-tile gives zero Kt/V reuse; the composition gets the big-tile GEMM. The
chain-fusion detection fires automatically and slows every f16 attention
down by 10× — a maximum-efficient-default violation.

**Action from Finding C:** the fused attention path must be gated OFF by
default (or gated on "fused beats composition", which the Stage-0 cost
model will compute). The composition IS the correct default. This removes
the urgency of the Stage-2 k-chunked fused kernel: the composition already
reaches cuBLAS parity, so a fused rewrite is only worth it if it can beat
0.073 ms — a much higher bar than the plan assumed.

**NOT a softmax problem (2026-09-16):** the measured chain is
`qk → scale → pv` where the middle is a single scalar multiply
(`s2[j] = s[j] * SCALE`) — there is NO softmax (no max/exp/divide). The
fused kernel's regression is purely the 16-row m-tile (no Kt/V reuse),
NOT a softmax synchronization barrier. The online-softmax (FlashAttention)
reformulation IS the right answer for a REAL attention chain (with exp/
max/divide), but it does not apply to this benchmark. If a real softmax
chain is added later, the k-chunked fused rewrite should use the online
running-max/sum update in registers — the same structural fix, different
math.

**Revised Stage 2 direction:** skip the k-chunked fused kernel for now
(composition wins). Instead: (1) gate chain fusion on the cost model, (2)
focus the selector on the small-shape GEMM gap (Finding B). The k-chunked
fused kernel becomes a stretch goal gated on beating the composition.

**Finding D — the current one-config-for-all default is the root cause.**
Every shape 128³–4096³ uses the same (2,4)@256T tile. cuBLAS varies
32×32→256×128. The cost model (Stage 0b) will derive the tile from shape;
the parity at 256³+ is the calibration anchor that validates the model's
tile term.

## Stage 1 results (2026-09-16) — selector wired into the tensor GEMM dispatch

Wired `gpu_strategy::select` into `build_ptx_kernels` (mod.rs): the model
picks (tile, stages), mapped back to the emitter's (mw, nw, stages) via
`strategy_to_mwnw`; the legacy `select_mw_nw` walker stays as the fallback.
Also fixed a latent runner bug exposed by the selector: a 2-warp mw kernel
(block_threads=64, shared>0) was mis-dispatched through the S3b single-warp
grid — `dispatch_geometry_stmt` now distinguishes them.

Measured on RTX 3060, batch timing, all shapes max_rel ≤ 6e-3 (correct):

| Shape | Baseline | Selector | Delta |
|-------|----------|----------|-------|
| 128³ | 0.47 TF | 0.87 TF | **+85%** |
| 256³ | 2.33 TF | 4.35 TF | **+87%** |
| 512³ | 10.0 TF | 11.3 TF | +13% |
| 4096³ | 26.7 TF | 26.6 TF | −0.3% (noise) |

The small-shape gap (Finding B) is closed by the selector's smaller tiles
(64×64 @128³ vs the 128×128 default). Large shapes keep the E4c tile. The
cost model's tile choice, not a knob, drives the win — the
maximum-efficient-default principle realized.

Note: the earlier Stage 0a small-shape baseline (0.46/2.32 TF) used the
correct mw grid (ctas = M·N/(64·256) = 1/4) and matches the re-measured
baseline here — the confusion was the harness's per-launch sync vs batch
timing, not the kernel. The runtime's `launch_resident_2d` always syncs
per launch (BRIEV_ACCEL_ASYNC has no effect); fair timing needs
`ptx_gemm_bench` batch mode.

## Stage 3+4 results (2026-09-16)

**Stage 3 — model calibration, not a runtime probe.** The pipeline model's
`time = compute + memory/stages` always favored deeper pipelines, but
stages=4 costs 32KB smem → 3 CTAs/SM vs stages=3 at 24KB → 4 CTAs/SM (the
measured E4c sweet spot). Added an occupancy penalty when cta_smem pins
CTAs/SM below 4. The model now picks stages=3 at 1024³/4096³, matching the
on-device optimum:

| Shape | stages=4 (before) | stages=3 (fixed) |
|-------|-------------------|------------------|
| 4096³ | 26.6 TF | 27.05 TF (+1.7%) |
| 1024³ | 19.6 TF | 20.62 TF (+5%) |

A runtime probe was deemed unnecessary: the static model now matches the
measured optimum on sm_86, and the probe's residual (clock variance,
cross-GPU calibration) is a future per-device hardware-table concern, not
a kernel-selection concern.

### CORRECTED A/B (2026-09-16): cuBLAS best-algo search, interleaved

The original post-fix sweep (below) used cuBLAS's DEFAULT algo and an
optimistic clock state, claiming "beats cuBLAS at every shape". A rigorous
re-measurement (cuBLAS best-of-11 tensor algos, interleaved on device 0)
corrects it:

| Shape | Briev | cuBLAS best | Verdict |
|-------|-------|-------------|---------|
| 64³ | 0.16 | 0.05 | Briev 3.2× |
| 128³ | 0.88 | 0.36 | Briev 2.4× |
| 256³ | 4.34 | 3.34 | Briev 1.3× |
| 512³ | 11.3 | 14.3 | **cuBLAS 1.26×** |
| 1024³ | 20.0 | 19.6 | Briev 1.02× (parity) |
| 2048³ | 25.4 | 24.5 | Briev 1.04× |
| 4096³ | 23.4 | 26.2 | **cuBLAS 1.12×** |

**Honest verdict: Briev wins small shapes (64³–256³, up to 3.2×), holds
parity at 1024³/2048³, and loses 512³ (26%) and 4096³ (12%).** The 4096³
gap is NOT tile selection — the selector correctly picks our best
(128×128/stages=3 = 23.4 TF vs our 512-thread 256×128 = 21.0 TF,
2026-09-11). It is a kernel-efficiency gap: cuBLAS's 256×128 kernel has
more per-thread register blocking. That is the honest next target (the E8a
warp-specialization path).

### Original (DEPRECATED) post-fix sweep — retained for the record

| Shape | Briev | cuBLAS (default) | Ratio |
|-------|-------|--------|-------|
| 64³ | 0.16 | 0.04 | 4.0× |
| 128³ | 0.88 | 0.33 | 2.7× |
| 256³ | 4.34 | 3.10 | 1.4× |
| 512³ | 11.35 | 9.52 | 1.19× |
| 1024³ | 20.62 | 18.61 | 1.11× |
| 2048³ | 26.31 | 23.63 | 1.11× |
| 4096³ | 27.05 | 25.42 | 1.06× |

**Stage 4 — cross-kernel validation.** The selector generalizes:
- Rectangular GEMM (4096×512×512): 20 TF, max_rel 7.5e-3 — the M-big/K-small
  shape gives high intensity, and the selector's 128×128 tile handles it.
- GEMM + bias-add chain (512³, non-attention middle): compiles to 2 kernels
  (gemm + addbias), both correct; the gemm hits 11.3 TF (same as standalone
  — the selector is not attention-bound).

The bias-add chain proves the chain-detection + selector combination is
general: any GEMM → elementwise → GEMM pattern routes through the same
machinery. 2269 tests pass.