# B1 re-baseline — 2026-09-30 (Phase B step 1, Rule 12)

Commit: `6dca75b0` (dual-image dispatch CUDA fix — the session's three
GPU commits `630d0e27`, `76ce50a6`, `6dca75b0` are in).
Machine: RTX 3060 (sm_86), driver 615.71.09, hosted x86_64 lane.
Harness: `bash benchmarks/build_and_bench.sh --runtime` then `--optimizer`.
Raw logs: `/tmp/opencode/b1_runtime.log`, `/tmp/opencode/b1_optimizer.log`.
Comparison reference: `benchmarks/results/2026-09-25-front-d-before.md`
(commit `7ee700f2`, same machine + driver).

## Runtime throughput (`--runtime`, BOUND=50M, 5 iters, avg wall)

| Benchmark | Briev | C | Ratio | Winner | Correct |
|-----------|-------|---|-------|--------|---------|
| ring_buffer | 0.0443s | 0.0465s | 0.95x | Briev | MATCH |
| float_math | 0.0458s | 0.0468s | 0.98x | Briev | MATCH |
| float_math_nonzero | 0.2020s | 0.1665s | 1.21x | C | MATCH |
| sparse_dispatch | 0.0868s | 0.0630s | 1.38x | C | MATCH |
| print_loop | 0.0301s | 0.0585s | 0.51x | Briev | MATCH |
| nbody_newton | 6.8468s | 7.6670s | 0.89x | Briev | MATCH |
| nbody_newton_accel | 1.3823s | 0.1308s | 10.57x | C | MATCH* |
| nbody_sqrt | 2.3667s | 3.3130s | 0.71x | Briev | MATCH |
| nbody_sqrt_idio | 2.7595s | 3.5667s | 0.77x | Briev | MATCH |
| fasta | 0.1874s | 0.2625s | 0.71x | Briev | MATCH |
| fannkuch_redux | 0.0648s | 0.0678s | 0.96x | Briev | MATCH |
| mandelbrot | 0.7915s | 0.6800s | 1.16x | C | MATCH |
| kalman_filter_runtime | 0.1672s | 0.1801s | 0.93x | Briev | MATCH |
| knucleotide | 0.2416s | 0.1910s | 1.26x | C | MATCH |
| cancel_math | 0.0445s | 0.0606s | 0.73x | Briev | MATCH |
| bit_clear | 0.0005s | 0.0004s | 1.25x | C | MATCH |
| queue_drain | 0.0905s | 0.0609s | 1.49x | C | MATCH |
| queue_drain_sym | 0.0302s | 0.0604s | 0.50x | Briev | MATCH |
| queue_drain_idio | 0.0291s | 0.0608s | 0.48x | Briev | MATCH |
| stack_push_pop | 0.0311s | 0.0628s | 0.50x | Briev | MATCH |
| interval_step | 0.0859s | 0.0615s | 1.40x | C | MATCH |
| telemetry_stream | 0.1559s | 0.1577s | 0.99x | Briev | MATCH |
| pid_control | 0.3454s | 0.3455s | 1.00x | ~tie | MATCH |
| matrix_pipeline | 0.5222s | 0.6385s | 0.82x | Briev | MATCH |
| accumulator_flush | 0.1313s | 0.1425s | 0.92x | Briev | MATCH |
| sweep_sparse | 0.2047s | 0.1532s | 1.34x | C | MATCH |
| sweep_mid | 0.2975s | 0.3006s | 0.99x | Briev | MATCH |
| sweep_dense | 0.5043s | 0.3327s | 1.52x | C | MATCH |
| sweep_arr | 0.7356s | 0.6258s | 1.18x | C | MATCH |
| series_converge | 0.0002s | 0.0002s | 1.00x | ~tie | MATCH |
| global_lifetime | 0.0290s | 0.0659s | 0.44x | Briev | MATCH |
| deep_recursion | 0.0003s | 0.0003s | 1.00x | ~tie | MATCH |
| arena_churn | 0.0319s | 0.0924s | 0.35x | Briev | MATCH |
| linked_list | 0.9854s | 1.9473s | 0.51x | Briev | MATCH |
| hash_ops | 0.8504s | 0.9978s | 0.85x | Briev | MATCH |
| hash_ops_idio | 0.7367s | 0.6937s | 1.06x | C | MATCH |
| enemy_swarm | 0.1024s | 0.1313s | 0.78x | Briev | MATCH |
| bridge_glue | done | — | — | — | MATCH |
| bridge_multi | done | — | — | — | PASS |

38/38 after the nbody_newton_accel fix below (37/38 at sweep time —
the FAIL was a harness false positive, see next section).

\* FAIL in the raw sweep; MATCH post-fix (verified by re-running
`bash benchmarks/build_and_bench.sh nbody_newton_accel` → 11.19x MATCH).

## Optimizer (`--optimizer`, precompute category)

| Benchmark | Result | Correct |
|-----------|--------|---------|
| iir_filter | precomputed | MATCH |
| precompute_sum | precomputed | MATCH |
| const_heavy | precomputed | MATCH |
| async_counters_idio | precomputed | MISMATCH (line count 10 vs 1) |
| UTF8_ops | (no binary — source archived) | SKIP |

## Findings

### 1. nbody_newton_accel FAIL = harness telemetry false positive (fixed)

The Briev binary's stdout matched C exactly (`0.50002861` both,
`BOUND=5` correctness run); FAIL came from Bug 5's non-empty-stderr rule
counting the device runtime's `# gpu_time: …` telemetry lines as a
runtime error. Telemetry only appears once the Vulkan lane actually
dispatches — which it now does (this bench was CPU-lane-fallback at
the 09-25 baseline). Fix in `benchmarks/build_and_bench.sh`: filter
`^# ` lines from the stderr emptiness test (crash diagnostics never
use the `# ` prefix — `briev: dispatch failed`, driver
`[briev_accel/…]`, panics). Post-fix rerun: **MATCH, 11.19x**.

### 2. No Briev-side runtime regressions vs 2026-09-25

Absolute Briev times improved or held across the whole table (e.g.
nbody_newton 8.83→6.85, hash_ops 1.46→0.85, matrix_pipeline
0.72→0.52, nbody_sqrt 3.41→2.37, enemy_swarm 0.16→0.10). The C
reference also got faster on the same machine (nbody_newton C
11.07→7.67, float_math C 0.064→0.047) — run-to-run machine variance
moves ratios in both directions; Briev absolute time is the
regression signal and shows none.

One ratio worsened while the machine was otherwise faster
(`float_math` 0.67x→0.98x, Briev 0.0424→0.0458) → controlled A/B
per B3/Rule 12b: `bash benchmarks/compare_baseline.sh float_math`
→ baseline `5d1d7e45` avg 0.4150s vs current 0.4116s, ratio
**0.9918 — within tolerance, no regression**
(`/tmp/opencode/b3_float_math.log`).

### 3. nbody_newton_accel perf (known, unchanged concern)

CPU-lane fallback ran 2.3821s (09-25); the now-working GPU lane runs
1.3823s — better than fallback, still 10.6x behind C (0.1308s).
Per-launch `gpu_time` is ~0.003 ms while wall is 1.38 s: host-side
per-step launch/sync overhead dominates this small-kernel-per-step
shape. Not a regression; B4 re-rank material.

### 4. async_counters_idio MISMATCH — pre-existing, deferred

Recorded MISMATCH since `benchmarks/results/2026-07-19-post-migration.md`
(pre-existing column; precomputed output 10 lines vs C's 1). No
September record re-verified it; not introduced by this session.

### 5. UTF8_ops SKIP — source archived, honest skip

`utf8_ops.bv` moved to `benchmarks/archive/legacy-stdlib/` in `80c89d4c`
when its dependencies (`std/core/ring_buffer`, `std/types/UTF8view`)
were deleted; the harness entry `UTF8_ops` can never build again
without resurrecting those stdlib modules. The stale binary
`benchmarks/utf8_ops` (Jul 29) does not match the entry's case, so
the harness correctly reports missing → SKIP. Left in place pending
B4/sweep decisions; not a session regression.

## Status

- B1 steps 1–3 done (both categories, full tables recorded here).
- B1 step 4 = this file.
- B1 step 5 = stage-5 notes updated in
  `docs/plans/2026-09-24-followup-stages.md` (same commit).
- B3: only float_math flagged and A/B'd — OK.
