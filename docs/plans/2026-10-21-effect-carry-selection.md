# Effect-carry selection (best-of-both interim)

**Date:** 2026-10-21
**Branch:** `experiment/effect-carry-selection`
**Base:** canonical Local/effect branch (`106e04dc`) + carry experiment (`63f0e8ca`)
**Status:** CANDIDATE — full runtime suite MATCH, no confirmed regressions.

## Purpose

Use the carry shape only where it wins, while keeping the escaped shape where
it already wins. This is the interim best-of-both policy; the canonical Local +
explicit-effect architecture remains the long-term direction.

## Selection rule

Per program:

```
all direct-Float loops prefer carry AND use countdown
    -> stateless print + carry selected loops
otherwise
    -> escaped print (current fallback)
```

The countdown requirement is enforced by `BatchShape.txn_name` +
`program_carry_mode`: a no-batch direct loop falls back to escaped rather than
suppressing escaped and emitting a stateless loop with no carry.

Per direct-Float loop:

```
carried_float_count > 0
and not fully temp-mediated
and carried_float_count <= target.float_registers
and (temp_mediated_float_count >= target.effect_carry_temp_min
     or any vector group width <= target.vector_min_width)
```

This selects:

- carry: `float_math_nonzero`, `kalman_filter_runtime`, `sweep_sparse`
- escaped: `float_math`, `telemetry_stream`, `pid_control`,
  `accumulator_flush`, `sweep_mid`, `nbody_*`, mixed direct loop
- stateless: queue/interval/matrix/sweep_dense and aggregate loops

No benchmark names, field prefixes, or vocabulary matching. The numbers are
target config (`float_registers`, `vector_min_width`).

## Full runtime suite

`bash benchmarks/build_and_bench.sh --runtime` — all correctness rows MATCH.

| Benchmark | Ratio |
|---|---:|
| float_math | **0.92x** |
| float_math_nonzero | **0.97x** |
| kalman_filter_runtime | **0.86x** |
| sweep_sparse | **0.98x** |
| queue_drain | **0.50x** |
| interval_step | **0.71x** |
| telemetry_stream | 0.99x |
| pid_control | 1.00x |
| accumulator_flush | 0.86x |
| matrix_pipeline | 0.97x |
| sweep_mid | 1.40x |
| nbody_newton | 1.09x |

Compared with the current escaped selector, the clear wins are
`float_math_nonzero` (−20%), `kalman` (−14%), and `sweep_sparse` (−26%);
`float_math` stays 0.92x, queue/interval/matrix keep their wins. Remaining
differences (telemetry 0.99 vs 0.96, sweep_mid 1.40 vs 1.33–1.38, nbody 1.09
vs 1.08) are within run-to-run variance and are not confirmed regressions.

## Guard

`benchmarks/effect_shape_guard.sh --quick` now recognizes three decisions
(`escaped`, `carry`, `stateless`) and pins:

- carry: float_math_nonzero, kalman_filter_runtime, sweep_sparse
- escaped: float_math, telemetry, pid, accumulator, sweep_mid, mixed
- stateless: queue, interval, matrix, sweep_dense

It also keeps the mixed output-multiset and plain-context checks. The timing
mode adds catastrophic ceilings for the carry winners (`float_math_nonzero`
1.15x, `kalman` 1.05x, `sweep_sparse` 1.15x).

## Rollback

- Carry experiment checkpoint remains
  `checkpoint-effect-state-carry-2026-10-21`.
- Disable selection with `BRIEV_DIAG_EFFECT_CARRY=off` (escaped fallback).
- Force stateless with `BRIEV_DIAG_EFFECT=stateless`.
- The canonical branch remains the long-term target; this policy can be retired
  when the canonical Local/effect path matches the suite.

## Commit log

| Commit | Change |
|---|---|
| `63f0e8ca` | carry experiment ported onto canonical base |
| `157470c3` | carry selector predicate + target-derived program mode; three-way guard; full-suite record |
| `d1427095` | countdown-eligibility gate; `effect_carry_temp_min` target config |
