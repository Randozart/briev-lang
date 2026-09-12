# Plan: PTX f16acc warp-tile double-pump — raising the mma-issue ceiling

**2026-09-11.** Strategy-refocus plan after the shape-curve and
decomposition findings (ledger 2026-09-08 plan, 2026-09-11 entries).

## 1. Why this, why now — the wall has moved

The fill-pipeline campaign (stages, coalescing, pipelined B, register
cap) took the PTX tier from ~9 to 19.5 TFLOP/s sustained @4096³. The
decomposition shows that road is nearly exhausted:

| measurement @4096³ (2,4) f32 config | value |
|--------------------------------------|-------|
| fills stripped (compute-only ceiling)| 6.06 ms = **22.7 TF** |
| fills only | 4.39 ms |
| full kernel | 8.25 ms → now 7.0 ms (f16acc 19.5 TF) |

Shipped 19.5 TF is 86% of the 22.7 TF compute-only ceiling. Further
fill work buys ≤16%. The anchor (ggml-cuda 42 TFLOP/s = 41% of the
F16-acc tensor peak) cannot be reached from here — the wall moved to
the **mma issue path**: the B-fragment `ldmatrix` shared-read traffic
per FLOP saturates before the tensor cores.

## 2. The arithmetic — where the bytes go

Current f16acc kernel, warp tile = 32 rows × 64 cols (2 mh-blocks ×
8 col-groups), per kstep (16k), per warp:

- A fragments: 2 × `ldmatrix.x4` (one per mh) = 1024 B
- B fragments: **16 × `ldmatrix.x2.trans`** — 8 per g-loop **× 2
  (re-loaded for each mh!)** = 4096 B
- FLOP: 32 × 64 × 16 × 2 = 65536

**12.8 FLOP per shared-read byte.** The B fragments carry no mh
dependence — they are read twice by construction. The compute-only
ceiling (22.7 TF) is the shared path servicing that 2×.

## 3. The experiment — variant C (A-share across 4 mh-blocks)

Retile the warp to **64 rows × 32 cols** (4 mh-blocks × 4 col-groups):

- A fragments: 4 × `ldmatrix.x4` = 2048 B
- B fragments: 4 × `ldmatrix.x2.trans` = 1024 B
- FLOP: 64 × 32 × 16 × 2 = **65536 (unchanged)**
- Accumulators: 4mh × 4g × 2 f16x2 = **32 b32 — register-neutral**
- A regs: 4 live (sequential per mh), B regs: 4 (pipelined, unchanged)

**21.3 FLOP/byte (+67% intensity, −40% shared traffic).** If the
ceiling tracks shared traffic: 22.7 → ~35-38 TF ceiling; shipped
19.5 → target 28-33 TF.

CTA geometry: the reshape lands on the **(mw=8, nw=2)** CTA (256 rows
× 128 cols, 16 warps) — already exercised end-to-end ((8,2) measured
17.2 TF with the OLD warp tiling), so fill/barrier infrastructure is
proven; only the per-warp loop reindexing (mh 0..4, g 0..4, acc
`cb = 2*(mh*4+g)`) and the y-promotion addressing change.

Fallback rung (variant A, B-share): warp stays 32×64, B loaded once
for both mh — 64 accs, ~106 regs, 1 CTA. Only if variant C undershoots.

## 4. Baseline (ALL benchmarks, current commit `648d6902`, sustained 110W)

| benchmark | tier | result |
|-----------|------|--------|
| 2048³ PTX | f16acc (4,4)@512T | ~17 TF, 3.8e-3 |
| 4096³ PTX (anchor) | f16acc (4,4)@512T | **19.5 TF**, 1.2e-3 |
| 8192³ PTX | f16acc (4,4)@512T | 18.0 TF, 1.6e-3 |
| 2048³ PTX | f32 (2,4)@256T | 16.5 TF, 3.3e-4 |
| 4096³ PTX | f32 (2,4)@256T | 13.4 TF, 2.4e-4 |
| 8192³ PTX | f32 (2,4)@256T | 13.3 TF, 3.3e-4 |
| 4096³ coopmat | f16acc R=4 (shipped cfg) | 13.7 TF sync-ts, 4.4e-3 |
| 2048³ coopmat | f16acc R=4 | 27-29 TF, 2.1e-3 |
| 8192³ coopmat | f16acc R=4 | 24-34 TF, 8.3e-3 |
| 1024³ coopmat | f16acc R=4 | 18.2 TF, 7.3e-3 |
| 3072³/6144³ coopmat | f16acc R=4 | 3.2-12 TF (collapse band, parked) |
| anchor | ggml-cuda | 42.0 TF @4096³ |

CUDA is driver-wedged (needs owner sudo to clear) — the PTX re-baseline
after the change requires the reload first.

## 5. Protocol (per AGENTS.md)

1. Implement variant C in `tensor_gemm_ptx_smem_mw` behind the existing
   f16acc path — new match arm only, additive (rule 6).
2. Dump tests: (8,2)-geometry f16acc variants at 2048/4096/8192 + the
   K-sweep at the new warp tiling.
3. Correctness FIRST: S4 portfolio ≤1e-2 (f16acc contract), exact at
   single-chunk K. Kernel index math changed → on-device gate before
   any perf claim (the 2026-09-11 rule).
4. Perf: interleaved A/B vs the (4,4) baseline at power-steady state,
   ≥3 rounds, both shapes bands (2048³ + 4096³ + 8192³).
5. VERDICT entries for everything rejected; ledger row for everything
   kept. Baseline worktree (`../briev-compiler-baseline`) untouched for
   A/B regression detection.
6. If variant C wins: config default flip decision separately (the
   select_mw_nw f16acc thread-cap/geometry choice), never silently.

## 6. Documentation obligations

- Ledger row (this plan's §4 table + post-change table) in
  `docs/plans/2026-09-08-ptx-tier-execution.md`.
- The scheduling comment in `tensor_gemm_ptx_smem_mw` (the "why" of the
  warp tiling) rewritten — never deleted.
- BUGS.md if any correctness surprise appears.
- `select_mw_nw` comment update if the f16acc geometry choice changes.

## 7. Non-goals

- No coopmat changes (collapse band parked; its ledger row stands).
- No f32-acc path changes (the numerics reference tier).
- No config default flips inside this plan.
