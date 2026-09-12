# 2026-09-13: DRAM-real microbench + f32 2-stage + tier boundary

Follow-on to `2026-09-11-ptx-mma-issue-ceiling.md` (read that ledger
first: fill widening, warp_mh=4 ship, E1-E3 record, window-1 record).

## State at plan time

PTX f16acc tier: 2048³ 27.9 / 4096³ 32.3 / 8192³ 34.1 TF (81% of the
42 TF cuBLAS anchor). Kernel-vs-sync-microbench residual ~5.3 TF at
4096³ (72% ratio). Sector-waste hypothesis REFUTED by analysis —
the residual needs a DRAM-real instrument.

## L-B: f32 2-stage experiment

`mod.rs`: `stages = if f16_acc { 2 } else { 4 }`. The f32 kernel at
(4,2)@256T stages=4 has smem 32768 → 1 CTA/SM at 128 regs. At stages=2
smem 16384 → 2 CTAs/SM — the occupancy play that won for f16acc.

- Dump via `select_mw_nw(4096,4096,256,2)` (dispatch-true), ptxas,
  correctness gate 5e-3, interleaved ×3 vs the s4 cubin
  (`tgemm_f32_mh2.cubin`, MW_SMEM 32768; s2 candidate MW_SMEM 16384;
  both 256T × 1024 CTAs, BRIEV_Y_ELEM=4).
- Win → flip the else-branch to 2. Loss → record, done.

## L-A: DRAM-real microbench (`dump_mma_dram_microbench`)

The sync microbenches read one broadcast address — zero DRAM. Four
variants from one generator, per-CTA tile bases from `%ctaid`
(256×128 tiles, mh4 geometry), k-loop over 256 stripes, fills from
computed `a[m][k]`/`b[k][n]` addresses (real D-mappings: A 16B `.cg`,
B 8B), real ldmatrix + 16 mma chains as consumer, store-only guard:

- `dr`  full pattern — MUST reproduce ≈29.7 TF (self-validation vs
  production)
- `drn` fills only — pure DRAM fill ceiling
- `dra` A stream only · `drb` B stream only

Driver: `ptx_gemm_bench` (state seeding already 32MB a + 32MB b),
512T × 512 CTAs, MW_SMEM 24576, BRIEV_GEMM_F16ACC=1 not needed (no y
contract beyond the guard — but keep error output sane).

Verdict branches:
1. Scheduling win visible (e.g. stripe order, stream interleave) →
   build the kernel change, full gates.
2. Fill/ldmatrix contention dominant → target smem access schedule.
3. Intrinsic latency wall → document the PTX-tier ceiling; pivot.

## Results (2026-09-13)

### L-B: f32 stages — wash, s4 stays

f32 s4 22.12 vs s2 21.99 TF avg (s4 wins 3/3 by 0.12 — noise). The
occupancy-doubling play does NOT pay for f32 (unlike f16acc). Dispatch
keeps `stages = 4` for f32; s2 dump kept as an artifact
(`tgemm_f32_s2.cubin`). Correctness exact (0.0 rel err).

### L-A: DRAM-real microbench — VERDICT: mma-schedule-bound

`dump_mma_dram_microbench` (variants n/a/b/f) at production geometry,
real fill D-mappings, real per-CTA tile addresses, light ldmatrix+xor
consumer, same-window ×3:

| variant | ms | TF-equiv | reading |
|---------|-----|----------|---------|
| b (B stream only) | 1.07 | 128.9 | B fill nearly free (L2 row sharing) |
| a (A stream only) | 1.43 | 95.7 | A fill fast |
| n (fills only) | 1.92 | 71.0 | full DRAM fill ceiling |
| f (fills + light consume) | 2.36 | 58.2 | +0.44 ms consumer cost |
| production mh4 | 4.27 | 32.1 | +1.91 ms beyond f |

**The fills are not the wall.** All fill traffic completes in 1.92 ms
of the kernel's 4.27 ms; the fill+consume microbench runs at 58
TF-equiv. The real kernel's extra ~1.9 ms (≈14 TF-equivalent) lives in
the mma section: per-mh A-fragment ldmatrix address computation, the
16 mma chains' dependency schedule, and the store-only epilogue. The
earlier "5.3 TF DRAM residual" attribution (E1/window-1) is dead —
the sync microbenches were mismeasuring because their consumer is
trivial, not because their fills are broadcast.

Caveats (documented in the generator): the B smem layout is a plain
k-major bijection (no XOR swizzle) and the consumer drops mma pressure,
so absolute numbers sit above production by design; only the
decomposition is the measurement. Stripe coverage is 255/256 (0.4%).

**Next lever (revised): the mma section schedule** — per-fragment
address precomputation structure, fragment-load-to-mma dependency
chaining, and the promo/epilogue pass. The 2026-09-10 hoisting
experiment (11% LOSS from hoisting invariant B addressing) is the
puzzle to re-examine under this new attribution: if the section is
issue-bound, the fix is fewer/fused address ops per fragment, not
hoisting.

### L-C: 2048³ tier boundary

(checked 2026-09-13) — see ledger note below.

## Gates (all steps)

Byte-identical correctness vs the shipped kernel's per-shape errors
(5.208e-3 @4096³ f16acc, exact K=16), no reg/occupancy regressions,
2119 lib tests green, Praetor no new diagnostics in changed ranges.
