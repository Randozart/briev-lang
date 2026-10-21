# Checkpoint — effect-state-carry experiment

**Date:** 2026-10-21
**Branch:** `experiment/effect-state-carry`
**Commit:** `3c1611d2`
**Tag:** `checkpoint-effect-state-carry-2026-10-21`
**Status:** SUCCESSFUL EXPERIMENT — intentionally **not** promoted to default.

## What this checkpoint contains

The `experiment/effect-state-carry` branch adds an env-gated loop-carried SSA
carry for plain effect fields on direct-Float loops.

- `BRIEV_DIAG_EFFECT_CARRY=1|latch|exit`
- `latch`: identity store of the carried effect values every iteration.
- `exit`: identity store only after the loop (faster; recommended for A/B).
- Only direct-Float loops carry (the existing temp-mediated gate).
- No `%State` escape, no second stdout buffer, no ABI split.
- `main` is unchanged; default compiler behavior is unchanged.

The code is deliberately isolated because it trades a small float/telemetry
regression for a large `sweep_sparse` win. It is the fallback point if the
canonical Local/effect architecture fails.

## Linked-LTO results (BOUND=50M unless noted)

| Benchmark | carry/latch | carry/exit | current escaped selector |
|---|---:|---:|---:|
| float_math | 0.98x | 0.97–0.99x | 0.89–0.96x |
| float_math_nonzero | — | 0.97x | 1.21x |
| telemetry_stream | 0.99x | 1.01–1.03x | 0.96–0.98x |
| pid_control | 0.99x | 1.00–1.01x | 0.99–1.01x |
| sweep_sparse | 0.97x | 0.98x | 1.31–1.33x |
| sweep_mid | — | 1.38x | 1.33–1.38x |
| queue_drain | 0.50x | 0.51–0.56x | 0.50–0.53x |
| interval_step | 0.69x | 0.75x | 0.71–0.75x |
| matrix_pipeline | 0.96x | 0.97x | 0.96–0.97x |
| kalman_filter_runtime | — | 0.86x | 1.00x |
| accumulator_flush | — | 0.95x | 0.90–0.95x |
| mixed fixture | 0.228s | — | 0.205s (escaped) |

Full suite: all correctness rows MATCH.

The controlling cause: the stateless loop carries no effect state and LLVM
spills the reduced accumulator; carrying the effect state produces the
loop-rotation/liveness shape that keeps it in `%xmm0`. It is a real
optimization lever, but the per-benchmark tradeoff is not the final policy.

## How to return here

```bash
git switch experiment/effect-state-carry
# or:
git switch -c experiment/effect-state-carry-return checkpoint-effect-state-carry-2026-10-21
```

A/B invocation:

```bash
BRIEV_DIAG_EFFECT=stateless BRIEV_DIAG_EFFECT_CARRY=exit \
  bash benchmarks/build_and_bench.sh --runtime
```

Tests at the checkpoint: `cargo test --lib` → 3269 passed; quick guard PASS.

## Why not default

- `float_math`, telemetry, accumulator, sweep_mid, and the mixed fixture are
  slightly slower than the current escaped selector.
- `sweep_sparse`, float_math_nonzero, and kalman are much faster.
- The result is a genuine tradeoff, not an acceptable universal replacement.

## Next direction

The canonical Local/SSA + explicit effect context architecture supersedes this
selector. This checkpoint remains the rollback if that fails.
