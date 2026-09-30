# 5a — parallel decode attention (2026-09-30)

Target: composite decode attention **≤125 µs** at the bitnet geometry
(D=128, H=20, HKV=5, G=4, NKV=4096, CUDA lane), from the measured
**2478 µs**. Supersedes the "retire the fused family" framing, which the
measurement refuted — see `benchmarks/results/2026-09-30-5a-attention-decode.md`.

## Measured baseline

| path | p50 | notes |
|---|---|---|
| composite (one launch) | 2478 µs | **bimodal**: p10 = 200.8 µs |
| 3-kernel composition | 3443 µs | pv alone 3179 µs |
| ggml fattn | ~58 µs | reference |
| target | ≤125 µs | plan 2026-09-20-gpu-dialect §3.2 |

**Measurement caveat (critical):** the composite launch is bimodal
(p10 200.8 µs ↔ p50 2470 µs) — small-grid clock cap (see the GEMM
campaign's 225 MHz finding). The ramped latency is **~200 µs**, ~1.6× off
the target, not 20×. **A reliable sustained/clock-controlled decode rig
is Step 0** — the current per-step microbench (host push between
launches) is clock-unstable.

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

## Step 1 (starting now)

E0 (prerequisite): a **sustained decode measurement** — back-to-back
launches with no host gap (or a clock-warming duty cycle), reporting the
ramped latency and the SM clock during the run; re-baseline the composite
and the 3-kernel pv. Then E2 (confirm the per-warp j-iteration count) and
E1 (block-size, expected moot — grid is H-limited) before any lowering
change. Split-K over NKV (design option 1) is the likely fix once the
measurement is trustworthy.
