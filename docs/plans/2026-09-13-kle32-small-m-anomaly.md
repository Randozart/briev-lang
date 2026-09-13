# Hunt: ship PTX GEMM kernel — K≤32 small-M all-zero y

**Date:** 2026-09-13
**Discovered:** E8a debug session (BUGS.md 2026-09-13 entry). Gates E8a
and any small-K GEMM (decode-shaped attention head matrices live at
K=64-128).

## Symptom

Ship f16acc kernel (E4c, (2,4)@256T stages=2), correctness gate:

| shape | verdict |
|---|---|
| 4096×4096×16 | PASS 0.0 exact (the recorded k16 gate) |
| 1024×1024×16 | FAIL 1.0, worst y[0] |
| 128×128×16 | FAIL 1.0, worst y[15309] |
| 2048×2048×32 | FAIL 1.0, worst y[0] |
| 2048³ / 4096³ / 8192³ | PASS (the recorded portfolio) |

rel exactly 1.0 = stored y is 0 where ref ≠ 0 (accumulators never
accumulated, or the y-pass stored zeros). k16-4096 and k16-128 kernels
are structurally identical (constant-normalized diff empty) — the
trigger is a constant/grid threshold, not a code path.

## Bisection plan

1. **K sweep** at M=N=128: K ∈ {16,32,48,64,80,96,112,128} — find the
   threshold. Hypothesis space: ksteps-per-tile count (1..8) or the
   prologue/fill-skip guard boundary (`k - 16·(stages-1)` ≤ 0 flips the
   KLOOP fill to always-skip at K=16/32 — the always-skip path is only
   exercised at tiny K).
2. **M sweep** at a passing K — grid-size sensitivity.
3. **f32-acc control** at the failing shape (BRIEV_GEMM_F16ACC unset,
   gate 5e-3) — f16acc-specific vs shared.
4. Driver-artifact control: the check seeds a[j]=(j%7)·0.25 (ZEROS at
   j%7==0) — for K=16 every A row has ≥2 zero elements; if the kernel
   is fine and the REFERENCE samples… (check computes its own ref from
   the same seeds — control already covered by the k16-4096 exact pass).

## Candidate root causes (ranked)

- **R1: the always-skip fill path at K ≤ 16·(stages-1).** The KLOOP
  fill guard `r2 ≥ k-16` is true from kstep 0 — the in-loop fill never
  runs; only the prologue feed exists. If the prologue's commit/wait
  accounting breaks when NO in-loop fill ever runs (e.g. wait_group
  counts with the empty in-loop commit), the compute may read unfilled
  smem. ksteps=1..2 shapes fail; k=4096 (256 ksteps, fills running)
  passes; **k16-4096 passes though** — weakens R1 unless the failure is
  data/timing-dependent (race → grid-size-sensitive).
- **R2: a race that large grids mask.** Small grids = fewer CTAs/SM =
  different warp interleaving; a prologue-visibility race (membar/bar
  insufficient at exactly this schedule shape) would be
  non-deterministic per launch — the check is one launch.
- **R3: driver/check artifact at small M·N** — the 4096 exact pass uses
  identical seeding logic; unlikely.

## Protocol

Sweep → minimal failing shape → single-CTA capture (grid=1) → hand-patch
the PTX (e.g. replace cp.async with ld/st for the prologue) to separate
addressing from async-visibility → fix → gates at ALL newly-covered
shapes (the sweep grid) + the recorded portfolio → E8a correctness
re-test (its stage-1 failure plausibly shares this root cause).

## Undo

Any fix must keep the recorded portfolio green (2178 tests + the on-device
gates) and stay additive (no weakening of the fill-skip guard semantics
for K > 16·(stages-1)).
