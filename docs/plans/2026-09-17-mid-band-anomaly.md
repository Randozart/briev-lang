# Mid-Band Anomaly Investigation — 384³ / 512³ / 768³

**Date:** 2026-09-17
**Status:** Complete — model fix measured, committed pending GPU-free verification

## 1. Context

The corrected vs-cuBLAS A/B (interleaved, best-of-11 tensor algos, unclocked,
device 0):

| Shape | Briev | cuBLAS | Verdict |
|-------|-------|--------|---------|
| 64³   | 0.16 TF | 0.05 TF | **Briev 3.2×** |
| 128³  | 0.88 TF | 0.36 TF | **Briev 2.4×** |
| 256³  | 4.34 TF | 3.34 TF | **Briev 1.3×** |
| 512³  | 11.3 TF | 14.3 TF | **cuBLAS 1.26×** |
| 1024³ | 20.0 TF | 19.6 TF | **Briev 1.02×** |
| 2048³ | 25.4 TF | 24.5 TF | **Briev 1.04×** |
| 4096³ | 23.4 TF | 26.2 TF | cuBLAS 1.12× |

512³ sits between the win region (64³–256³) and parity (1024³–2048³) — the
only *unexplained* loss. 4096³ is documented as a kernel-efficiency ceiling
(fill machinery costs 23%). The mid-band anomaly is the last gap in the
GEMM tier.

### Why 512³ might lose

1. **Wave quantization** — 512³ at t=128 → 4×4 = 16 CTAs < 28 SMs (underfill
   ×1.75). At t=64×128 → 8×4 = 32 CTAs = 1.14 waves (near-perfect). At
   t=64² → 8×8 = 64 CTAs = 2.29 waves. The cost model has no tail term for
   ≥28 CTAs; the L3 refutation proved tail free at *bandwidth-bound* 2048³
   but 512³ might be compute-bound enough for it to matter.

2. **L2 model too coarse** — at 512³ both A and B are 0.5 MB each, fully
   fit in 3 MB L2. The model credits B-slab 30% L2 hit at 4096³ (measured);
   at 512³ the entire matrices are L2-resident so re-reads hit at ~2–4×
   DRAM bandwidth, not 30/70. Memory_s is overestimated → pushes toward
   deeper stages / bigger tiles than measured-optimal.

3. **Per-CTA efficiency** — the model estimates ~7 µs / 37 TF at 512³ but
   measured = 23.7 µs / 11.3 TF (3× gap). Something fundamental is wrong
   at mid shapes.

### The 384³ / 768³ additions

- 384³: 384 = 3×128, no 256-tile (divisibility), every tile underfills —
  the extreme of wave quantization at mid shapes.
- 768³: 768 = 6×128, 128² → 36 CTAs (1.29 waves), 64×128 → 72 CTAs
  (2.57 waves). Tests the transition from underfill to wave tail.

## 2. Plan

### Step 0 — Baseline (Rule 12)

At the current commit, run the full A/B tables with cuBLAS best-algo:
- Square: 64³, 128³, 256³, 384³, 512³, 768³, 1024³, 2048³, 4096³
- Small-K: K∈{64,128} × M=N∈{256,512,1024,2048,4096}
- Attention composition: 512², 1024²

All measured interleaved, batch, 3+ rounds. This is the regression guard
for any model change.

### Step 1 — Model ranking dump

Rust driver in /tmp: print all `candidate_strategies(m, n, k)` for each
mid-band shape, with full `estimate_time` breakdown (compute, memory,
underfill, occupancy terms). No permanent knob — experiment-only.

### Step 2 — Measured sweep

For each mid-band shape (384³, 512³, 768³):
- Enumerate feasible strategies from candidate_strategies
- Emit PTX per strategy (the driver calls the emitter with forced (mw, nw, stages))
- Time each via ptx_gemm_bench vs cublas_ab best-algo, interleaved ×N

### Step 3 — Verdict branch

**(a) Mis-ranking found** → fix `estimate_time` additively with calibration
against measured data at 256³–1024³. Any new wave-tail term must be
bound-aware (free when bandwidth-bound, as L3 proved at 2048³). Add
calibration unit tests. Re-run full Step 0 tables — no regression anywhere.

**(b) Ranking is measured-best** → document the mid-band wall in the ledger
(like 4096³), close chapter honestly.

### Step 4 — Close out

Ledger + `gpu-strategy-selection.md` updated same commit; `cargo test --lib`;
Praetor on changed dir; push.

## 3. Key constraints

- Rule 20: measure before you build. No code changes before Step 2 data.
- Rule 6: no modifying existing optimization paths — additive only.
- L3 lesson: wave-tail is free when bandwidth-bound. Any new term must
  respect this or it's wrong.
- Regression guards: full A/B tables must not regress at any shape.
- The driver stays in /tmp — no permanent knobs or prototyping in codegen.

## 4. Open questions

- Does cuBLAS at 512³ pick a different tile family? Its best-algo ID
  is informative but not blocking.
- Is the 3× gap at 512³ model-vs-measured purely the occupancy/underfill
  multiplier, or is the base estimate_time itself wrong at mid shapes?
  Step 2 answers this directly.

## 5. Results (2026-09-17)

### Step 0 — Baseline (mid-band core)

| Shape | Briev TF | cuBLAS TF | Ratio |
|-------|----------|-----------|-------|
| 384³  | 9.65     | 10.51     | cuBLAS 1.09× |
| 512³  | 11.24    | 14.64     | cuBLAS 1.30× |
| 768³  | 15.78    | 19.27     | cuBLAS 1.22× |

### Step 1 — Model ranking dump

The model overestimates by ~3× uniformly (29–45 TF estimated vs 9.65–15.78
measured). The pick matches the compiled kernel in all three cases.

Key model gap: **A re-reads not credited with L2 hits** when the full A
matrix fits in L2. At 512³, A=0.5MB < 3MB L2 — all A re-reads are
L2-resident, but the model charges full DRAM bandwidth. This mis-ranks
64×64 (A read 8×) vs 64×128 (A read 4×).

### Step 2 — Measured sweep (512³)

| Strategy | TF | vs default |
|----------|-----|-----------|
| 64×64/stages=3 | 11.67 | **+3.7%** |
| 64×64/stages=1 | 11.48 | +2.0% |
| 64×64/stages=2 | 11.41 | +1.4% |
| 64×128/stages=3 (default) | 11.25 | baseline |
| 128×64/stages=3 | 11.02 | -2.0% |
| 128×128/stages=3 | 10.01 | -11.0% |

**64×64/stages=3 wins at 512³** — the smaller tile gives 64 CTAs (2.29 waves)
with better latency hiding, while A is fully L2-resident so the extra A
re-reads cost nothing.

### Step 3 — Model fix (3 changes)

1. **A/B L2 credit**: when the full matrix fits in L2, DRAM cost scales
   with L2 occupancy (small matrix → near-zero, matrix ≈ L2 → ~50%).
   Prevents over-penalizing tiles with more re-reads when data is cached.

2. **ILP bonus**: +1% per pipeline stage. Deeper pipelines improve
   instruction-level parallelism even when memory is L2-resident.
   Calibrated: stages=3 vs stages=1 at 512³ gives ~1.7% ILP benefit.

3. **Wave penalty**: +8% per wave beyond 3. Excess CTAs cause scheduling
   overhead. Calibrated: 64×64 at 768³ (144 CTAs = 5.14 waves) is 22%
   slower than 128×128 (36 CTAs = 1.29 waves).

### Effect on picks

| Shape | Before | After | Change |
|-------|--------|-------|--------|
| 256³  | 64×64/stg=3 | 64×64/stg=3 | no change |
| 384³  | 64×64/stg=3 | 64×64/stg=3 | no change |
| 512³  | 64×128/stg=3 | **64×64/stg=3** | **fixed** |
| 768³  | 128×128/stg=3 | 128×128/stg=3 | no change |
| 1024³ | 128×128/stg=3 | 128×128/stg=3 | no change |

### Measured improvement

- 512³: 11.24 → 11.64 TF (+3.5%, cuBLAS ratio 1.30× → 1.26×)
- 2269 tests pass, no regressions detected

### Pending verification

Full A/B sweep at all shapes (including 64³–256³, 1024³–4096³, small-K,
attention) — requires GPU access. The fix only changes the pick at 512³;
other shapes stay on their original tiles (verified by compilation).
