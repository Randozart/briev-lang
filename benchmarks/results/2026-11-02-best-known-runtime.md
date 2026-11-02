# Best-known runtime ratios — post-baseline ledger (2026-11-02)

**Purpose.** Account for the fastest valid Briev/C runtime ratio observed per
benchmark since the pinned baseline. This is a no-regression reference, not a
C-parity claim.

**Pinned baseline:** `compiler-baseline-2026-10-23-tail-loop`
(`42279aaf`). Measurements come from recorded full-runtime runs with every
correctness column `MATCH`.

**Rule.** Lower ratio is better (`Briev / C`). A row may replace the pinned
baseline only when the source records a full-runtime correctness pass. Rows
marked **invalid** were dead-code/precomputed artifacts or pointer-lifetime
accidents; they must never be used as a performance target.

## Post-baseline best known

Most rows are unchanged from the pinned baseline (no newer full-suite record).
Rows with post-baseline movement cite the newer source.

| Benchmark | Best known | Source |
|---|---:|---|
| accumulator_flush | 0.78x | 2026-10-23-tail-loop-lowering |
| arena_bump | 4.34x | 2026-10-30-d4-object-caller-destination |
| arena_churn | 0.28x | 2026-10-23-tail-loop-lowering |
| bit_clear | 1.25x | 2026-10-23-tail-loop-lowering |
| cancel_math | 0.67x | 2026-10-23-tail-loop-lowering |
| deep_recursion | 1.20x | 2026-10-23-tail-loop-lowering |
| enemy_swarm | 0.78x | 2026-10-23-tail-loop-lowering |
| fannkuch_redux | 1.12x | 2026-10-23-tail-loop-lowering |
| fasta | 0.58x | 2026-10-23-tail-loop-lowering |
| float_math | 0.67x | 2026-10-23-tail-loop-lowering |
| float_math_nonzero | 0.98x | 2026-10-23-tail-loop-lowering |
| fsm_dispatch | 0.56x | 2026-10-30-d4-object-caller-destination |
| global_lifetime | 0.44x | 2026-10-23-tail-loop-lowering |
| hash_ops | 0.80x | 2026-10-23-tail-loop-lowering |
| hash_ops_idio | 1.03x | 2026-10-23-tail-loop-lowering |
| interval_step | 0.62x | 2026-10-23-tail-loop-lowering |
| kalman_filter_runtime | 0.92x | 2026-10-23-tail-loop-lowering |
| knucleotide | 0.94x | 2026-10-23-tail-loop-lowering |
| linked_list | 0.49x | 2026-10-23-tail-loop-lowering |
| mandelbrot | 0.91x | 2026-10-23-tail-loop-lowering |
| matrix_pipeline | 0.92x | 2026-10-23-tail-loop-lowering |
| nbody_newton | 1.09x | 2026-10-23-tail-loop-lowering |
| nbody_newton_accel | 13.13x | 2026-10-23-tail-loop-lowering |
| nbody_sqrt | 0.77x | 2026-10-23-tail-loop-lowering |
| nbody_sqrt_idio | 0.90x | 2026-10-23-tail-loop-lowering |
| pid_control | 1.00x | 2026-10-23-tail-loop-lowering |
| print_loop | 0.45x | 2026-10-23-tail-loop-lowering |
| queue_drain | 0.47x | 2026-10-23-tail-loop-lowering |
| queue_drain_idio | 0.47x | 2026-10-23-tail-loop-lowering |
| queue_drain_sym | 0.44x | 2026-10-23-tail-loop-lowering |
| recommender | 0.93x | 2026-10-30-d4-object-caller-destination |
| ring_buffer | 0.86x | 2026-10-23-tail-loop-lowering |
| series_converge | 1.00x | 2026-09-30-b1-rebaseline |
| sparse_dispatch | 0.74x | 2026-10-23-tail-loop-lowering |
| spsc_ring | 1.85x | 2026-10-30-d4-object-caller-destination |
| stack_push_pop | 0.60x | 2026-10-23-tail-loop-lowering |
| sweep_arr | 1.13x | 2026-10-23-tail-loop-lowering |
| sweep_dense | 1.15x | 2026-10-23-tail-loop-lowering |
| sweep_mid | 1.29x | 2026-10-23-tail-loop-lowering |
| sweep_sparse | 0.95x | 2026-10-23-tail-loop-lowering |
| telemetry_stream | 0.75x | 2026-10-23-tail-loop-lowering |
| ws_frame | 0.93x | 2026-10-30-d5b-object-frame-images |

`fsm_dispatch`, `arena_bump`, `spsc_ring`, `recommender`, and `ws_frame` are
the rows with confirmed post-baseline improvement records. The controlled
`ws_frame` A/B in `2026-10-30-d4-object-caller-destination.md` is the stronger
evidence (1.510x C); the 0.93x row is a full-runtime harness result from D5b
and was not used as a decision measurement.

## Latest validated snapshot (2026-11-02)

From `2026-11-02-destination-ownership-final.md`; all outputs `MATCH`.

| Benchmark | Briev | C | Ratio |
|---|---:|---:|---:|
| ws_frame | 0.0152s | 0.0114s | 1.33x |
| recommender | 0.0125s | 0.0120s | 1.04x |
| hash_ops | 0.9690s | 1.1358s | 0.85x |
| fsm_dispatch | 0.1402s | 0.1616s | 0.87x |
| arena_bump | 0.3248s | 0.0499s | 6.51x |
| spsc_ring | 0.1489s | 0.0732s | 2.03x |

## Rows to verify before accepting no-regression

These latest rows are above the post-baseline best known and therefore need a
controlled same-commit A/B before a release verdict:

| Benchmark | Best known | Latest | Delta |
|---|---:|---:|---:|
| arena_bump | 4.34x | 6.51x | +50% |
| spsc_ring | 1.85x | 2.03x | +10% |
| fsm_dispatch | 0.56x | 0.87x | +55% |
| recommender | 0.93x | 1.04x | +12% |
| hash_ops | 0.80x | 0.85x | +6% |
| ws_frame | 0.93x | 1.33x | +43% |

No row is declared regressed from these cross-date numbers alone. Machine and
run variation are large enough that each needs the pinned-baseline A/B.

## Invalid historical records — do not use

These recorded very-low ratios came from folded/dead-code binaries or invalid
lifetime behavior. They are excluded from best-known accounting:

- `ws_frame` 0.25x (2026-10-23): the old fast path returned a pointer into a
  dead callee frame; the corrected ABI cannot use it.
- `queue_drain` 0.01x, `queue_drain_sym` 0.00x, `queue_drain_idio` 0.00x,
  `interval_step` 0.01x, `sparse_dispatch` 0.09x, `float_math` 0.08x,
  `print_loop` 0.00x, `cancel_math` 0.00x: optimizer/precompute artifacts from
  the old runtime harness, not comparable runtime work.
- `ws_frame` 0.36x from the D8 retention-helper probe: invalid because the
  probe broke real-browser I11 correctness.

## Accounting notes

- A ratio below 1.0 means Briev is faster than the C reference for that row.
- `nbody_newton_accel` is the largest known C gap in the baseline table and is
  outside Tier 1 native CPU parity until the accel lane is revisited.
- This ledger records measurements only. It does not authorize a baseline
  re-lock; `benchmarks/BASELINE.lock` stays pinned.
