# Parity probe: stages=3 (the never-measured CUTLASS default) + the fill instrument

**Date:** 2026-09-14
**Baseline (ship, E4c):** (2,4)@256T warp_mh=4 stages=2 K16, 35.5 TF
@4096³ (84.5% of the 42-TF cuBLAS anchor), 36.3 @8192³, 31.5 @2048³.

## Why this probe exists

The E-series closed with the conclusion that every deeper-pipeline axis
"pays a CTA slot on this 28-SM/100KB part." That conclusion rests on
three measurements: stages=4 (32KB → 3 CTAs, −2%), kps=2 (32KB → 3 CTAs,
−3%), and (4,4)@512T (256×128 tile → 2 CTAs, −3%). **All three lose a
CTA.** The one depth point that does NOT lose a CTA was never measured:
**stages=3 at 24KB → 4 CTAs (96KB ≤ 100KB), registers unchanged.**

E6 P1 (stages=3) died on a *correctness* bug, not on perf:
`and.b32 %r9, %r9, stages-1` aliases non-power-of-2 stages (a stages=3
dump measured 2.4e-2, 5× over the gate), so the probe was abandoned and
`debug_assert!(stages.is_power_of_two())` went in. The E6 doc records
the path forward explicitly: *"true-modulo stage math would buy it if
ever needed."*

stages=3 is also the **CUTLASS sm80 default** (`kStages = 3`), and the
Triton Ampere autotune table's winner at our tile size is 4 stages —
every doctrine source runs deeper than our 2. This probe measures the
doctrine's own point honestly.

## The wall this must move

| point | TF @4096³ | reading |
|---|---:|---|
| E7b no-fill KLOOP | 46.2 | smem-fed mma ceiling (ldmatrix kept) |
| E1f@4CTAs (fills→regs) | 42.5 | ≈ cuBLAS (42); no smem round-trip |
| **ship E4c** | **35.5** | fill+smem round-trip = **10.7 TF** |
| E8a warp-spec (fixed) | 23.8 | dead: barrier-serialized + 320T → 3 CTAs |

**Geometry is exhausted.** At 8 warps (mw·nw=8, 4 CTAs/SM) the tile area
is fixed at 128×128; any non-square aspect *raises* fill bytes/FLOP
(1/tile_M + 1/tile_N), and any bigger tile needs more warps → ≤3 CTAs →
the measured E-series loss. The square (2,4) point is the minimum-smem
geometry. So the only untested lever is pipeline **depth**.

## P1 design

**Config:** (2,4)@256T warp_mh=4, K16, f16acc, **stages=3**, 24KB/CTA.

**Generator change (tensor.rs):**
1. Relax `debug_assert!(stages.is_power_of_two())` to allow 3.
2. At the KLOOP stage sites (currently `and.b32 %r9, %r9, stages-1`
   then `add %r17, %r9, stages-1; and %r17, %r17, stages-1`), emit
   constant-modulo math for stages=3:
   - `%r9 = strip % 3` (strip = kstep/16).
   - `%r17 = (%r9 + 2) % 3` (the fill ring, one conditional subtract:
     `add %r17, %r9, 2; setp.ge %p, %r17, 3; @%p sub %r17, %r17, 3`).
   Power-of-2 stages keep the existing `& (stages-1)` byte-identically.
3. The E5a tail-prefetch site (881) also uses `& (stages-1)` but is only
   emitted under `b_lookahead` (default off) — leave it asserted
   power-of-2 (b_lookahead + stages=3 is not this probe).
4. The smem budget: `shared_bytes = (mw·mhr·512 + nw·gr·256)·stages` →
   3 × 8192 = 24576; the driver must launch `MW_SMEM=24576`.

**Prologue/peel unchanged**: the prologue fills stage 0; the KLOOP
prefetch distance `16·kps·(stages-1)` becomes 32 (2 stages ahead) —
already generic in `stages-1`.

## Protocol (house rules)

1. Dump PTX → `ptxas -arch=sm_86 -maxrregcount=64 -v` (expect ≤64 regs,
   0 spills; BUGS.md: the 128 cap pessimizes).
2. Correctness: `MW_SMEM=24576`, `BRIEV_GEMM_F16ACC=1`; 2048³/4096³/8192³
   ≤1e-2; k16 exact; error signatures must match the recorded
   (1.546e-3 / 5.208e-3 / 9.115e-3 / 0.0).
3. Interleaved A/B ×4 same-window vs ship (ship `MW_SMEM=16384`, probe
   `MW_SMEM=24576`), `ptx_gemm_bench`. **Win bar ≥40 TF @4096³**;
   regression check 2048³/8192³.
4. Commit per verdict:
   - **win** → `ptx_tensor_stages` knob + dispatch + `cargo test --lib`
     + E2E byte-identical ship path (stages=2 default).
   - **loss** → ledger entry + knob kept default-off as instrument
     (Rule 20: negatives stand).

## Expected outcome (honest)

Weakly negative prior: the fill-gap microbench measured 3-stage **t =
38.0 vs 2-stage p = 39.1** (sync-fill era, 1 CTA/SM), and 2 stages
already hide the ~1µs fill latency behind ~12µs stages. **But** the
microbench was not the real kernel (cp.async, 4 CTAs, swizzled smem),
and the real kernel's 10.7 TF fill cost is fill *issue + smem write*,
not latency. A deeper ring gives the scheduler two full stages of slack
to absorb the fill-issue burst — genuinely unmeasured. One dump-test +
~40 generator lines is cheap for the last untested doctrine point.

## If P1 fails → the strategic tier

1. **The DRAM-real fill microbench** (the E1f doc's listed missing
   instrument): computed per-CTA global fill addresses, A/B interleave,
   rasterization order — isolates exactly where the 7 TF goes before any
   further scheduling work.
2. **`gpu_schedule` (L5)**: DAG-driven fusion — amortize the smem
   round-trip across multi-GEMM graphs. The only path to the 46.2 no-fill
   ceiling / reliably beating cuBLAS.
3. **Portfolio (independent):** L3 split-K (2048³ 31.5 → ~39), L4
   small-K K=64-128 decode shapes (a class cuBLAS doesn't tune).

## P1 VERDICT (2026-09-14): marginal win +1.0-1.8% — SHIPPED as the f16acc default

| shape | ship (s2) | s3 | delta |
|---|---:|---:|---:|
| 2048³ | 32.60 | 33.25 | **+1.8%** (wins 3/3) |
| 4096³ | 35.51 | 35.86 | **+1.0%** (wins 4/5) |
| 8192³ | 36.09 | 36.49 | **+1.1%** (wins 3/3) |

Interleaved same-window A/B (ptx_gemm_bench, ship 16KB vs s3 24KB).
Correctness identical: 128-K-sweep exact (k16/32/64/96/128 MSE=0),
2048³ 1.546e-3 / 4096³ 5.208e-3 / 8192³ 9.115e-3 — the recorded
signatures. 64 regs / 0 spills, 4 CTAs/SM kept (24KB × 4 = 96KB).

**Readings:**
- stages=3 is a REAL but small win — deeper overlap absorbs the
  fill-issue burst, exactly as hypothesized. But the win is +1%, NOT the
  +13% needed for the 40 TF parity bar. The 7 TF fill/smem round-trip
  gap is structural; depth does not close it.
- The prior (fill-gap microbench "t = 38.0 vs p = 39.1") was directionally
  WRONG at the real kernel — cp.async + 4 CTAs + swizzled smem flips it.
  Another case of "microbench ≠ kernel verdict" (the LTO lesson).

**Ship:** `ptx_tensor_stages` knob (0 = auto: f16acc → 3, f32 → 4; else
override, valid 2|3). The f16acc default is now stages=3; the f32 path
is byte-identical (stages=4). E2E gemm_2048 builds 256T / 24576B smem.
warp-spec forces stages=2 (E8a protocol). `cargo test --lib` 2205 green.

**Conclusion:** this is the last config-level lever. The 4096³ parity
bar is not reachable by pipeline depth, occupancy, or geometry within
this smem-fed architecture — the residual ~7 TF is the smem round-trip,
which is structural on Ampere (no TMA; ldmatrix is smem-only). The path
to 42+ TF is the strategic tier (gpu_schedule, multi-GEMM fusion) plus
the portfolio (L3/L4) — the per-kernel sprint is over.

## Undo

stages stays a generator argument defaulting to 2; the modulo math is
emitted only for stages==3, power-of-2 stages keep `& (stages-1)`
byte-identically. The dump test is additive. If the probe loses, the
knob is kept default-off as an instrument (Rule 20).
