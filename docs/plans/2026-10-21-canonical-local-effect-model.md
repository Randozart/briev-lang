# Canonical Local state + explicit effect model

**Date:** 2026-10-21
**Branch:** `feat/local-effect-canonical`
**Status:** ACTIVE — long-term direction; first correctness slices landed.
**Checkpoint to return to:** branch `experiment/effect-state-carry`, tag
`checkpoint-effect-state-carry-2026-10-21`, commit `3c1611d2` (code) /
`67973adc` (checkpoint doc).

## Why

The carry-vs-escaped selector is a **diagnostic bridge**, not the architecture:

- it steers LLVM's register allocation through loop-carried liveness;
- it is fragile across LLVM versions/targets;
- it trades `sweep_sparse`/`float_math_nonzero`/`kalman` wins against small
  regressions in `float_math`/telemetry/accumulator/sweep_mid.

The durable model is:

> **Loop-internal state is local SSA. Effect state is an explicit context.
> Machine `%State` is committed only at real observable boundaries.**

Then there is no state/effect shape toggle for the compiler to get wrong.

## Local-level rule

For a field written only inside one bounded node and not observable outside it:

- keep it as an SSA value / local phi;
- do not load/store it through `%State`;
- do not carry it through a stateful print ABI;
- commit it to `%State` only at a real boundary (post-loop, contract, export,
  rollback, cross-tick, async).

Effect calls receive scalar values or a **distinct effect context**. That
context may carry the stdout buffer/fill, but it must be `noalias`/`nocapture`
with respect to the loop's local SSA and must not be the machine `%State`.

This is the C shape: locals + explicit effect boundary, not a state-escape
trick.

## Phases

### Phase A — correct derivation (started here)

- Complete Local classification across every TopLevel surface.
  - Current slice: use `ast::visit::walk_toplevel` for all external items,
    including `Export.inner`; the previous hand-written match skipped exports,
    cells, trigger bindings, when-laws, assertions, fuzzed items, type/operator
    bodies, and compile-time items.
  - Add a regression test: an exported defn that reads `f` keeps `f` machine
    state, not Local.
- Continue with explicit `StorageClass` / `MaterializationPlan` in
  `AnalysisResults` (Local / Plain / Machine / Volatile).

### Phase B — canonical local loop

- Emit Local fields as explicit SSA phis; no `%State` traffic in the hot loop.
- Keep the periodic guard and FP evaluation order.
- No program-level plain demotion; no carry/escape mode.
- Commit Local values to `%State` only where an observation requires it.

### Phase C — explicit effect ABI

- Print/effect functions take a distinct effect context (or scalar args).
- Prove `noalias`/`nocapture` against loop locals; one stdout buffer.
- If the context alone reintroduces the interval/matrix regressions, refine the
  effect summary/alias facts, not the state shape.

### Phase D — target model

Use `config/targets.dbvl` only for genuine machine facts (register classes,
vector widths, ABI). No benchmark-derived carry/escape rules.

### Phase E — retire the bridge

Once canonical Local/effect matches the suite, retire the carry/escaped
selector to diagnostics. Keep the checkpoint branch available until then.

## Acceptance

- `float_math` at/below parity, no per-iteration accumulator reload.
- `queue_drain`, `interval_step`, `matrix_pipeline` keep their wins.
- `sweep_sparse`, `float_math_nonzero`, `kalman` keep their gains.
- Mixed program: both loop kinds use Local SSA; one effect context; output
  correct.
- Full suite MATCH; `cargo test --lib`; quick/timing guards green.
- No benchmark names, field prefixes, or vocabulary rules.

## Commit log

| Date | Commit | Change |
|---|---|---|
| 2026-10-21 | `3c1611d2` | effect-state-carry experiment (checkpoint branch) |
| 2026-10-21 | `67973adc` | checkpoint doc + tag `checkpoint-effect-state-carry-2026-10-21` |
| 2026-10-21 | `6c8ba811` | canonical direction doc; generic TopLevel Local walker; `Export.inner` descent; exported-field regression test |
| 2026-10-21 | `486b5b48` | pin canonical branch commit log |
| 2026-10-21 | _pending_ | `LoopMaterializationPlan` in `AnalysisResults`; backend consumes plan for `fun.local_fields`; plan tests for Local/Machine |
| 2026-10-21 | `main` / `a7fe6c3d` | branch point for canonical work (carry absent) |

Update this table for every slice so the path from checkpoint to canonical
model is auditable.
