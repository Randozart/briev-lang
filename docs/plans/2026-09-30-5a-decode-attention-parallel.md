# 5a — parallel decode attention (2026-09-30)

Target: composite decode attention **≤125 µs** at the bitnet geometry
(D=128, H=20, HKV=5, G=4, NKV=4096, CUDA lane), from the measured
**2478 µs**. Supersedes the "retire the fused family" framing, which the
measurement refuted — see `benchmarks/results/2026-09-30-5a-attention-decode.md`.

## Measured baseline

Reliable protocol = **≥1000 reps** (the low-rep p50 is a clock-ramp
transient — see `benchmarks/results/2026-09-30-5a-attention-decode.md`).

| path | ramped p50 | notes |
|---|---|---|
| composite (one launch) | **200.6 µs** | p10 194.9 / p90 207.1 (2000 reps) |
| ggml fattn | ~58 µs | reference |
| target | ≤125 µs | plan 2026-09-20-gpu-dialect §3.2 |

Gap: **~1.6× to target** (3.4× to ggml) — a tractable kernel-efficiency
problem, not a 20× defect. Structural cause: `grid = H` (no split-K) +
per-`j` pass overhead.

## Emitted kernel (composite `fattn`)

General-tier kernel, `src/backend/ptx/general.rs` deferred-region lowering
(`emit_deferred_region`, ~L3230): max pass + accumulate pass over `j`,
`z = q·k[j]` dot over `d`.

- desc: `block_threads = 1024` (32 warps), static `SHARED:16640`
  (`redm[128]`, `redl[128]`, `smacc[16384]`), `REG:51`.
- **Parallelism today:** `j` split across WARPS (`r_lo = warp·jslice`,
  jslice = NKV/warps = 128), `d` split across LANES (`strips = D/32 = 4`).
- **Grid = H CTAs only (20–32)** on 28 SMs → ~1 wave; no split over NKV.
- Per-`j` overhead is heavy: dot strips + butterfly (11 `SHFL.BFLY`, some
  via out-of-line `CALL.REL.NOINC` to `$__cuda_sm70_shflsync_bfly`) + `Exp#`
  + a full second sweep (two passes: max, then accumulate), + the div
  slowpath calls.

The kernel is ~828 static instructions; the cost is the **number of
sequential j-iterations per warp × 2 passes** at a near-empty grid.

## Design options (ordered by expected value)

1. **Split-K over NKV.** Grid becomes `H × S` (S CTAs per head, each
   reducing an NKV/S slice into partial `m`, `l`, `acc`); a small combine
   (second kernel or atomics on `smacc`) merges. With S=4–8 the grid
   reaches 80–160 CTAs → fills the SMs. This is the single largest lever
   (grid underfill is the root cause). Needs either an atomic-combine or a
   compact second kernel — a lowering addition in the general tier, not a
   benchmark-specific matcher (Rule 23).
2. **Reduce per-j cost.** Inline the butterfly (drop the `CALL`), fuse the
   two `j` passes where the algebra permits (the deferred-region doc
   already exposes `m`/`l` running state), and hoist `q[d]` (loop-invariant
   in j) into registers once per pass.
3. **Multi-head / more CTAs per SM.** With 1024-thread CTAs at REG=51
   only ~1 CTA/SM fits; smaller CTAs (256–512 T) could co-reside, but the
   grid is H-limited so this only helps after split-K.

## Pre-B experiments (Rule 20 — before building)

- **E1**: measure the kernel with `block_threads` varied (256/512/1024)
  via the general-tier block-size path (or a temporary knob) to separate
  "warps-per-CTA" from "grid-size" effects.
- **E2**: count the dynamic `j` iterations that actually execute per warp
  (predicate/guard inspection) — confirm the loop runs NKV/warps times and
  is not accidentally full-NKV serial.
- **E3**: a hand-timed variant of the emitted PTX with the two passes
  fused for the max/accumulate (small hand patch on the dumped PTX) to
  size lever 2 before any generator change.

## Gates

- Correctness: `bash benchmarks/m3_attention_harness.sh 4096` (both lanes,
  a_err < 1e-3) and the composite gate (`softmax_gate.sh`).
- Latency: `bash benchmarks/composite_decode_microbench.sh 128 20 5 4096`
  (p50 across reps; clock-aware — the gemm-h_gate showed small-grid
  clock caps, so cross-check `nvidia-smi` SM clock during the run).
- `cargo test --lib` green; Praetor no new diagnostics; docs same commit.
- Delete `fused_attention_*` only when the composite meets decode parity
  (Rule 24 retirement gate) — not before.

## Step 1

E0 DONE — reliable protocol is high-REPS sustained (≥1000); ramped
composite baseline = 200.6 µs.

**Split-K sweep DONE 2026-10-01** (`0a1a2b38`,
`benchmarks/composite_decode_split_sweep.sh`): the declared `split<S>`
deferred modifier (the EXISTING partial+combine machinery — no new
lowering needed) measured at 1200 reps. split=4 (the cost model's own
factor) wins: p50 220.8 → 202.5 µs (−8%), p90 339 → 210 µs (the
combine stabilizes the clock-cap tail). S>4 regresses. Conclusion: the
grid-underfill lever is worth ~8% — the residual gap to 125 µs is the
PER-J COST (two passes × 128 j-iters/warp + butterflies + Exp# + div
slowpath). **Lever 2 first half DONE 2026-10-01** (`a06ef304`): the E3 dump
re-sized the lever — the butterflies were ALREADY inline (post-ptxas
SASS artifacts) and the address math already strength-reduced BY ptxas
(the E3 PTX-level premise refuted at the SASS level); what survives is
the Q RE-LOAD: 8 in-loop LDGs (4 strips x 2 passes) at a j-invariant
address ptxas cannot hoist (state-pointer aliasing). The deferred
emission now lifts j-invariant array reads into immutable per-strip
lets (registers). Decode composite: no-split p50 220.8 -> 191.5 us
(-13%, the shipped default; from the 200.6 baseline — 1.53x to target).
Two gate-caught defects fixed (strip-collapse; all-strips rewrite
pairs). Full data in the results file.

**Pass-split probe DONE 2026-10-01** (`9f97c72b`,
`ptx_deferred_skip_pass`): the dot is computed TWICE (pass A for max,
pass B for the accumulate); the online form computes it ONCE —
projected ~125-140 µs from the measured 255/125/77 split. The rescale
counter-cost is negligible (~ln(4096) updates; the acc is
smem-resident). The composite's dlen≤32 online branch exists — the
lever is the deferred emitter learning the fused form for
smem-resident acc.

Next: **the fused j loop** — one sweep (dot, running max, rescale
acc/l on update) replacing the two passes, gated by the m3 harness
(both lanes) + the softmax fixtures before timing. Then the div
slowpath and shape-gating (prefill -> composite / decode -> the
measured-best config). Correctness gates: softmax_gate s8 + knob PASS
both lanes at pre-hoist residuals.
