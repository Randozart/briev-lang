# E6: Ampere doctrine probes — stages depth and ksteps-per-stage

**Date:** 2026-09-13
**Baseline:** E4c ship config — (2,4)@256T warp_mh=4 stages=2 K16,
35.5 TF @4096³ (84.5% of the 42-TF cuBLAS anchor), 36.3 @8192³ (86%).
E5a/b/d (lookahead, bank de-phase, (2,8)@512T) all rejected — the config
space *within* stages=2/K16 was swept. This plan moves the two axes the
E-series never touched: **pipeline depth** and **k-depth per stage**.

## Research summary (2026-09-13, external)

1. **Hardware truth (GA106, Wikipedia spec table).** RTX 3060 nameplate
   tensor-f16 = 51.2 TF @ 1777 MHz boost, 170 W. cuBLAS's 42 TF implies
   ~1458 MHz sustained — the anchor is a throttled clock, not nameplate.
   E1's 52.8 TF pure-mma microbench was a short-burst over-boost. All E6
   comparisons are same-window; ratio claims stay clock-honest.
2. **CUTLASS Sm80 default** (`include/cutlass/gemm/device/default_gemm_configuration.h`):
   Threadblock **128×256×64**, Warp 64×64, **kStages = 3**, 8 warps.
3. **Triton Ampere fp16 autotune table** (tutorial 03): every CUDA config
   runs **BLOCK_K 32–64 with 3–5 stages**; the winner at our tile size is
   **128×128×32K @ 4 stages, 4 warps**; explicit pattern — smaller tiles
   get MORE stages (64×32 → 5 stages, 2 warps).
4. **Our ship config sits in the opposite corner:** 128×128 tile, K16,
   stages=2, 8 warps, 16 KB smem/CTA = 16% of the SM's 100 KB budget,
   ONE async group in flight, full-drain `wait_group 0` every kstep.
5. **Prior negatives don't cover this:** the stages-4 "wash" was the
   (4,4)@512T-era geometry; E5a tested register lookahead (not smem
   depth); E5d tested MORE warps. Stages 3–4 at the E4c geometry and
   K-deeper fills are genuinely untested.

## Probe ladder

| Probe | Config | smem/CTA → occupancy | Change |
|---|---|---|---|
| **P1** | stages=3 @ (2,4) K16 | 24 KB → **4 CTAs/SM** (96 KB) | config value only |
| **P2** | stages=4 @ (2,4) K16 | 32 KB → 3 CTAs/SM | config value only |
| **P3** | ksteps/stage=2 (K32 fill, one wait+bar per two 16-ksteps) | 32 KB → 3 CTAs at s2 | generator: fill/bar cadence |
| **P4** | kps=2 × stages=3 | 48 KB → 2 CTAs | combo |

**Why kps=2 matters (the E1d connection):** the mix-microbench ladder put
the fill *rhythm* (wait+barrier cadence) at ~10 TF. Halving the number of
wait+bar events per FLOP attacks that directly, with UNCHANGED register
pressure — unlike the E5a register lookahead which lost its CTA slot.

## Protocol (per probe, unchanged from house rules)

1. Dump PTX → `ptxas -arch=sm_86 -maxrregcount=64 -v` (expect ≤64 regs,
   0 spills; BUGS.md: the 128 cap pessimizes, 64 is the f16acc budget).
2. Correctness gate: `MW_SMEM=<per-config>`, `BRIEV_GEMM_F16ACC=1`,
   2048³/4096³/8192³ ≤1e-2, k16 exact. Error signatures must match the
   recorded full-K ones (1.5e-3 / 5.2e-3 / 9.1e-3 / 0.0).
3. Interleaved A/B ×4 same-window vs ship. Win bar ≥37 TF @4096³;
   regression check 2048³/8192³.
4. Commit per verdict: win → config knob + dispatch + `cargo test --lib`
   + E2E byte-identical ship path + plan-doc update; loss → ledger entry
   + knob kept default-off as instrument (Rule 20: negatives stand).

## P3 design (ksteps/stage = 2)

- Eligibility gate `k % (16·kps) == 0`, else kps=1 — no partial-stage
  fills, ever. k16 stays kps=1.
- KLOOP stays 16-strided. Fill + `cp.async.wait_group` + `membar` +
  `bar.sync` fire only when `(kstep/16) % kps == 0`. The compute phase's
  smem base = stage_base + sub·(asmem_buf), sub = (kstep/16) % kps.
- Per-stage buffers are kps× larger: stage bytes = kps·(asmem_buf +
  bsmem_buf)·stages total (P3 s2: 32 KB, P4 s3: 48 KB).
- Prefetch distance kstep + 16·kps·(stages-1); fill rung recomputed
  (per_a = 8 at (2,4)kps2 → 2× 16B .cg — the rung ladder already covers
  it). Promo unchanged (full-K, KEND store-only).
- Registers: +1 sub index (scratch reuse) — expect ≤72, still 4 CTAs at
  s2/32 KB... 32 KB × 4 = 128 KB > 100 KB → P3 at s2 is 3 CTAs (96 KB).

## Risks / caveats

- P1 co-residency at 96 KB/SM is unverified on this part — measure,
  don't theorize (per-CTA 24 KB < 48 KB static cap, no opt-in needed).
- P3's per-iteration fill-guard branch may disturb ptxas scheduling
  (E4b hoisting paradox) — the A/B decides; keep the E4b lane-term ALU.
- k16 / k%32!=0 shapes stay kps=1 (eligibility gate).
- Clock caveat recorded with every result table (finding 1).

## Undo

Every probe rides config knobs defaulting to the ship config; the E2E
ship path must diff byte-identical. Knobs: `ptx_tensor_stages`,
`ptx_tensor_ksteps_per_stage` (added only if their probe wins; dump-test
artifacts otherwise).

## E6 VERDICT (2026-09-13): the doctrine loses — occupancy dominates

| Probe | Config | 2048³ | 4096³ | 8192³ | Verdict |
|---|---|---:|---:|---:|---|
| P1 | stages=3 | — | — | — | **structurally invalid** (see below) |
| P2 | stages=4 (32KB, 3 CTA) | −4% | −2% | −1% | REJECTED |
| P3 | kps=2·s2 (32KB, 3 CTA) | −4% | −3% | −3% | REJECTED |
| P4 | kps=2·s4 (64KB, 1 CTA) | — | −18% | — | REJECTED |

All interleaved A/B ×2-4 same-window; correctness identical signatures
(1.546e-3 / 5.208e-3 / 9.115e-3) after the strip-loop fix.

**P1/P4 stage-count discovery:** stage indexing wraps with
`& (stages-1)` — non-power-of-2 stages silently alias and corrupt
(a stages=3 dump measured 2.4e-2, 5× over the gate, before the
`debug_assert!(stages.is_power_of_two())` went in). The literal
CUTLASS-default point (3 stages) is structurally unreachable; true-
modulo stage math would buy it if ever needed.

**P3 fill-strip discovery:** both fill emitters bake the 16-k strip
into their D-decomposition (A: `D>>5` = M-row; B: `D&1023` k-mask) —
a kps=2 stage fill overruns M and wraps the B swizzle. Fix: the fills
loop per-strip (`for strip in 0..kps`) with `dst_off`/strip-adjusted
source offsets; kps=1 emits byte-identical code.

**The lesson.** Every deeper-pipeline axis (stages, K-depth, register
lookahead) pays a CTA slot on this 28-SM/100KB part, and the lost fill
streams cost more than the saved latency/rhythm. The E4c point —
(2,4)@256T warp_mh=4, stages=2, K16, 16KB, 4 CTAs/SM — has now survived
challenges from five directions (tiles ×4, stages ×2, k-depth ×2,
register lookahead, bank layout). The external doctrine's deeper
pipelines are tuned for parts with ≥164KB smem (A100) where fat 1-2-CTA
kernels win; GA106's budget inverts the tradeoff. **35.5 TF @4096³
(84.5% of anchor) and 36.3 @8192³ (86%) stand as the measured optimum
for this kernel architecture on this part.**

Ship path byte-identical (E2E-diffed post-refactor); knobs
`ptx_tensor_ksteps_per_stage` (1) kept default-off as instruments.
