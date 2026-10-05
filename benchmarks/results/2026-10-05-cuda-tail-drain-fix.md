# CUDA tensor tail-drain fix — gate evidence + perf A/B (2026-10-05)

Bug: `BUGS.md` "CUDA f16 tensor patterned nondeterminism at g1024"
(RESOLVED). Fix: `src/backend/ptx/tensor.rs` WAIT_DRAIN branch — from the
fill-guard iteration on, `cp.async.wait_group 0` instead of
`wait_group (stages−2)` (the deep-K sibling of the 2026-09-16 shallow-K
drain). Machine: RTX 3060 pair, CUDA lane `CUDA_VISIBLE_DEVICES=1` for
timing, default device for gates.

## Correctness (same-commit gate, fixture set `examples/gpu/.tmp_quad/`)

Harness: `benchmarks/declared_matmul_gate.sh <fixture> <M> <N> <K> f16`,
both lanes, modes 0 (ones) + 1 (patterned), default tol 5e-3 (no
`BRIEV_GEMM_F16ACC` needed — the fix removed the race above the floor).

| shape | result (CUDA / Vulkan) |
|---|---|
| g64, g128, g256 | EXACT + max_rel 0.000e+00 / same |
| g512 | EXACT + 1.636e-3 / same |
| **g1024** | EXACT + **3.748e-3 / 3.748e-3** |
| g2048 | EXACT + 1.303e-3 / same |
| g4096 | EXACT + 4.436e-3 / same |
| x_64x64x256, x_256x256x64 | EXACT + 0.000e+00 / same |
| y_64x64x128, y_64x64x256, y_64x128x256, y_128x128x256 | EXACT + 0.000e+00 / same |
| g192 | gate script asserts K power-of-two (unconditional) — pre-existing, script untouched |

g1024 CUDA stress: same binary ×12 → 3.748e-03 **every run (zero
variance)**; ones EXACT ×3. Pre-fix the same binary varied
3.748e-3..1.04e-2 with 0..60+ bad cells per run.

`cargo test --lib`: 2856 green (incl. new
`backend::ptx::tensor::r16_dump::mw_tail_drain_branch_keeps_last_stripe_wait`).

## Perf A/B (provisional — machine-drift dominated, min convention)

Protocol: `git worktree` at HEAD `beb313f2` (pre-fix compiler) vs
working tree; each builds `g256`/`g1024`/`g4096`; `/tmp/opencode/timing_ab.py`
splices batched timing into the generated runner (reset the i-guard,
`BRIEV_ITERS` launches inside the guard loop, wall time around
loop+final download); rounds alternate order (old-first/new-first);
CUDA GPU1, 210 MHz idle → per-launch boost (DVFS envelope ≈ ±5%,
observed 1.47→2.37 ms drift across rounds on g1024 in one window).

Raw `per_ms` (30-40 iters/run):

| round | g1024 old | g1024 new | g4096 old | g4096 new |
|---|---|---|---|---|
| 1 | 1.9431 | 1.9964 | 23.4796 (cold) | 18.4139 |
| 2 | 2.0880 | 1.9708 | 18.9632 | 18.8070 |
| 3 | 1.9635 | 2.0107 | 18.5859 | 18.9476 |
| 4 | 1.9662 | 1.9394 | 18.4398 | 18.6462 |
| 5 | 1.9691 | 2.0247 | 18.6516 | 18.5903 |
| 6 | 2.3370 | 1.9870 | 17.9153 | 18.9413 |
| 7 | 2.0001 | 2.0150 | — | — |
| 8 | 1.9595 | 1.9598 | — | — |

First window (separate g4096 session): old min 17.2193, new min
17.0420; g256 launch-overhead-dominated (0.10-0.24, no arm separation).

**Reading:** per-round deltas flip sign in both directions; no arm
separation beyond the DVFS envelope. Min-convention (ledger rule): g1024
1.9394 new vs 1.9431 old (−0.2%), g4096 17.04 new vs 17.22 old (−1%).
**No >1% regression established**; timings provisional (no clock pinning,
idle-boost ramp between runs).

## Artifacts

- Emitted PTX (pre/post fix): `BRIEV_DEBUG_PTX=<dir> brievc build ...`
- SASS dump kept at session: `/tmp/opencode/g1024_sass.txt`
- A/B rig: `/tmp/opencode/timing_ab.sh`, `/tmp/opencode/timing_ab.py`,
  worktree `/tmp/opencode/prewait`
