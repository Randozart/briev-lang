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

## RESOLVED (2026-09-13): two overlapping causes, one real fix

**Cause 1 (most of the "anomaly"): my own driver args.** The dump bakes
`y_off = b_off + M·K·2 + 8`; my manual sweep args computed
`b_off + M·N·2 + 8`. They coincide only when K=N — so every k<128 sweep
point and 2048²×32 compared the reference against the WRONG y offset.
With dump-consistent offsets: K=32/48/64/96/128 at M=128 and k16-1024
all PASS exact. (k16-1024's first failure also had grid=16 instead of
64 — 3/4 of the tile never written.)

**Cause 2 (real kernel bug, FIXED): the empty mid-loop commit.** At
K=16 the fill-skip guard (`r2 ≥ k-16` = `r2 ≥ 0`) fires from kstep 0 —
the in-loop fill never runs and `FILL_DONE:`'s `cp.async.commit_group`
commits an EMPTY group every kstep. With the empty-commit path the
kernel returned all-zero y (the compute's ldmatrix read the stage as
never-filled) — even though the prologue had drained + barred before
the loop. Hand-patch bisection: never-skip → exact; skip-without-commit
→ exact; skip-with-empty-commit → zeros. **Fix: the commit moved INSIDE
the fill path (before FILL_DONE)** — skipped fills don't commit; the
prologue drain suffices. For K > 16·(stages-1) the tail ksteps' empty
commits disappear too — instruction stream for filling ksteps is
unchanged, only the label moved after the commit.

**Gates after the fix:** 128-K16/32/48/64/96/128 all exact; k16-1024
exact; recorded portfolio 2048³ 1.546e-3 / 4096³ 5.208e-3 / 8192³
9.115e-3 / 4096-k16 0.0 — all unchanged. Same-window 4096³ A/B pre/post
fix: overlapping ranges (34.7-35.1 vs 33.2-35.2), no regression beyond
window noise. 2178 lib tests green; E2E assembles clean.

**E8a still FAILs (rel 0.64) after the fix** — its stage-1 defect is
independent (the producer loop never commits empty groups). Remaining
suspects for next session: producer-side async-visibility semantics
(membar vs fence.proxy-class ordering for cp.async across the named
barrier), or the 64-lane rebased-fill D-mapping at stage parity.
