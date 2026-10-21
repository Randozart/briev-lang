# Effect-shape selection

**Date:** 2026-10-21
**Status:** implemented, guarded.
**Plan:** `docs/plans/2026-10-21-target-pressure-effect-shape.md`
**Results:** `benchmarks/results/2026-10-21-target-pressure-effect-shape.md`

## Why this exists

The same periodic-print loop has two legal memory shapes with very different
register-allocation behaviour:

| Shape | Effect path memory | Result |
|---|---|---|
| **StatelessEffect** | `@stdout_*` globals | SROA/vectorization of the loop state is possible; queue/matrix wins |
| **EscapedEffect** | `%State` alloca (inferred plain fields demoted to machine state) | state alloca escapes through the effect; direct Float accumulators stay in registers |

Neither shape is universally better. The compiler chooses from loop structure,
not benchmark identity.

## Decision

For each transaction's `LoopShape`:

```
if carried_aggregate_count != 0
    -> StatelessEffect
else if carried_float_count == 0
    -> StatelessEffect
else if temp_mediated_float_count == carried_float_count
    -> StatelessEffect
else
    -> EscapedEffect
```

`temp_mediated_float_count` counts carried Float fields assigned from a
body-local let after that let was computed (`f = n;` following
`let n = ...;`). A fully temp-mediated body is the old-value/vectorizable
shape (`matrix_pipeline`, `sweep_dense`) and keeps stateless SROA. Direct
field arithmetic (`f = f * k + ...`) takes the escaped context
(`float_math`, `telemetry_stream`, `pid_control`, `sweep_sparse`).

Target capability gate:

```
usable_fp = float_registers.saturating_sub(fp_reserved_temps)
if usable_fp == 0 or target has virtual registers (wasm) -> StatelessEffect
```

`float_registers` and `fp_reserved_temps` are per-target config in
`config/targets.dbvl` (fields 0 and 5). This is not a field-count threshold:
the N=1..20 sweep showed escaped wins at every N. The reserve only decides
whether the target has a physical FP spill shape to escape.

The decision is **program-level** in this slice: if any transaction selects
escaped, inferred plain fields (stdout buffer/fill) are demoted to machine
state before the storage-class ABI is computed. Explicit `plain` assertions
remain plain. A mixed program with both kinds of loop therefore uses escaped
for both; the guard fixture verifies both shapes still produce the same output
multiset. Per-loop effect ABI specialization is the planned refinement.

## Where it lives

- `src/analysis/loop_shape.rs`
  - `LoopShape.carried_float_count`, `carried_scalar_count`,
    `carried_aggregate_count`, `temp_mediated_float_count`
  - `escaped_effect_cause(...)`
  - type/pressure and temp-mediated helpers
- `src/config_tuning.rs`
  - `TargetSettings.float_registers`, `TargetSettings.fp_reserved_temps`
- `config/targets.dbvl`
  - field 0 = `float_registers`, field 5 = `fp_reserved_temps`
- `src/backend/llvm/mod.rs`
  - selector runs before `plain_lets` / needs-state ABI computation;
  - demotes only inferred plain fields;
  - emits `info: effect shape escaped for txn '…'`.

## Expected decisions (regression pin)

| Program | Expected | Why |
|---|---|---|
| float_math | escaped | direct 12-Float arithmetic |
| float_math_nonzero | escaped | direct Float arithmetic |
| telemetry_stream | escaped | direct EMA/m2 updates |
| pid_control | escaped | direct PID updates |
| accumulator_flush | escaped | direct Float accumulation |
| sweep_sparse | escaped | direct 4-field update |
| sweep_mid | escaped | direct 8-field update |
| queue_drain | stateless | aggregate RingBuffer |
| interval_step | stateless | Int-only |
| matrix_pipeline | stateless | fully temp-mediated 16 fields |
| sweep_dense | stateless | fully temp-mediated 16 fields |
| mixed_effect_shape | escaped | contains `direct`; `temp` also present |

`tests/tier1/mixed_effect_shape.bv` contains both loop kinds under one
`sync<mixed>` group. The unit test
`test_effect_shape_mixed_program_shape_facts` pins the shape facts, and
`benchmarks/effect_shape_guard.sh` compiles it with auto and forced-stateless
selection and compares sorted output.

## Regression guard

Default (CI-safe, structural + mixed correctness):

```bash
bash benchmarks/effect_shape_guard.sh --quick
```

Timing ceilings (local; catches catastrophic regressions only):

```bash
bash benchmarks/effect_shape_guard.sh --timing
```

The CI workflow runs the quick guard after building the release compiler.

### Diagnosing a guard failure

1. **Shape flipped**: run `BRIEV_DIAG_LOOP=1 brievc build <bench> --out /tmp/x`.
   Inspect `floats=`, `aggregates=`, `vector_groups=` and, for the relevant
   txn, the `temp_mediated` fact. The selector is structural; a wrong flip
   means the shape facts changed, not the threshold.
2. **Ratio jumped but shape is right**: inspect the hot loop asm. Escaped-good
   loops must keep the accumulator in `xmm` with no per-iteration
   `vmovss` reload from the stack. Stateless-good loops must not force
   `%State` through the print path.
3. **`queue_drain`/`interval_step` regressed**: check that no selector info
   line appears for them. If it does, the aggregate/Int gate broke.
4. **Mixed output mismatch**: the guard compares sorted output; a failure is a
   real shape-dependent semantic divergence, not print ordering.

Temporary experiment knobs:

```bash
BRIEV_DIAG_EFFECT=stateless brievc build <bench> --out /tmp/stateless
BRIEV_DIAG_EFFECT=escaped   brievc build <bench> --out /tmp/escaped
```

These force the shape for A/B only; unset uses the selector.

## Known limitations / next steps

- Program-level plain demotion can over-apply to a mixed program. Per-loop
  effect ABI specialization is the next structural step; the mixed fixture and
  guard already exist to pin the semantics during that change.
- `temp_mediated_float_count` currently scans top-level body statements. A
  future nested-let form should be added if a benchmark exercises it.
- The loop emitter is unchanged. The remaining float_math gap is loop shape
  (single-latch SSA), not effect memory.
