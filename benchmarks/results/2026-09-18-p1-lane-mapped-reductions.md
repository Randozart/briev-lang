# P1 — Lane-Mapped Inner Reductions: capability landed, occupancy gap found

**Date:** 2026-09-18
**Plan:** `docs/plans/2026-09-18-fused-f16-decode-node.md` (P1)
**Commit:** `66367d63` (probe `12a45506`, P0 in `0da4f0e9`)
**Device:** RTX 3060 (sm_86), CUDA lane

## Landed

The general PTX emitter now lowers the nested lane-reduction shape: a
serial outer foreach whose body holds a warp-divisible inner foreach
reduction (`acc = acc + <mul>`) maps the inner loop across lanes
(base `tid&31`, step 32) and combines each outer iteration with the
5-round `shfl.bfly` butterfly. Every lane ends with the full reduction,
so the remaining outer body reads exactly what the serial form produced
— sequential semantics preserved by construction. Serial fallback (and
CPU/interpreter/LLVM) unchanged. Gates: single self-referencing Add
body, const range, `% 32 == 0`, accumulator consumed after the loop.
2275 tests + M3 matrix green.

Regression surface: nil by construction — the pattern did not exist in
any pre-P1 .abv (the probe introduced it), so the new arm cannot fire
on existing kernels. Full-suite baseline A/B still scheduled before any
M4 integration.

## The honest perf finding

Probe (bitnet NKV=4096, H=20, D=128, one fused head-kernel):

| Form | Dispatch time | Correctness |
|---|---|---|
| serial-nested (pre-P1) | 71.5 ms | 1.81e-05 |
| lane-mapped (P1) | ~71 ms | 1.81e-05 |
| 3-kernel composition | 406–413 µs | exact |
| stock ggml fattn | 58–61 µs | — |

Lane-mapping fixed the compute mapping, not the wall: 20 blocks × 2
warps (40 warps on 28 SMs) hide no DRAM latency. Per inner iteration:
2 global loads ≈ 200+ cycles unhidden, ×16K iterations per lane. The
gap to ggml (~1200×) is not closable by ILP tiling (≤4–8×) — it needs
ggml-grade memory-path engineering: K-layout transform (their [D][j]
column-major cache makes lane-over-j reads coalesced), load pipelining,
and an occupancy design. Those are kernel-engineering decisions about
the composition's STATE LAYOUT and node structure, not expressiveness
gaps — the language can now SAY the fused form; making it fast is a
different campaign.

## Consequence for the plan

- **P2 gate unchanged, honesty sharpened**: a fused-node attempt today
  would land between ~9 ms (j-tiled ILP, predicted) and 71 ms — far
  above both the 3-kernel 406 µs and the 58 µs gate. BUILDING P2 NOW
  WOULD FAIL ITS OWN GATE. The gate stands; the fused node waits for
  the memory-path campaign (K layout in the composition + pipelined
  loads + occupancy), which is its own plan.
- P1 is complete as a CAPABILITY: the shape lowers correctly on both
  lanes (SPIR-V serially, PTX lane-mapped) with sequential semantics
  intact. The capability generalizes (any inner-dim reduction inside a
  serial loop: matvec tails, conv sums, online algorithms).
- P4/P5 remain gated exactly as written: no fattn.cu wiring without a
  P2 pass.

## CyberLlama campaign status (honest ledger)

The decode-attention track delivered: device-verified composition
correct on both lanes at all geometries (P0, 36/36), a decode-append
runtime path (push_ranges), the honest 7× negative on the 3-kernel
form, two compiler capability fixes (Cast walkers, lane-mapped
reductions) with tests, and a precise map of what a competitive fused
kernel needs. What it did NOT deliver: a fattn.cu integration — the
gates refused, as designed. The attention share of mellum decode is
<3% anyway; the campaign's own ledger said the win had to appear on
bitnet p4096, and the kernel-efficiency gap (same one documented vs
cuBLAS at 512³/4096³) is the standing blocker.
