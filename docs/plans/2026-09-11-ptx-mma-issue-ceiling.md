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
