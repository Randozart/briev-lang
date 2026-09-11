# PTX tier: mma-issue ceiling investigation (research round 2026-09-11 evening)

## Why

The double-pump rejection (previous section) refuted the shared-BW model.
The shipped kernel (21.0 TF @4096³) sits at 86% of our own compute-only
ceiling (22.7 TF) but at 41% of the dense f16-acc silicon peak (~51.6 TF)
and 50% of the cuBLAS anchor (42 TF). A research round identified what the
anchor actually is and what it does differently.

## Research findings

**The anchor is cuBLAS.** The vendored llama.cpp routes F16×F16 mul_mat to
`use_batched_cublas_f16` → `cublasGemmEx` (ggml-cuda.cu:2450, compute type
f16 — only the f16-acc rate explains 42 of a 51.6 dense peak). "Beating
the anchor" = matching the CUTLASS-style sm80 mainloop, which is public:

CUTLASS `efficient_gemm.md` + Bruce-Lee-LY/cuda_hgemm (≥95% of cuBLAS on
sm_86, MIT) recipe:

1. Multi-stage cp.async pipeline: 3–4 stages, k-depth 32 (ours: 2 × 16)
2. Ps2r register double-buffer: next kstep fragments load under this
   kstep's mma phase (ours: 2-group ld-ahead)
3. ONE epilogue: full-K register accumulation (ours: y RMV every 512-k
   = 8 drains per K-loop)
4. Warp tile 64×64 = 32 independent mma chains (ours: 32×64 = 16)
5. cp.async 16B transfers, swizzled smem, L2-aware CTA rasterization

**The decisive local fact**: fills are already subordinated (shipped 21.0
vs compute-only ceiling 22.7; consistent with the S4/chunk VERDICTs). The
wall is inside the mma phase. Either the ISA needs ~32 independent chains
(our 16-chain schedule issue-stalls), or the 22.7 ceiling was our
schedule's ceiling, not the hardware's. A pure-issue microbenchmark on
this box answers that — stronger than paper numbers for an 110W-capped
3060.

## Baseline table (commit 54329eaf, sustained, batched)

| measurement | value |
|---|---|
| PTX f16acc (4,4)@512T 4096³ | **21.0 TF** (1.22e-3) |
| PTX f16acc 2048³ / 8192³ | 17.6 / 20.1 TF |
| PTX f32 (4,2)@256T 4096³ | 19.4 TF (2.44e-4) |
| coopmat f16acc 2048³ / 4096³ / 8192³ | 27.7 / 7.7–9.5 / 21.2 TF |
| compute-only ceiling (mma_ceiling_bench, ledger era) | 22.7 TF |
| **anchor: cuBLAS f16 f16acc 4096³** | **42.0 TF** |
| dense f16-acc silicon peak (boost clocks) | ~51.6 TF |

## Experiment ladder

| # | experiment | expected | decision point |
|---|-----------|----------|----------------|
| E1 | pure-mma issue microbench: chains ∈ {4,8,16,32,64}, f16acc + f32acc, no smem | pins the ISA ceiling | if 16 chains cap ≈23 TF → E3/E4 must deliver 32 chains; if ≈45 TF → the schedule overhead is the wall |
| E2 | full-K accumulation (drop the 8 y-RMV drains; contract: cuBLAS/coopmat measure 4–8e-3 at K≤8192 < 1e-2 gate) | +2–5% | cheap keeper |
| E3 | k32 × 3-stage pipeline (CUTLASS sm86 config, 72KB, 2 CTAs) | +5–15% | main structural rung |
| E4 | Ps2r phase-level fragment double-buffer + hoisted addressing (the 2026-09-10 hoist lost 11% — revisit only at k32) | +5–10% | after E3 |
| E5 | cp.async 16B fills + L2 rasterization swizzle | +2–8% | polish |

Protocol per rung: dump → ptxas reg check → on-device correctness ≤1e-2 →
sustained interleaved A/B (ptx_gemm_bench, 3 reps) → 2048/4096/8192 sweep →
VERDICT in the ledger (docs/plans/2026-09-08-ptx-tier-execution.md).

## E1 design

PTX kernel, one .b64 param (state pointer, DCE-guard store target), no
smem: N independent mma chains per warp (N acc register pairs), operands
in registers from lane/tid-derived constants, R iterations of an N-chain
mma block. Launch: 512-thread CTAs × 28 (1 CTA/SM, matching the GEMM's
occupancy). Total FLOP = R·N·4096·warps; ptx_gemm_bench reports TF from
M,N,K = the cube root — keep totals ≈ 100–150 GFLOP so runs are ~5–8 ms.

Sweeps: chains {4, 8, 16, 32, 64} × {f16acc, f32acc}. The f16acc/f32acc
ratio cross-checks the GA10x 2× dense rate claim on this driver.

## Documentation plan

- This plan: hypotheses, baselines, ladder.
- Ledger (2026-09-08 execution doc): E1 VERDICT + rung results as they land.
- No architecture doc changes until a rung lands (E3 would touch
  abv-gpu-doctrine's tier notes only if it changes the default config).

## E1 VERDICT (2026-09-11 night): schedule overhead is the wall, not chains

Pure-issue microbench (dump_mma_microbench, 512T × 28 CTAs, sustained):

| chains | regs | TFLOP/s |
|--------|------|---------|
| 4 | 20 | 52.4 |
| 8 | 28 | 52.6 |
| 16 | 42 | 52.8 |
| 32 | 74 | 53.4 |
| 64 | 138 | launch-impossible at 512T (138×512 > 64K regfile) |

**16 chains saturate the tensor cores at ~53 TF — the full dense peak.**
The plan's decision point resolves to the second branch: the 22.7 TF
"compute-only ceiling" was our KLOOP's own ceiling (ldmatrix + per-ld
address math + branch competing for issue), not the hardware's. The
52.8 → 22.7 → 21.0 decomposition puts the entire 2.3× inside the mma
phase's support stream.

Consequence for E3/E4: do NOT design for 32 chains. Design for a
skinny KLOOP — loop-carried pointer arithmetic (kstep stride adds
instead of per-fragment address recomputation), k32 stages, phase-level
Ps2r — so the issue budget goes to mma, not math. E1b/E1c (ldmatrix-mix
and ALU-load sensitivity microbenches) quantify the decomposition next.

## E1b/d/f ladder + rasterization VERDICT (2026-09-11 late night)

The mix microbenches (dump_mma_mix_microbench, variants b/c/d/f) decompose
the 52.8 → 21.0 gap:

| rung | adds | TFLOP/s |
|------|------|---------|
| E1 | pure mma issue | 52.8 |
| E1b | + ldmatrix(x4+8×x2) + 150 ALU ops, fixed addrs | **53.5 (free)** |
| E1d | + cp.async fills + wait_group 1 + bar.sync, L2-resident data | 42.5 |
| E1f | E1d with STREAMING (DRAM-latency) fill reads | 32.9 |
| real kernel | | 21.0 |
| cuBLAS anchor | | 42.0 |

Readings: support ALU is FREE (schedulers absorb it; the 2026-09-10
"hoisting dropped 11%" note stays unexplained but the issue-budget model
is dead). The fill rhythm (wait+barrier) costs ~10 TF; DRAM-latency
streaming costs ~10 more. E1d — fills+barrier with L2-resident data — is
exactly cuBLAS.

**E5-rasterization VERDICT: REJECTED (−7%).** Group-swizzled the CTA
decode (8m×4n, divisor-safe): 19.6 vs 21.0 across three tight reps. The
L2-reuse model does not bind for this kernel at this shape — DRAM
bandwidth is far from saturated (~56GB/s of ~360), so packing panel
sharers closer in time buys nothing and the theory is refuted. Reverted;
the negative stands in the ledger per rule 20.

Open for next session (the 32.9 → 21.0 delta, three suspects, all cheap
probes): wait_group 0 in the real kernel vs 1 in E1f; 2-CTA/SM
occupancy contention (E1f ran 1/SM); the every-32-iteration y-RMV
promotion. The s4 paradox (deeper prefetch slower) remains open and now
bears on the wait_group question directly.

## Probe round 2 (2026-09-11 late night): occupancy heals fills; ldmatrix lookahead is the wall

Corrected-accounting occupancy sweep (E1f streaming fills):

| occupancy | E1b no-ALU | E1c +150 ALU | E1f +streaming fills |
|-----------|-----------|--------------|----------------------|
| 1 CTA/SM | 52.6 | 53.8 | 32.7 |
| 2 CTA/SM | 54.7 | 54.0 | 41.0 |
| 4 CTA/SM | 55.2 | 54.9 | **42.5** |

- **wait_group 0 vs 1: no difference** (E1g = E1f within noise). The
  full-drain theory is dead; the s4 paradox is NOT wait_group slack.
- **Support ALU is free at every occupancy** — the issue-slot model is
  dead for good.
- **Occupancy heals the fill-latency stall**: 4 independent CTA fill
  streams reach cuBLAS level even with DRAM-latency fills. The real
  kernel runs 2/SM (64 regs × 512T).

**The no-fill KLOOP re-measured TODAY: 29.9 TF** (ledger's 22.7 was
pre-pipeline-fix). vs E1b's 53.5 with identical ldmatrix+mma counts and
fixed addresses: the delta is the **ldmatrix lookahead depth**. The B
schedule keeps 2 groups live with a ld-ahead of g+2 — one or two mma
(~8–16 clk) hide a ~30 clk ldmatrix → each of the 10 lds per kstep
stalls ~15–20 clk ⇒ ~89–130 clk/kstep measured. The cure (all 8 B + 2 A
fragments live across the phase boundary, or k32 with cross-kstep
prefetch) costs +18 registers — over the 2-CTA/SM budget at 512T.

Next session, in order:
1. Re-emit (2,4)@256T stages-2 f16acc and re-sweep the config — the
   12.2/13.6 sweep numbers predate the pipeline-pair fix; at 64 regs
   × 256T the (2,4) geometry runs **4 CTAs/SM** (the E1f@4/SM effect)
   AND frees the registers for deeper fragment lookahead.
2. If occupancy wins, the new geometry becomes the select default.
3. Otherwise: ld-ahead restructuring against the register wall
   (triple-lookahead at 512T, or 256T with full-phase preload).
4. The y-RMV promo (~7% of memory instructions) is a cheap final rung.

## Probe round 3 addendum (same night): occupancy loses on the real kernel

(2,4)@256T stages-2 f16acc (the 4-CTA/SM candidate), three tight reps:
**15.58 TF vs 21.0 at (4,4)@512T.** The E1f occupancy effect does not
transfer to the real kernel — the config sweep's (4,4) verdict stands
post-fix. The occupancy lever is dead.

The register-wall gamble is nonetheless rational: E1b measured 52.6 at
1 CTA/SM — occupancy only pays when there are stalls to hide, and
full-phase fragment preload removes the stalls themselves. E4's shape:
1 CTA/SM × 512T × ~90 regs, all 8 B + 2 A fragments live across the
phase boundary, addresses computed as loop-carried increments. Target:
the no-fill KLOOP ceiling 29.9 → 45+, shipped 21.0 → 30+. E4 is next
session's opening rung.

## E4a VERDICT (2026-09-11 night): cluster preload +1% — kept; the wall moves again

E4a shipped: all 8 B + both A fragments load back-to-back before the mma
phase (the g+2 alternate-pair pipeline and its historical bug site are
gone — simpler schedule, same 64 regs after ptxas, correctness identical
at 1.789e-03). Result: 21.43 vs 21.23 TF — a keeper, not a rung.

The ld-stall model over-promised: ptxas was evidently hiding more of the
g+2 lookahead than the model credited. The decisive unexplained number
is now **E1f vs the real kernel at identical 2-CTA/SM occupancy: 41.0 vs
21.0**. Everything the ladder tested (issue, chains, ALU, ld order,
wait slack, occupancy) is excluded; what E1f lacks vs the real kernel:
the every-512-k y-RMV promotion, the swizzled-smem write/read bank
interleaving with cycling kstep addresses, and the 512×2-thread barrier.
Next session: strip the promo from a dump variant (the only single-
feature probe left that can carry ~20 TF), then the smem-port question.

## THE WALL BROKEN (2026-09-11, night): full-K + store-only epilogue — 29.3 TF (4096³)

The promo-strip probe (sed the predicated branch unconditional) measured
45.4 TF — the every-512-k y-RMV promotion was costing ~half the kernel.
Root causes, in order of discovery:

1. **Full-K accumulation**: the 138 pipeline-draining RMV rounds are gone;
   the f16x2 chains accumulate the whole K loop. Contract verified on
   device: 5.2e-3 @K=4096, 8.2e-3 @K=8192 — the same K-budget curve the
   coopmat tier documents (boundary ≈ K=12288, beyond that the f32-acc
   tier serves).
2. **Store-only final epilogue**: with full-K, the final pass owns every
   y element — the RMV's cold-miss global READS (17 TF of end-of-kernel
   DRAM interference: 45.4 stripped vs 28.0 with one RMV round) and the
   y-zeroing pass are both gone. The final store is a straight-line tail
   AFTER KEND — the in-loop predicated promo structure itself measured
   28 vs 45 with an identical body made unconditional, so the loop tail
   stays clean.

Production blob path (stale-binary artifact chased down: ALWAYS rebuild
the release binary before trusting a blob bench):

| shape | before (session start) | now | rel |
|-------|------------------------|-----|-----|
| 2048³ | 17.6 | **25.5** | 1.30e-3 |
| 4096³ | 21.0 | **29.3** | 4.44e-3 |
| 8192³ | 20.1 | **30.2** | 8.22e-3 |

Tier picture flipped: the PTX f16acc tier now beats coopmat at 4096³
(29.3 vs ~9.5) AND 8192³ (30.2 vs 21.2), nearly ties at 2048³ (25.5 vs
27.7). Anchor: 29.3/42 = 70%.

Open: the stripped-probe 45.4 ran at 40 ptxas regs (3 CTAs/SM — the
dead promo body shrank the allocation) vs our 64 (2 CTAs/SM). Whether
a ≤42-reg schedule of the LIVE kernel exists (3 CTAs/SM) is the next
rung; the Coopmat 2048³ lead (27.7 vs 25.5) may also fall to it.
