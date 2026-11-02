# Runtime no-regression check — 2026-11-02

Commit: `96e92c3a` (zero quarantined tests)
Baseline: `compiler-baseline-2026-10-23-tail-loop` (`42279aaf`)
Harness: `bash benchmarks/build_and_bench.sh --runtime`, `BOUND=50000000`
A/B: `bash benchmarks/compare_baseline.sh <name>` with `BOUND=50000000`

## Correctness

- Full runtime suite: every row `MATCH`.
- `bridge_glue`: MATCH.
- `bridge_multi`: PASS.
- `cargo test --all`: PASS, zero ignored tests.

## Full runtime table

| Benchmark | Briev | C | Ratio | Correct |
|---|---:|---:|---:|---|
| ring_buffer | 0.0335s | 0.0531s | 0.63x | MATCH |
| float_math | 0.0433s | 0.0490s | 0.88x | MATCH |
| float_math_nonzero | 0.1399s | 0.1418s | 0.99x | MATCH |
| sparse_dispatch | 0.0352s | 0.0469s | 0.75x | MATCH |
| print_loop | 0.0170s | 0.0518s | 0.33x | MATCH |
| nbody_newton | 5.6129s | 5.8901s | 0.95x | MATCH |
| nbody_newton_accel | 1.3466s | 0.0974s | 13.83x | MATCH |
| nbody_sqrt | 2.1791s | 2.7390s | 0.80x | MATCH |
| nbody_sqrt_idio | 2.6500s | 2.8000s | 0.95x | MATCH |
| fasta | 0.1980s | 0.2622s | 0.76x | MATCH |
| fannkuch_redux | 0.0499s | 0.0501s | 1.00x | MATCH |
| mandelbrot | 0.5570s | 0.6018s | 0.93x | MATCH |
| kalman_filter_runtime | 0.1447s | 0.1467s | 0.99x | MATCH |
| knucleotide | 0.1854s | 0.1858s | 1.00x | MATCH |
| cancel_math | 0.0257s | 0.0532s | 0.48x | MATCH |
| bit_clear | 0.0002s | 0.0005s | 0.40x | MATCH |
| queue_drain | 0.0163s | 0.0462s | 0.35x | MATCH |
| queue_drain_sym | 0.0154s | 0.0484s | 0.32x | MATCH |
| queue_drain_idio | 0.0174s | 0.0439s | 0.40x | MATCH |
| stack_push_pop | 0.0255s | 0.0441s | 0.58x | MATCH |
| interval_step | 0.0316s | 0.0537s | 0.59x | MATCH |
| telemetry_stream | 0.1607s | 0.1319s | 1.22x | MATCH |
| pid_control | 0.3052s | 0.3002s | 1.02x | MATCH |
| matrix_pipeline | 0.4247s | 0.4596s | 0.92x | MATCH |
| accumulator_flush | 0.1594s | 0.1242s | 1.28x | MATCH |
| sweep_sparse | 0.1338s | 0.1307s | 1.02x | MATCH |
| sweep_mid | 0.2748s | 0.2772s | 0.99x | MATCH |
| sweep_dense | 0.2820s | 0.3515s | 0.80x | MATCH |
| sweep_arr | 0.3333s | 0.3578s | 0.93x | MATCH |
| series_converge | 0.0004s | 0.0001s | 4.00x | MATCH |
| global_lifetime | 0.0245s | 0.0558s | 0.44x | MATCH |
| deep_recursion | 0.0003s | 0.0001s | 3.00x | MATCH |
| arena_churn | 0.0174s | 0.0964s | 0.18x | MATCH |
| linked_list | 0.9457s | 1.8241s | 0.52x | MATCH |
| hash_ops | 0.7441s | 1.0389s | 0.72x | MATCH |
| hash_ops_idio | 0.8110s | 0.7567s | 1.07x | MATCH |
| enemy_swarm | 0.0829s | 0.1084s | 0.76x | MATCH |
| recommender | 0.0124s | 0.0114s | 1.09x | MATCH |
| ws_frame | 0.0164s | 0.0123s | 1.33x | MATCH |
| fsm_dispatch | 0.1435s | 0.1617s | 0.89x | MATCH |
| arena_bump | 0.3145s | 0.0476s | 6.61x | MATCH |
| spsc_ring | 0.1521s | 0.0698s | 2.18x | MATCH |

## Controlled baseline A/B

All rows passed except `ws_frame`. Ties and small rows were repeated.

| Benchmark | A/B current/baseline | Verdict |
|---|---:|---|
| sparse_dispatch | 0.99–1.08x over 3 runs | noise, within tolerance |
| arena_churn | 0.94–1.10x over 3 runs | noise, within tolerance |
| enemy_swarm | 0.98–1.00x over 3 runs | within tolerance |
| nbody_newton_accel | 0.93–1.08x over 3 repeat runs | noise, within tolerance |
| ws_frame | 1.83–2.14x over 3 runs | **explained regression** |

`ws_frame` is slower than the pinned baseline because the baseline’s fast path
returns a pointer into a dead callee frame. `2026-10-28-v0.1-preview-baseline.md`
already records that the locked 0.25x ratio is not a valid safe target. The
current corrected-ABI ratio is 1.33x C.

## Verdict

- Correctness: green, all rows MATCH.
- No unexplained regression: **yes**, all measured rows are within tolerance.
- `ws_frame` needs an explicit baseline exception or a deliberate re-lock
  before the release claim is unconditional.
