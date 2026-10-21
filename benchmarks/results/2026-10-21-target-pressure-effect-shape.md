# Target-pressure effect-shape selection — results

**Date:** 2026-10-21
**Host:** same machine for all rows; harness
`bash benchmarks/build_and_bench.sh --runtime`; `BOUND=50000000`.
**Plan:** `docs/plans/2026-10-21-target-pressure-effect-shape.md`.

## What was tested

The compiler now selects between:

- **StatelessEffect** — print path uses bare `@stdout_*` globals; `%State`
  stays unescaped.
- **EscapedEffect** — inferred plain fields are demoted to machine state, so
  the periodic effect path uses the escaped `%State` alloca.

The selector is structural and target-gated, not benchmark-specific:

```
aggregate carried                -> stateless
no carried Float                 -> stateless
all carried Floats temp-mediated -> stateless   (f = n; after let n = ...)
otherwise                        -> escaped
```

`fp_reserved_temps` is a target config capability gate (x86_64 = 4); it is not
a field-count crossover.

## Falsified hypothesis: field-count pressure

Synthetic loops with N Float accumulators, periodic `println!`, both shapes,
BOUND=50M, 3 interleaved runs each:

| N | stateless | escaped | escaped/stateless |
|---:|---:|---:|---:|
| 1 | 0.1294s | 0.0470s | 0.36 |
| 2 | 0.1243s | 0.0433s | 0.35 |
| 4 | 0.1272s | 0.0487s | 0.38 |
| 8 | 0.1285s | 0.0477s | 0.37 |
| 10 | 0.1233s | 0.0459s | 0.37 |
| 12 | 0.1233s | 0.0461s | 0.37 |
| 16 | 0.1294s | 0.0471s | 0.36 |
| 20 | 0.1282s | 0.0476s | 0.37 |

Escaped wins at **every** N. There is no field-count knee at 12 (or anywhere
in the sweep). Int-only synthetic loops N=1/5/12/20 were neutral (escaped /
stateless 1.00–1.01). Queue's aggregate shape was worse escaped (1.13× of
stateless).

The pressure model was therefore narrowed to the target capability gate; the
causal shape feature is direct field arithmetic vs old-value temp capture.

## Real benchmark A/B and final ratios

Final ratios are harness rows from the full runtime run; the stateless/escaped
columns are controlled A/B measurements.

| Benchmark | Stateless | Escaped | Selected | Final | Prior |
|---|---:|---:|---|---:|---:|
| float_math | 2.72–2.89x | ~0.92x | escaped | **0.89–0.96x** | 2.74x |
| float_math_nonzero | — | — | escaped | **1.21–1.24x** | 1.48x |
| telemetry_stream | 1.48x | 0.67× stateless | escaped | **0.96–0.98x** | 1.48x |
| pid_control | 1.22x | 0.81× stateless | escaped | **0.99–1.01x** | 1.22x |
| matrix_pipeline | 0.97x | 1.04x | stateless | **0.96–0.97x** | 0.97x |
| sweep_sparse | 1.46x | 1.32x | escaped | **1.31–1.33x** | 1.49x |
| sweep_mid | 1.38x | 1.40x | escaped | **1.33–1.38x** | 1.36x |
| sweep_dense | 1.04x | 1.05x | stateless | **1.04–1.06x** | 1.05x |
| queue_drain | 0.50–0.53x | 1.13× stateless | stateless | **0.50–0.53x** | 0.47x |
| interval_step | 0.71–0.75x | ≈1.01× stateless | stateless | **0.71–0.75x** | 0.72x |
| accumulator_flush | — | — | escaped | **0.90–0.95x** | 0.93x |
| nbody_newton | 1.05–1.08x | 1.04–1.08x | escaped | **1.04–1.08x** | 0.95x (older run) |

Full runtime suite completed with **all rows MATCH**. `float_math` and the
float-heavy scalar loops are the headline recovery; matrix/sweep dense keep
the stateless SROA/vector shape.

## Regressions / residuals to discuss

These are **not** attributed to the selector by controlled A/B:

- `queue_drain`: 0.50–0.53x versus the historical 0.45–0.49x best. The
  selector does not trigger (aggregate gate), so this is wall-clock variance
  in the C reference (`0.059–0.061s` here versus lower earlier) rather than a
  codegen change. The Briev side is stable at ~0.031s.
- `nbody_newton`: 1.04–1.08x here versus 0.95x in an older fixture. Forced
  stateless vs escaped are within 0.3% on the current tree, so the selector is
  not the cause; the C reference moved from ~7.66s to ~6.6s.
- `ws_frame`, `fsm_dispatch`, `arena_bump`, `spsc_ring`: no selector trigger;
  their ratios move with run-to-run wall-clock variance and remain at their
  known fixture levels (1.5–1.6x, ~2.8–3.1x, ~4.3–4.5x, ~1.8–1.9x).
- `deep_recursion`: 0.0003–0.0005s timings; ratio is timer noise.
- `float_math`: 0.89–0.96x, slightly above the historical 0.90–0.94x best.
  The hot loop now keeps the accumulator in `xmm0` with no per-iteration
  `vmovss` reload; the remainder is the countdown loop shape, not the effect
  memory.

## Mixed-program coverage

`tests/tier1/mixed_effect_shape.bv` places a direct-Float loop and a
temp-mediated Float loop in one `sync<mixed>` program. The program-level
selector chooses escaped (the direct loop needs it); the quick guard compiles
the fixture auto and forced-stateless and compares sorted output. This pins
both correctness and the current program-level policy while per-loop ABI
specialization remains future work.

## Regression guard

`benchmarks/effect_shape_guard.sh --quick` asserts the expected shape for 12
representative programs and the mixed-program output multiset. CI runs it
after the release build. `--timing` adds catastrophic-regression ceilings for
the primary cases. The expected-decision table and diagnosis guide live in
`docs/architecture/effect-shape-selection.md`.

## Mixed-program / per-loop E1 experiment (2026-10-21)

The stack plain-context experiment (`BRIEV_DIAG_PLAIN_CONTEXT=1`, `%PlainState`
+ `ptr %plain` ABI) was run on the same host/BOUND. It keeps machine `%State`
unescaped by the print path.

| Benchmark | E1 | C | E1 ratio | Current selected |
|---|---:|---:|---:|---:|
| float_math | 0.0479s | 0.0477s | 1.00x | 0.89–0.96x |
| queue_drain | 0.0337s | 0.0603s | 0.56x | 0.50–0.53x |
| interval_step | 0.0899s | 0.0633s | 1.42x | 0.71–0.75x |
| telemetry_stream | 0.1572s | 0.1576s | 1.00x | 0.96–0.98x |
| pid_control | 0.4276s | 0.3478s | 1.23x | 0.99–1.01x |
| matrix_pipeline | 0.5181s | 0.4927s | 1.05x | 0.96–0.97x |
| mixed fixture | 0.2282s | — | — | 0.201–0.204s |

E1 recovers float from ~2.9x to ~1.0x, but loses interval/pid/matrix and does
not specialize per loop. It is retained only behind
`BRIEV_DIAG_PLAIN_CONTEXT=1`. Next: E2 per-loop state materialization
(`docs/plans/2026-10-21-per-loop-effect-abi.md`).

## E2 preflight — alternate stateless loop engines (2026-10-21)

Forcing the existing version-DAG or per-field stateless emitters
(`BRIEV_DIAG_LOOP_ENGINE=version|perfield`, `BRIEV_DIAG_EFFECT=stateless`):

| Benchmark | version | per-field | stateless countdown |
|---|---:|---:|---:|
| float_math | 0.1284s | 0.1241s | ~0.128s |
| interval_step | 0.0407s | 0.0461s | ~0.041s |
| matrix_pipeline | 0.512s | 0.519s | ~0.48s |

No existing stateless loop engine reaches the 0.9x escaped shape on
`float_math`. E2 needs a new materialized loop-state emitter or a per-loop
effect context; engine selection alone is insufficient.

## E2 method attempt — batched pure inner loop (REJECTED 2026-10-21)

Repairing the dead batch emitter to store batch state before the guard removed
the dominance errors but changed float output:

| BOUND | Batch output | C / current escaped |
|---:|---:|---:|
| 5,000,000 | 1,497,752 | 1,449,352.5 |
| 10,000,000 | 2,992,644.5 | 2,699,352.5 |

The pure inner loop lets LLVM reassociate/vectorize the float32 accumulations
under `fast` math. The local probe independently printed 1,434,824.38 at 5M,
confirming shape-sensitive FP results. Output parity is mandatory, so the
batch shape was removed from dispatch and left unused. E2 must preserve the
guard-bearing FP evaluation order (or reproduce C's exact shape).

## Acceptance

- [x] `float_math` recovered from ~2.9x to ≤1.0x.
- [x] `queue_drain` remains a Briev win (~0.5x).
- [x] `interval_step` remains ~0.71–0.75x.
- [x] telemetry_stream / pid_control improved to C parity.
- [x] matrix_pipeline keeps its stateless win.
- [x] all correctness rows MATCH.
- [ ] Per-loop ABI specialization for mixed-shape programs (future).
- [ ] Single-latch SSA loop to close the remaining float_math gap (future).
