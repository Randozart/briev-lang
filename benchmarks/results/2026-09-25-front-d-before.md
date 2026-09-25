# Front D Baseline — 2026-09-25

Commit: `7ee700f2` (fix(llvm): flush buffered stdout at every host main exit)
Machine: 2× RTX 3060 12GB (sm_86), driver 615.71.09, hosted x86_64 lane
Harness: `bash benchmarks/build_and_bench.sh --runtime` (full log:
`/tmp/opencode/runtime-baseline-raw.log`, sweep pid 9175, 2026-09-25)

## Session Context

Sweep blocked at nbody_newton_accel since 2026-09-24 (SIGSEGV). This
baseline unblocks it: four compiler bugs fixed first (`7b8ebaa2` —
descriptor ABI lockstep, stale accel archive, probe state_size, probe
sandbox purity), then two more found BY the sweep (`7ee700f2` —
missing stdout flush at host main exits, precomputed-main duplicate
`%state`). Bit_clear and deep_recursion MISMATCHed in the sweep table
below (fixed in `7ee700f2`); their post-fix timings sit on the
fork+exec floor (~0.02–0.03 s wall incl. harness overhead), parity
with C at that floor.

## CPU Runtime Benchmarks (Briev / C, lower ratio = Briev wins)

| Benchmark | Briev | C | Ratio | Correct |
|-----------|-------|---|-------|---------|
| ring_buffer | 0.0627s | 0.0710s | 0.88x | MATCH |
| float_math | 0.0424s | 0.0637s | 0.67x | MATCH |
| float_math_nonzero | 0.2254s | 0.1812s | 1.24x | MATCH |
| sparse_dispatch | 0.0907s | 0.0776s | 1.17x | MATCH |
| print_loop | 0.0358s | 0.0808s | 0.44x | MATCH |
| nbody_newton | 8.8307s | 11.0708s | 0.80x | MATCH |
| nbody_newton_accel | 2.3821s | 0.1995s | 11.94x | MATCH |
| nbody_sqrt | 3.4070s | 4.6878s | 0.73x | MATCH |
| nbody_sqrt_idio | 3.8125s | 4.6931s | 0.81x | MATCH |
| fasta | 0.1968s | 0.3179s | 0.62x | MATCH |
| fannkuch_redux | 0.0950s | 0.0961s | 0.99x | MATCH |
| mandelbrot | 0.9132s | 0.7919s | 1.15x | MATCH |
| kalman_filter_runtime | 0.1929s | 0.1811s | 1.07x | MATCH |
| knucleotide | 0.2513s | 0.2105s | 1.19x | MATCH |
| cancel_math | 0.0464s | 0.0735s | 0.63x | MATCH |
| bit_clear | 0.0003s | 0.0004s | 0.75x | MISMATCH → fixed 7ee700f2 |
| queue_drain | 0.0878s | 0.0694s | 1.27x | MATCH |
| queue_drain_sym | 0.0330s | 0.0676s | 0.49x | MATCH |
| queue_drain_idio | 0.0322s | 0.0709s | 0.45x | MATCH |
| stack_push_pop | 0.0406s | 0.0711s | 0.57x | MATCH |
| interval_step | 0.0856s | 0.0743s | 1.15x | MATCH |
| telemetry_stream | 0.1961s | 0.2161s | 0.91x | MATCH |
| pid_control | 0.3442s | 0.3461s | 0.99x | MATCH |
| matrix_pipeline | 0.7180s | 0.8620s | 0.83x | MATCH |
| accumulator_flush | 0.1902s | 0.1947s | 0.98x | MATCH |
| sweep_sparse | 0.2054s | 0.1641s | 1.25x | MATCH |
| sweep_mid | 0.3009s | 0.3004s | 1.00x | ~tie MATCH |
| sweep_dense | 0.5530s | 0.3591s | 1.54x | MATCH |
| sweep_arr | 0.8128s | 0.6740s | 1.21x | MATCH |
| series_converge | 0.0003s | 0.0002s | 1.50x | MATCH |
| global_lifetime | 0.0433s | 0.0943s | 0.46x | MATCH |
| deep_recursion | 0.0005s | 0.0002s | 2.50x | MISMATCH → fixed 7ee700f2 |
| arena_churn | 0.0326s | 0.1261s | 0.26x | MATCH |
| linked_list | 1.3325s | 2.7349s | 0.49x | MATCH |
| hash_ops | 1.4583s | 1.8605s | 0.78x | MATCH |
| hash_ops_idio | 1.0115s | 0.9918s | 1.02x | MATCH |
| enemy_swarm | 0.1616s | 0.1976s | 0.82x | MATCH |
| bridge_glue | done | — | — | MATCH |
| bridge_multi | done | — | — | PASS |

36/38 correctness MATCH at sweep time; 38/38 after `7ee700f2`.

## Notes

- **nbody_newton_accel 11.94x**: correct output, CPU-lane fallback
  (probe verdict 0 — no CUDA kernel images emitted for the .bv lane on
  this box: `no cuda image — CPU lane`). The GPU path is the followup-
  stages GPU re-rank wave's target, not Front D.
- **bit_clear / deep_recursion**: sweep timings are the pre-fix no-op
  binaries (output lost at exit — the work itself was real but the
  binary did less than the C ref). Post-fix both compute correctly at
  the fork+exec floor; ratios there are noise-floor dominated (see
  bit_clear.bv header).
- **bridge_multi PASS**: the 2026-09-24 multi_lang build death is gone
  (shared-entry gating fix, `8dc6138c`).
- Sweep ran against `7ee700f2`-parent binaries for the table rows and
  the two fixed benchmarks were rebuilt+verified at `7ee700f2`.

## Front D Gate (from followup-stages plan)

Plain `ptx_deferred_region: 0` vs deferred `1` A/B on the attention
composite; gate: plain ≤ 1.10× deferred, both lanes correct
(`max_rel < 1e-3`). Baseline of record for that comparison lives in
`2026-09-20-baseline.md` (deferred 198 µs, flash v4 125 µs) — the A/B
re-measures both knobs on today's tree.
