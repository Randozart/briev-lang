# E7: persistent grid-stride tiles — then E8: warp-specialization

**Date:** 2026-09-13
**Baseline:** E4c/E6 ship — (2,4)@256T warp_mh=4 stages=2 K16, 16KB/CTA,
4 CTAs/SM. 35.5 TF @4096³ (84.5% of the 42-TF cuBLAS anchor), 36.3
@8192³ (86%), 31.5 @2048³ (75%). The E5/E6 ladders swept tiles, stages,
k-depth, lookahead, bank layout — every deeper-pipeline axis loses a CTA
slot and loses net. Two axes remain, both structural: **launch-level
tail waste** (E7) and **barrier cadence** (E8).

## E7 arithmetic (the case for persistence)

Wave quantization at 4 CTAs/SM × 28 SMs = 112 slots:

| shape | CTAs | waves | tail util | idle loss |
|---|---:|---:|---:|---:|
| 2048³ | 256 | 2.29 | 76% | **~24%** |
| 4096³ | 1024 | 9.14 | 91% | **~9%** |
| 8192³ | 4096 | 36.6 | 99% | ~1% |

Full-tail recovery bounds: 2048³ → ~40, 4096³ → ~38.5, 8192³ flat.

## E7 design

**Kernel** (`tensor_gemm_ptx_smem_mw_opt`): the entire per-tile body —
CTA decode (r3/r4, rd2/rd3/rd6), prologue fills, KLOOP, y-store — wraps
in a grid-stride loop:

```
%r30 = ctaid (first iteration), %r31 = %nctaid.x
PLOOP:
  decode from %r30; run the tile; y-store
  %r30 += %r31
  setp %r30 < n_tiles → PLOOP
```

- Full-grid launch ⇒ exactly one iteration ⇒ identical behavior and
  numerics (per-tile full-K, per-tile y-store; no split-K reduction).
- All per-tile state is recomputed inside the body already (verified);
  kstep `%r2` is KLOOP-local and re-initialized per tile.
- Register cost: +2 u32. Occupancy unchanged (64-reg budget absorbs it).

**Driver** (`ptx_gemm_bench.c`): 
`grid = min(ctas, SMs × cuOccupancyMaxActiveBlocksPerMultiprocessor(k))`
— occupancy discovered, not hardcoded. **Emitted runner** gets the same
clamp (device props queried at its init already).

**Protocol:** assemble 64-cap → correctness all shapes (exact recorded
signatures 1.546e-3 / 5.208e-3 / 9.115e-3 / k16 0.0) → interleaved A/B
×4 vs ship → **win bar: 4096³ ≥ 38, 2048³ ≥ 35, 8192³ no regression** →
commit per verdict.

## E7b (satellite, one dump): no-fill KLOOP at ship geometry

The no-fill ceiling was measured 29.9 TF in the (4,4)@512T mh2 era and
never at the (2,4)@256T mh4 point. Ppins whether the compute section
(ldmatrix+mma+bar) has headroom after E7 removes the tail — sizes the
E8 prize and decides the endgame.

## E8: warp-specialization (design-first, occupancy-constrained)

**Hard constraint** (the E5/E6 law): 4 CTAs/SM × 8 warps, ≤64 regs,
≤16KB smem. No variant may lose a CTA slot.

**Rejected by arithmetic up front:**
- 6-consumer grids — `(x,3)`/`(3,x)` warp grids fail N-divisibility
  (n % 96 ≠ 0 for the canonical shapes).
- 10-warp (8C+2P) — drops to 3 CTAs/SM; the measured occupancy tax is
  ~1.5 TF per slot and the variant must beat it first.

**E8a — split-phase barrier (prototype order 1, smallest delta):**
all 8 warps keep (2,4) mma tiles; fills issued by warp 0 alone; the
full-CTA per-kstep bar is replaced by two named barriers:
- fill-done: warp 0 `bar.arrive 1, 32` after commit; all warps
  `bar.sync 1, 256` before consuming (stage-visibility pair with
  membar.cta, mirroring the existing prologue note).
- compute-done: consumers `bar.arrive 2, 224` after mma; warp 0
  `bar.sync 2, 256` before the next fill overwrites the stage.
Attack surface = barrier cadence itself; warp grid and tiles untouched.

**E8b — 4 producers + 4 consumers (order 2):** consumers (2,2) grid →
CTA tile 128×64, 2048 CTAs, stall-free consumers. Only if E8a wins but
leaves a gap — per-SM mma width halves (16 vs 32 warps) and must be
funded by near-E1 consumer rates.

**Mechanics validated before the real kernel:** a 2-warp `bar.arrive`/
`bar.sync` toy (semantics on driver 580.178.04 / sm_86), then lockstep
iteration tracing on the prototype (deadlock guard: mismatched arrive
counts). Whole-warp roles only — no divergence. Uniform 64-reg budget.

**Risks:** ptxas scheduling surprises (E4b hoisting paradox — measure,
never assume); named-barrier deadlock; driver-clamp interaction with
batch mode.

## Endgame decision (post-E8)

- ~90% parity → declare the anchor honestly, then pivot: **GEMV family
  #2** (generalization) and/or the **DAG-derived `gpu_schedule` pass**
  (queued 2026-09-13: inter-node independence → sync-eliminated
  launches; y-lifetime buffer reuse → allocator ownership; epilogue
  fusion → populates the dormant `fusable_pairs` carrier; `dependents`
  + the `global_lifetime` last-consumer shape are the instruments. Own
  plan doc after the E8 verdict).
- E7b shows compute headroom and E8a/b stall → true producer-consumer
  deep-queue variant as its own plan, or accept the ceiling.

## Undo

E7: the loop wrapper is additive — grid=nctaid at full grid collapses to
the historical single-tile path; driver/runner clamp is launch-side only.
E8a: `bar.arrive` blocks are gated behind a config knob defaulting to the
ship schedule; off-path byte-identical (E2E-diffed).
