# Target-pressure effect-shape selection

**Date:** 2026-10-21
**Status:** IMPLEMENTED — first slice landed; full-suite A/B recorded.
**Related:** `docs/plans/2026-10-20-machine-backed-state-and-loop-emission.md`,
`benchmarks/results/2026-10-21-target-pressure-effect-shape.md`,
`benchmarks/results/2026-10-20-performance-regression-bisect.md`.

## Goal

Recover both observed wins in one binary without benchmark names, field
prefixes, vocabulary recognition, or a user-facing strategy keyword:

- `float_math` ~0.9x (escaped effect context)
- `queue_drain` ~0.5x (stateless print, unescaped `%State`)
- `interval_step` / `telemetry_stream` / `pid_control` retained or improved.

## Evidence

The endpoints came from opposite memory shapes:

| Shape | float_math | queue_drain | interval_step |
|---|---:|---:|---:|
| stateless print | 2.72–2.89x | **0.45–0.49x** | **0.61x** |
| escaped/stateful print | **0.92–0.94x** | 1.44x | 1.46x |
| local `foreach` | 1.30x | — | — |

No historical run had both. The compiler previously applied one storage choice
program-wide.

## Rejected hypothesis: field-count pressure crossover

The initial model was:

```
K_target = float_registers - fp_reserved_temps
high_fp  = carried_float_count >= K_target
escaped  = high_fp && carried_aggregate_count == 0
```

The N=1..20 Float sweep **falsified** the count threshold: the escaped shape
was faster at every N (escaped/stateless ≈ 0.33–0.38), including N=1. The
crossover is not a field-count knee. `float_registers - fp_reserved_temps` is
therefore retained only as a **target capability gate**:

- no usable FP register budget → no escaped shape;
- virtual-register target (wasm) → stateless ABI.

`fp_reserved_temps` stays config (`config/targets.dbvl`, field 5; x86_64 = 4)
so a target with a different ABI reserve can disable the escaped shape without
a code change.

## Implemented decision (2026-10-21)

Derived per transaction, no benchmark identity:

```
if carried aggregate/memory-resident field
    -> Stateless
else if no carried Float
    -> Stateless
else if every carried Float is assigned from a body-local let
    -> Stateless        // old-value capture: SROA/vector shape wins
else
    -> Escaped          // direct field arithmetic: effect-context escape wins
```

The third arm is structural: `matrix_pipeline` and `sweep_dense` compute every
next value into a `let n…` before copying into fields (`f = n;`), which keeps
the old-value/vectorizable shape and prefers stateless SROA. `float_math`,
`telemetry_stream`, `pid_control`, and `sweep_sparse` do direct field
arithmetic and prefer the escaped context. Aggregate-carried loops
(`queue_drain`, `interval_step`) stay stateless.

The decision is program-level in this slice: if any transaction selects
escaped, inferred plain fields (stdout buffer/fill) are demoted to machine
state before the storage-class ABI is computed. Explicit `plain` assertions
are preserved. A per-loop ABI is the natural follow-up, not required for the
current benchmark set.

## Implementation

- `TargetSettings.fp_reserved_temps` + every `target.*` row, parsing, defaults,
  test.
- `LoopShape.carried_float_count`, `carried_scalar_count`,
  `carried_aggregate_count`, `temp_mediated_float_count`.
- `loop_shape::escaped_effect_cause(shapes, float_registers,
  fp_reserved_temps)`.
- `BRIEV_DIAG_EFFECT=stateless|escaped` for A/B experiments (auto when unset).
- `BRIEV_DIAG_LOOP=1` prints pressure, aggregate, vector-group facts.
- LLVM backend computes the selector before `plain_lets` / needs-state ABI;
  demotes inferred plain fields only; emits an `info:` line with the cause.

## Mixed-program coverage

`tests/tier1/mixed_effect_shape.bv` contains two sync-group loops in one
program: `direct` (direct Float arithmetic -> escaped) and `temp` (old-value
lets -> stateless-preferred). The program-level selector chooses escaped
because `direct` is present. Coverage is pinned three ways:

- `test_effect_shape_mixed_program_shape_facts` asserts the per-loop shape
  facts and the program-level cause;
- `benchmarks/effect_shape_guard.sh --quick` compiles the fixture auto and
  forced-stateless and compares sorted output multisets;
- the tier-1 daily-use smoke test compiles and runs the fixture.

Per-loop effect ABI specialization remains the follow-up; the mixed fixture is
the semantic pin for that change.

## Regression guard

`benchmarks/effect_shape_guard.sh` is the standing guard:

- default/`--quick` (CI): expected shape for 12 representative programs,
  plus the mixed-program auto-vs-stateless output check;
- `--timing` (local): wide catastrophic ceilings on the primary cases
  (`float_math` ≤1.25x, telemetry ≤1.25x, pid ≤1.20x, matrix ≤1.15x,
  queue ≤0.80x, interval ≤0.90x).

CI runs the quick guard after the release compiler build. A shape flip is
diagnosed with `BRIEV_DIAG_LOOP=1`; A/B with `BRIEV_DIAG_EFFECT=...`.
Architecture and diagnosis guide:
`docs/architecture/effect-shape-selection.md`.

## Experiment record

**Synthetic N=1..20 Float loops** (same periodic print, stateless vs escaped,
BOUND=50M): escaped won every N; no pressure knee. Int-only loops N=1/5/12/20
were neutral (≈1.00); aggregate queue was worse escaped (≈1.13).

**Real A/B** (same host, harness, BOUND=50M):

| Benchmark | Stateless | Escaped | Selected | Final ratio |
|---|---:|---:|---|---:|
| float_math | 2.9x | ~0.92x | escaped | **0.89–0.96x** |
| telemetry_stream | 1.48x | ~0.67 of stateless | escaped | **0.96–0.98x** |
| pid_control | 1.22x | ~0.81 of stateless | escaped | **0.99–1.01x** |
| matrix_pipeline | 0.97x | 1.04x | stateless | **0.96–0.97x** |
| sweep_sparse | 1.46x | 1.32x | escaped | **1.31–1.33x** |
| sweep_mid | 1.38x | 1.40x | escaped | **1.33–1.38x** |
| queue_drain | 0.50–0.53x | 1.13 of stateless | stateless | **0.50–0.53x** |
| interval_step | 0.71–0.75x | ≈1.01 of stateless | stateless | **0.71–0.75x** |

Full runtime suite: all correctness MATCH. No benchmark shows a confirmed
selector-caused regression; the residual variance is dominated by small
wall-clock differences (`ws_frame`, `fsm_dispatch`, `nbody_newton`) and by
queue_drain's 0.50–0.53 range versus its historical 0.45–0.49 best.

## Decision gates / follow-ups

- If a future loop is fully temp-mediated but escaped is still faster, the
  structural arm is too broad; add a measured fact (e.g. vectorizable group)
  rather than a benchmark name.
- If a program mixes a temp-mediated loop and a direct-float loop, the
  program-level plain demotion may over-apply; move to per-loop plain/effect
  ABI specialization.
- The loop emitter is unchanged; the single-latch SSA loop remains the next
  performance lever if float_math 0.9 is not enough.
- `fp_reserved_temps` is the only configurable number; no benchmark/threshold
  branches may enter the selector.
