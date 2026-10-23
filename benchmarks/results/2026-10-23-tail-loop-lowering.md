# Tail-recursion loop lowering — benchmark record

**Date:** 2026-10-23  
**Branch:** `feat/tail-loop-lowering`  
**Base:** `main` at `ad989e62`  
**Baseline:** `compiler-baseline-2026-10-21-carrier` (`5a1a0532`)

## Full runtime suite

| Benchmark | Briev | C | Ratio | Winner | Correct |
|---|---:|---:|---:|---|---|
| ring_buffer | 0.0568s | 0.0661s | 0.86x | Briev | MATCH |
| float_math | 0.0420s | 0.0627s | 0.67x | Briev | MATCH |
| float_math_nonzero | 0.1628s | 0.1659s | 0.98x | Briev | MATCH |
| sparse_dispatch | 0.0505s | 0.0685s | 0.74x | Briev | MATCH |
| print_loop | 0.0313s | 0.0690s | 0.45x | Briev | MATCH |
| nbody_newton | 10.0341s | 9.1716s | 1.09x | C | MATCH |
| nbody_newton_accel | 2.4370s | 0.1856s | 13.13x | C | MATCH |
| nbody_sqrt | 3.2910s | 4.2520s | 0.77x | Briev | MATCH |
| nbody_sqrt_idio | 3.6718s | 4.0623s | 0.90x | Briev | MATCH |
| fasta | 0.1993s | 0.3412s | 0.58x | Briev | MATCH |
| fannkuch_redux | 0.0930s | 0.0833s | 1.12x | C | MATCH |
| mandelbrot | 0.7348s | 0.8058s | 0.91x | Briev | MATCH |
| kalman_filter_runtime | 0.1737s | 0.1898s | 0.92x | Briev | MATCH |
| knucleotide | 0.1939s | 0.2067s | 0.94x | Briev | MATCH |
| cancel_math | 0.0467s | 0.0697s | 0.67x | Briev | MATCH |
| bit_clear | 0.0005s | 0.0004s | 1.25x | C | MATCH |
| queue_drain | 0.0325s | 0.0694s | 0.47x | Briev | MATCH |
| queue_drain_sym | 0.0313s | 0.0707s | 0.44x | Briev | MATCH |
| queue_drain_idio | 0.0324s | 0.0685s | 0.47x | Briev | MATCH |
| stack_push_pop | 0.0406s | 0.0673s | 0.60x | Briev | MATCH |
| interval_step | 0.0469s | 0.0760s | 0.62x | Briev | MATCH |
| telemetry_stream | 0.1670s | 0.2223s | 0.75x | Briev | MATCH |
| pid_control | 0.3523s | 0.3516s | 1.00x | ~tie | MATCH |
| matrix_pipeline | 0.6148s | 0.6659s | 0.92x | Briev | MATCH |
| accumulator_flush | 0.1580s | 0.2032s | 0.78x | Briev | MATCH |
| sweep_sparse | 0.1524s | 0.1604s | 0.95x | Briev | MATCH |
| sweep_mid | 0.2452s | 0.1896s | 1.29x | C | MATCH |
| sweep_dense | 0.4322s | 0.3768s | 1.15x | C | MATCH |
| sweep_arr | 0.5028s | 0.4448s | 1.13x | C | MATCH |
| series_converge | 0.0002s | 0.0000s | N/Ax | — | MATCH |
| global_lifetime | 0.0330s | 0.0753s | 0.44x | Briev | MATCH |
| deep_recursion | 0.0006s | 0.0005s | 1.20x | C | MATCH |
| arena_churn | 0.0316s | 0.1134s | 0.28x | Briev | MATCH |
| linked_list | 1.3417s | 2.7271s | 0.49x | Briev | MATCH |
| hash_ops | 1.4982s | 1.8749s | 0.80x | Briev | MATCH |
| hash_ops_idio | 0.9572s | 0.9307s | 1.03x | C | MATCH |
| enemy_swarm | 0.1375s | 0.1765s | 0.78x | Briev | MATCH |
| recommender | 0.0144s | 0.0126s | 1.14x | C | MATCH |
| ws_frame | 0.0075s | 0.0298s | 0.25x | Briev | MATCH |
| fsm_dispatch | 0.1924s | 0.2998s | 0.64x | Briev | MATCH |
| arena_bump | 0.2879s | 0.0635s | 4.53x | C | MATCH |
| spsc_ring | 0.2466s | 0.1035s | 2.38x | C | MATCH |
| bridge_glue | done |  |  |  | MATCH |
| bridge_multi | done |  |  |  | PASS |

All rows MATCH. The run showed elevated machine noise (C-reference times
also drifted); ratio-level regressions were rechecked with the locked-baseline
A/B harness.

## Exhaustive corpus IR identity

The full standard benchmark corpus was recompiled from source by both
compilers with the harness's exact BOUND, BODYCOUNT, and optimize-budget:

- baseline: `compiler-baseline-2026-10-21-carrier` (`5a1a0532`)
- current: `feat/tail-loop-lowering` (base `ad989e62`)
- corpus: all 46 standard `.bv` benchmark entries
- gate: byte-for-byte LLVM IR comparison

Result: **45 IDENTICAL, 1 DIFF, 0 MISSING**.

The only differing file is `fsm_dispatch.ll`, as intended: it gains the
`.tail_header_0` loop and loses the recursive self-call in `@fsm_run_loop`.
Every other benchmark emits byte-identical IR, so none of them can regress from
this patch.

## Controlled A/B

| Check | Result |
|---|---|
| `fsm_dispatch` vs carrier baseline | **0.3969×** (current 0.1824s, baseline 0.4596s) |
| `ws_frame` generated IR | byte-identical to baseline |
| `ws_frame` binary | byte-identical to baseline |
| `spsc_ring` generated IR | byte-identical to baseline |
| `spsc_ring` binary | byte-identical to baseline |
| `recommender`, `hash_ops_idio`, `linked_list`, `arena_bump`, `nbody_newton`, `deep_recursion` | generated IR byte-identical to baseline |
| `nbody_newton_accel` (BODYCOUNT=2048) | generated IR byte-identical to baseline |

The first broad implementation lowered plain tail recursion too and regressed
`ws_frame` (~1.45×). The backend now gates loop lowering on a loop-carried
nested-enum carrier, so only the FSM-shaped functions change.

The full-suite table above was collected on a noisy machine: several
C-reference times drifted, and rows with byte-identical IR still moved in the
ratio column. The IR-identity checks above are the authoritative regression
gate for non-FSM rows.

## FSM correctness

| BOUND | Output |
|---:|---:|
| 0 | 0 |
| 1 | 1 |
| 2 | 1000001 |
| 3 | 1000004 |
| 4 | 2000004 |
| 5 | 2000008 |
| 6 | 2000015 |
| 7 | 2000024 |
| 8 | 3000024 |
| 1,000,000 | 571428714296 |
| 50,000,000 | 28571512714296 |

FSM `malloc(i64 16)` count: **0**. No recursive self-call remains in
`@fsm_run_loop`.

## Gates

- `cargo test --lib`: **3,301 passed**
- effect-shape quick guard: **PASS**
- LLVM determinism quick gate: **ALL DETERMINISTIC**
- full runtime suite: **all MATCH**
