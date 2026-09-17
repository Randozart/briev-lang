# CyberLlama "Before" Ledger — stock fork baseline (pre-.abv)

**Date:** 2026-09-17
**Binary:** cyberllama `build/bin/llama-bench` @ `ab61c2a01 (1737)`, CUDA backend
**Stack:** driver **615.71.09** (nvidia-open-dkms, upgraded from 580.178.04 this
session), CUDA 13.4 toolkit, kernel 7.2.5-1-cachyos, single GPU via
`CUDA_VISIBLE_DEVICES=0` (RTX 3060 12GB, sm_86)
**Model:** mellum2-claude-Q2_K.gguf — mellum 12B.A2.5B Q2_K, 64 experts / 8 used,
ngl 99 (fully offloaded)
**Method:** llama-bench defaults, interleaved rounds as shown, warmup included.

## Results

| Test | t/s |
|---|---|
| tg32 @ p0 (decode floor, KV≈0) | **154.76 ± 2.92** |
| tg32 @ p1024 | 158.74 ± 2.85 |
| tg32 @ p4096 | 158.35 ± 3.79 |
| pp1024 | 2774.67 ± 7.07 |
| pp4096 | 2691.71 ± 7.48 |
| pp512 | 2766.23 ± 94.36 |

Reference dual-GPU smoke (both 3060s, -p 128 -n 32): pp128 1275.68 ± 85.23,
tg32 159.64 ± 1.55.

## Findings

1. **Decode is context-flat**: 154.8 → 158.4 t/s from p0 to p4096 (within
   ~noise, slightly upward — warm-cache effect). Attention is a negligible
   share of decode time on this MoE model at these lengths; expert/FFN
   bandwidth dominates. **Consequence for the .abv A/B: tg rows on mellum
   @ ≤4096 cannot show an attention-kernel win.** The A/B must scale context
   (16k/32k rows) or use a dense/attention-heavy model to make attention
   visible — to be added to the "after" matrix.
2. **Prefill flat vs length** (pp1024 ≈ pp4096 ≈ pp512 within ~3%): weight
   loads dominate prefill too at these shapes; Q2_K matmuls run the quant
   (mmq) path, not the f16 tensor path Briev's tier targets.
3. **Driver 580 → 615 delta on the stock binary**: pp128 1193 ± 87 →
   1276 ± 85 (+6.9%), tg32 155.8 ± 2.1 → 159.6 ± 1.6 (+2.5%). Modest but
   real; 615 is the new pinned reference. All future A/Bs run on 615.
4. **vitriol-server.service** auto-starts at boot and holds both GPUs
   (~22 GB); must be stopped for benchmarking
   (`sudo systemctl stop vitriol-server`), restarted after.

## Device-gate status after driver upgrade

- Vulkan/SPIR-V accel path: **PASS** (`pairs.abv` runner: gx=16×local 256,
  `i = 4096` correct, 0.003 ms/dispatch).
- CUDA accel path: N/A on standalone runners — they embed SPIR-V only
  (`pairs_runner.c` k0 blob = SPIR-V magic); the CUDA driver expects PTX
  text ("S2" emission not wired into standalone runners). Pre-existing
  structural gap, not a driver regression. `--backend gpu` brievc path
  panics on a defn-liveness error (`__stdout_flush` unreached) — separate
  pre-existing bug, parked in BUGS.md candidate.
- Coopmat-on-device verification: deferred to the M3 attention harness
  (first coopmat-exercising on-device run in this milestone); the old
  `gemm_h` scratch program is gone from the tree.

## "After" matrix (to run when .abv attention lands)

Same binary pair, same GPU/method, plus: tg32 @ p16384, tg32 @ p32768
(attention share grows linearly with KV length — the rows where an .abv
attention win must appear), and pp at 8192+ if the MMA variant lands.
The p0 row doubles as the sanity check: must stay ~155 t/s.
