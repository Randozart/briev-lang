# Reclamation solver representation — verification ledger

Base: `114b2a54`. Candidate: `fix/reclamation-solver-cost`, exact sparse-word
node sets. Compiler/application provenance and gates:
`docs/plans/2026-11-09-reclamation-solver-cost.md`.

## Compile cost

Same server source, release compiler, stdlib and config: baseline timed out at
600.12 s before emission; candidate native build succeeded in 197.21 s.
This is a **greater than 3.04x lower bound**, not an exact speedup. Debug builds
remain slow: old cancelled at 47m20s, candidate diagnostic capped at 10 minutes.
No debug speedup is inferred from these censored observations.

## Runtime harness

`bash benchmarks/build_and_bench.sh --runtime`, default five repetitions and
per-program geometry. Every baseline row passed correctness; Node.js bridge
subcase was skipped because Node.js is unavailable. Timings are **provisional**:
other compiler-validation jobs overlapped, so this table is a correctness/IR
regression ledger, not evidence of runtime wins or losses. No runtime improvement
is intended. Both runs passed all 44 summary rows; all 53 collected LLVM files
are byte-identical. Node.js was unavailable in both runs. Timing differences
are not attributed to this change: the emitted code is unchanged, and these
overlapping-job timings are not an isolated runtime-performance experiment.

| Benchmark | Before Briev (s) | Before C (s) | Before correctness |
|---|---:|---:|---|
| ring_buffer | .0576 | .0574 | MATCH |
| float_math | .0468 | .0571 | MATCH |
| float_math_nonzero | .1444 | .1511 | MATCH |
| sparse_dispatch | .0543 | .0480 | MATCH |
| print_loop | .0315 | .0539 | MATCH |
| nbody_newton | 5.5201 | 5.3193 | MATCH |
| nbody_newton_accel | 2.0727 | .1771 | MATCH |
| nbody_sqrt | 3.7255 | 3.3720 | MATCH |
| nbody_sqrt_idio | 3.0428 | 4.1834 | MATCH |
| fasta | .2835 | .3990 | MATCH |
| fannkuch_redux | .1191 | .1591 | MATCH |
| mandelbrot | .8290 | .8215 | MATCH |
| kalman_filter_runtime | .2124 | .2137 | MATCH |
| knucleotide | .2647 | .2834 | MATCH |
| cancel_math | .0726 | .0804 | MATCH |
| bit_clear | 0 | 0 | MATCH |
| queue_drain | .0417 | .0707 | MATCH |
| queue_drain_sym | .0405 | .0661 | MATCH |
| queue_drain_idio | .0391 | .0674 | MATCH |
| stack_push_pop | .0402 | .0749 | MATCH |
| interval_step | .0725 | .0769 | MATCH |
| telemetry_stream | .2211 | .2213 | MATCH |
| pid_control | .4274 | .3963 | MATCH |
| matrix_pipeline | .6661 | .6466 | MATCH |
| accumulator_flush | .1321 | .1950 | MATCH |
| sweep_sparse | .1926 | .1923 | MATCH |
| sweep_mid | .2059 | .2302 | MATCH |
| sweep_dense | .6078 | .5626 | MATCH |
| sweep_arr | .5648 | .5364 | MATCH |
| series_converge | 0 | 0 | MATCH |
| global_lifetime | .0460 | .0748 | MATCH |
| deep_recursion | .0001 | .0002 | MATCH |
| arena_churn | .0593 | .1000 | MATCH |
| linked_list | 1.5193 | 3.2640 | MATCH |
| hash_ops | 1.1431 | 1.4550 | MATCH |
| hash_ops_idio | 1.9801 | .9414 | MATCH |
| enemy_swarm | .1308 | .1513 | MATCH |
| recommender | .0211 | .0211 | MATCH |
| ws_frame | .0319 | .0269 | MATCH |
| fsm_dispatch | .2766 | .3206 | MATCH |
| arena_bump | .1993 | .0723 | MATCH |
| spsc_ring | .2542 | .1087 | MATCH |
| bridge_glue | separate bridge report | — | MATCH |
| bridge_multi | separate bridge report | — | PASS |

| Benchmark | After Briev (s) | After C (s) | After correctness |
|---|---:|---:|---|
| ring_buffer | .0543 | .0531 | MATCH |
| float_math | .0539 | .0554 | MATCH |
| float_math_nonzero | .1442 | .1508 | MATCH |
| sparse_dispatch | .0459 | .0480 | MATCH |
| print_loop | .0307 | .0520 | MATCH |
| nbody_newton | 5.1494 | 4.6726 | MATCH |
| nbody_newton_accel | 1.3631 | .1179 | MATCH |
| nbody_sqrt | 3.0472 | 2.9975 | MATCH |
| nbody_sqrt_idio | 2.6691 | 3.9013 | MATCH |
| fasta | .2429 | .3388 | MATCH |
| fannkuch_redux | .0971 | .1299 | MATCH |
| mandelbrot | .7165 | .6838 | MATCH |
| kalman_filter_runtime | .1671 | .1705 | MATCH |
| knucleotide | .2199 | .2237 | MATCH |
| cancel_math | .0577 | .0617 | MATCH |
| bit_clear | 0 | 0 | MATCH |
| queue_drain | .0330 | .0579 | MATCH |
| queue_drain_sym | .0346 | .0538 | MATCH |
| queue_drain_idio | .0322 | .0570 | MATCH |
| stack_push_pop | .0291 | .0591 | MATCH |
| interval_step | .0610 | .0666 | MATCH |
| telemetry_stream | .2043 | .2019 | MATCH |
| pid_control | .3635 | .3705 | MATCH |
| matrix_pipeline | .6369 | .6528 | MATCH |
| accumulator_flush | .1182 | .1648 | MATCH |
| sweep_sparse | .1518 | .1550 | MATCH |
| sweep_mid | .1720 | .1859 | MATCH |
| sweep_dense | .5400 | .5056 | MATCH |
| sweep_arr | .5068 | .4971 | MATCH |
| series_converge | 0 | .0002 | MATCH |
| global_lifetime | .0331 | .0619 | MATCH |
| deep_recursion | .0003 | 0 | MATCH |
| arena_churn | .0438 | .0840 | MATCH |
| linked_list | 1.0109 | 1.9963 | MATCH |
| hash_ops | 1.1463 | 1.4298 | MATCH |
| hash_ops_idio | 1.7009 | .7377 | MATCH |
| enemy_swarm | .1095 | .1154 | MATCH |
| recommender | .0161 | .0168 | MATCH |
| ws_frame | .0257 | .0215 | MATCH |
| fsm_dispatch | .2378 | .2748 | MATCH |
| arena_bump | .1595 | .0583 | MATCH |
| spsc_ring | .2413 | .0927 | MATCH |
| bridge_glue | separate bridge report | — | MATCH |
| bridge_multi | separate bridge report | — | PASS |

Raw logs: `/tmp/opencode/reclamation-baseline/runtime.log` and
`/tmp/opencode/reclamation-candidate/runtime.log`. Baseline emitted LLVM files
are preserved under `/tmp/opencode/reclamation-baseline/ir/` before the second
harness run. Standalone eight-program A/B already produced byte-identical IR.
