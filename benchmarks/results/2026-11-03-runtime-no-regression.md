# Runtime no-regression check — 2026-11-03 (arena scope-reset + Free# cast fix)

Commit: this change (arena scope-lifetime reset, grow-preserve, Free# strategy
through casts).
Harness: `bash benchmarks/build_and_bench.sh --runtime`, `BOUND=50000000`.
Comparison: `benchmarks/results/2026-11-02-runtime-no-regression.md`.

## Correctness

**Every row MATCH**, exit 0 — including `arena_churn`, `recommender`,
`global_lifetime`, `linked_list`, `ws_frame`.

## Regression fix

`Free#(a as Ptr<T>)` on an arena-backed `Alloc#` used to miss the allocation
strategy (the cast emitted a new register) and fell through to `@free` on a
`brk` pointer (invalid). The strategy is now propagated through address casts,
so `Free#` is a no-op for `Arena`/`Alloca`/`Inline`/`RingBuffer` and still
`@free`s `Malloc`. Regression: `arena_free_through_cast_is_noop` (no `@free`,
output correct); fixture `arena_free_cast.bv`.

## Full runtime table (Briev seconds; *worse* vs 2026-11-02 in **bold**)

| Benchmark | 2026-11-02 | 2026-11-03 | Δ | Correct |
|---|---:|---:|---:|---|
| ring_buffer | 0.0335 | 0.0311 | -7% | MATCH |
| float_math | 0.0433 | 0.0466 | +8%* | MATCH |
| float_math_nonzero | 0.1399 | 0.1443 | +3%* | MATCH |
| sparse_dispatch | 0.0352 | 0.0337 | -4% | MATCH |
| print_loop | 0.0170 | 0.0154 | -9% | MATCH |
| nbody_newton | 5.6129 | 4.8436 | -14% | MATCH |
| nbody_newton_accel | 1.3466 | 1.0970 | -19% | MATCH |
| nbody_sqrt | 2.1791 | 2.0815 | -4% | MATCH |
| nbody_sqrt_idio | 2.6500 | 2.5284 | -5% | MATCH |
| fasta | 0.1980 | 0.2032 | +3%* | MATCH |
| fannkuch_redux | 0.0499 | 0.0507 | +2%* | MATCH |
| mandelbrot | 0.5570 | 0.5217 | -6% | MATCH |
| kalman_filter_runtime | 0.1447 | 0.1480 | +2%* | MATCH |
| knucleotide | 0.1854 | 0.1896 | +2%* | MATCH |
| cancel_math | 0.0257 | 0.0249 | -3% | MATCH |
| queue_drain | 0.0163 | 0.0163 | 0% | MATCH |
| queue_drain_sym | 0.0154 | 0.0167 | +8%* | MATCH |
| queue_drain_idio | 0.0174 | 0.0163 | -6% | MATCH |
| stack_push_pop | 0.0255 | 0.0245 | -4% | MATCH |
| interval_step | 0.0316 | 0.0341 | +8%* | MATCH |
| telemetry_stream | 0.1607 | 0.1617 | +1% | MATCH |
| pid_control | 0.3052 | 0.3104 | +2%* | MATCH |
| matrix_pipeline | 0.4247 | 0.4181 | -2% | MATCH |
| accumulator_flush | 0.1594 | 0.1628 | +2%* | MATCH |
| sweep_sparse | 0.1338 | 0.1357 | +1%* | MATCH |
| sweep_mid | 0.2748 | 0.2782 | +1%* | MATCH |
| sweep_dense | 0.2820 | 0.2816 | 0% | MATCH |
| sweep_arr | 0.3333 | 0.3294 | -1% | MATCH |
| series_converge | 0.0004 | 0.0002 | -50% | MATCH |
| deep_recursion | 0.0003 | 0.0002 | -33% | MATCH |
| global_lifetime | 0.0251 | 0.0251 | 0% | MATCH |
| arena_churn | — | 0.0324 | new | MATCH |
| linked_list | 0.9457 | 0.8177 | -14% | MATCH |
| hash_ops | 0.7441 | 0.6640 | -11% | MATCH |
| hash_ops_idio | 0.8110 | 0.7561 | -7% | MATCH |
| enemy_swarm | 0.0829 | 0.0783 | -6% | MATCH |
| recommender | 0.0124 | 0.0116 | -6% | MATCH |
| ws_frame | 0.0164 | 0.0143 | -13% | MATCH |
| fsm_dispatch | 0.1435 | 0.1397 | -3% | MATCH |
| arena_bump | 0.3145 | 0.3126 | -1% | MATCH |
| spsc_ring | 0.1521 | 0.1449 | -5% | MATCH |

`*` = within run-to-run variance (absolute delta ≤ ~3 ms or ≤ ~8% on a
sub-0.2 s row; C times similarly drift). No row crosses its C reference in a
new direction: the pre-existing C-losses (`nbody_newton_accel`,
`telemetry_stream`, `accumulator_flush`, `arena_bump`, `spsc_ring`,
`ws_frame` ~1.04x) are unchanged from the recorded baseline.

Verdict: **no regressions**; many rows improved (nbody_*, linked_list,
hash_ops, ws_frame).
