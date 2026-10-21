# Per-loop effect memory / mixed-program specialization

**Date:** 2026-10-21
**Status:** PLAN — E1 experiment run and recorded; E2 next.
**Related:** `docs/architecture/effect-shape-selection.md`,
`docs/plans/2026-10-21-target-pressure-effect-shape.md`,
`docs/plans/2026-10-20-machine-backed-state-and-loop-emission.md`.

## Problem

The current selector is **program-level**. If any transaction needs the escaped
effect shape, inferred plain fields (the stdout buffer/fill) are demoted to
machine state for the whole program. A mixed program therefore uses one shape
for all loops:

- direct-Float loop wants escaped (0.9x);
- temp-mediated / aggregate loop wants stateless (0.97–0.5x);
- one `%State` alloca and one stdout stream make a naive per-loop ABI split
  dangerous (two buffers / wrong ordering).

Goal: each loop gets the optimal memory shape while stdout remains one
ordered stream and all output stays correct.

## Hard constraints

- One stdout buffer and one ordering domain.
- `%State` is one alloca; any escape through it is program-wide.
- No benchmark names, field-name prefixes, or user strategy keyword.
- The selector remains structural and target-gated.
- Mixed output must match irrespective of the chosen shape.

## Experiment E1 — stack plain context (RUN 2026-10-21)

Implemented behind `BRIEV_DIAG_PLAIN_CONTEXT=1`:

- `%PlainState` aggregate allocated in `main`;
- plain-field loads/stores go through `%plain` (function parameter where
  available, otherwise a pointer published in `@__briev_plain_ptr`);
- `ptr %plain` added to defn signatures via the existing
  `compute_defn_needs_plain` / `defn_takes_plain` analysis;
- print intrinsic/cast/ExtCall call sites pass the plain pointer;
- machine `%State` no longer escapes through print;
- program-level escaped demotion is bypassed for the experiment.

Measurements (BOUND=50M, 3–5 interleaved runs, same host; C reference in the
same run):

| Benchmark | E1 plain context | C | E1 ratio | Current selected ratio |
|---|---:|---:|---:|---:|
| float_math | 0.0479s | 0.0477s | **1.00x** | 0.89–0.96x |
| queue_drain | 0.0337s | 0.0603s | **0.56x** | 0.50–0.53x |
| interval_step | 0.0899s | 0.0633s | **1.42x** | 0.71–0.75x |
| telemetry_stream | 0.1572s | 0.1576s | **1.00x** | 0.96–0.98x |
| pid_control | 0.4276s | 0.3478s | **1.23x** | 0.99–1.01x |
| matrix_pipeline | 0.5181s | 0.4927s | **1.05x** | 0.96–0.97x |
| mixed fixture | 0.2282s | — | — | 0.201–0.204s (current escaped) |

Result:

- E1 **does recover float_math** from ~2.9x to ~1.0x by keeping `%State`
  unescaped, but it is still worse than the current escaped shape (0.9x).
- It **regresses interval_step** (1.42x vs 0.71x), pid (1.23x vs 1.0x), and
  matrix (1.05x vs 0.97x) because the context is program-wide and the
  pointer-based plain accesses block the stateless SROA/vector shape.
- Mixed remains program-wide: E1 cannot give the direct loop and the
  temp-mediated loop different shapes.
- The txn/cold path still publishes/loads `@__briev_plain_ptr` because not
  every txn carries a plain parameter; this is a further aliasing/optimization
  handicap.

E1 is therefore **insufficient as the mixed-program solution**. It is retained
only as a diagnostic experiment (`BRIEV_DIAG_PLAIN_CONTEXT=1`), default off.

## E2 — per-loop state materialization (next)

Keep **one stateless print ABI** and make the loop storage per-transaction.

1. Add a per-txn `LoopStatePlan` to `AnalysisResults`:
   - `StatelessSroa` for temp-mediated/aggregate loops;
   - `DirectScalarMaterialized` for direct-Float loops.
2. `DirectScalarMaterialized` emission:
   - keep the accumulator in SSA/register;
   - materialize the bulk carried fields in the loop’s state context;
   - use a single header/body/latch with the effect block rejoining once;
   - no `%State` escape through print.
3. The stateless loop keeps the current SROA/vector shape; print remains
   stateless for both.
4. Mixed program: each txn is emitted independently; no program-level
   plain demotion.
5. Remove/retire the program-level `plain_lets.retain` path once E2 holds the
   anchors.

Why E2: E1 proves a separate effect context can recover the register shape,
but a program-wide context loses the stateless loop. E2 moves the
materialization inside the loop, so it can differ per loop without splitting
stdout.

Acceptance:
- mixed: `direct` selected materialized, `temp` stateless; sorted output and
  program output unchanged;
- float_math ≤1.0x (target 0.9);
- queue_drain ≤0.55x; interval ≤0.75x; matrix ≤1.0x; telemetry/pid ≤1.02x;
- asm: accumulator in `xmm`, no per-iteration stack reload;
- full suite and guards green.

## E3 — dual effect ABI (only if E2 fails)

Generate `print_*$stateless` and `print_*$escaped` variants, both writing one
shared stdout context. This is the last resort:

- one buffer, one ordering domain;
- escaped variant must access escaped loop state without forcing the
  stateless variant through `%State`;
- ABI/export/liveness and mixed-order tests required.

If one shared buffer cannot be maintained, reject E3 rather than split stdout.

## Guard / tests

- `benchmarks/effect_shape_guard.sh --quick` remains the default pin; extend
  its mixed fixture to assert the **per-loop** decisions once E2 exposes them.
- `test_effect_shape_mixed_program_shape_facts` is the unit-level pin.
- `cargo test --lib`, tier-1 daily smoke, runtime suite.
- Record E1/E2 numbers in
  `benchmarks/results/2026-10-21-target-pressure-effect-shape.md`.

## Rollback / status

The E1 code is env-gated; default compiler behavior is unchanged, and the
quick guard/timing guard still pass. It is not a production selector.
E2 is the next implementation; if E2 cannot hit the anchors, revisit E3 or
keep the program-level selector with the documented mixed-program limitation.
