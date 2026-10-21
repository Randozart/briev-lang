# Effect-shape follow-up plan

**Date:** 2026-10-21
**Status:** PLANNED — post-merge validation and structural follow-ups.
**Main merge:** `7fdb2b10` (selection policy + Local foundation).
**Full research report:** `docs/2026-10-21-effect-shape-full-research-report.md`.
**Research tag:** `effect-shape-research-2026-10-21`.

## Context

The primary `float_math`/`queue_drain` effect-shape regression is resolved:

- escaped effect memory for loops where it wins;
- loop-carried effect state (`Carry`) where it wins;
- stateless print for aggregate/temp-mediated loops;
- target-derived carry threshold and countdown-eligibility gate;
- full runtime suite all MATCH, library tests 3271, guards PASS.

Remaining items are validation, structural enablement, and cleanup. They are
ordered by risk/benefit; none should reopen the core selection policy unless a
measured regression is found.

## F1 — Controlled A/B for noisy rows (do first)

### Why

Full-suite runs show a few percent spread on rows the selector does not change
or barely changes:

- `telemetry_stream`
- `sweep_mid`
- `ring_buffer`
- `queue_drain_sym`

We have not proven whether any is a real merge regression.

### Procedure

1. Use the persistent baseline worktree at `../briev-compiler-baseline`.
2. Build both baseline and current `main` with the release harness.
3. Run baseline/current interleaved, same host, same `BOUND`, at least 5
   medians per row.
4. Prefer the project A/B utility where applicable:
   `bash benchmarks/compare_baseline.sh <name>`.
5. Inspect the selected effect shape for each row:
   `BRIEV_DIAG_LOOP=1` and the `effect state carried` / `effect shape escaped`
   info lines.
6. Record raw runs and medians in
   `benchmarks/results/2026-10-21-effect-shape-merge-ab.md`.

### Acceptance

- No confirmed >2% selector-caused regression.
- Any real regression gets an A/B-backed diagnosis before code changes.
- If a regression is caused by the selection heuristic, add a synthetic case
  and adjust the structural predicate/config, not a benchmark name.

### Result

DONE: `benchmarks/results/2026-10-21-effect-shape-merge-ab.md`.
No confirmed regression: telemetry +0.4%, sweep_mid +0.1%, ring −2.9%,
queue_sym −3.1% (current/baseline).

### Rollback

No code change expected. If a regression is found, revert only the selection
policy to the checkpoint:
`checkpoint-effect-carry-selection-2026-10-21`.

## F2 — Per-txn BatchShape map

### Why

`AnalysisResults.batch_shape` is one `Option<BatchShape>`.
`detect_batch_shape` returns `None` when multiple txns qualify. Mixed
direct-loop programs therefore lose countdown/carry and fall back to escaped.
`BatchShape.txn_name` already exists.

### Changes

1. Add `detect_batch_shapes()` returning `HashMap<String, BatchShape>`.
2. Keep `detect_batch_shape()` as a compatibility wrapper for tests/callers
   that require a unique shape.
3. Change `AnalysisResults.batch_shape` to a per-txn map (or add a map and
   migrate consumers in one commit).
4. Update LLVM dispatch to look up the batch by `node.name`.
5. Update `program_carry_mode`/effect selection to use per-txn batch
   eligibility.
6. Add a synthetic two-batch-loop fixture covering:
   - both loops carry-preferring;
   - one carry-preferring and one escaped-preferring;
   - mixed direct/temp loops.
7. Extend the guard to pin per-txn decisions and output multisets.

### Acceptance

- Multiple batch txns compile with the countdown engine where eligible.
- No mixed program loses a previously working shape.
- Full suite + guard green; output MATCH.
- No benchmark names or field prefixes.

### Result

DONE (`118eb20c`): `detect_batch_shapes()` returns a per-txn map;
`detect_batch_shape()` remains the unique-shape compatibility helper.
`AnalysisResults.batch_shapes` and countdown/carry dispatch use the entry for
the current txn. Added a multiple-batch unit test. Full lib **3272 green**;
timing guard PASS.

### Rollback

The current single-`Option` gate remains the fallback; checkpoint branches
preserve the pre-change state.

## F3 — Remove the dead batch emitter

### Why

`emit_countable_batched_main` is unused and known-broken:

- the original path produced dominance-invalid IR;
- a repaired version changed float output through fast-math reassociation.

Leaving it unused invites a future developer to wire a known-bad path.

### Changes

- Remove the dead emitter and its helper-only code, **or** keep it behind an
  explicit compile-time-unreachable test if any reference remains.
- Update the E2 preflight/results docs to point at the removal.
- Ensure no warning/dead-code regression.

### Result

DONE: removed `emit_countable_batched_main` and its stale source comments.
No live call site remains. Full lib/tests and guards must stay green; emitted
IR for the suite is unchanged (the function was unreachable).

### Acceptance

- `cargo test --lib` green.
- No change to emitted IR for any benchmark.
- `git grep` shows no live call site.

## F4 — Retire or formalize the E1 plain-context experiment

### Why

`BRIEV_DIAG_PLAIN_CONTEXT=1` remains default-off with
`@__briev_plain_ptr`. It recovered float to ~1.0x but regressed interval,
pid, and matrix. It is useful history, but it is hidden production surface.

### Options

1. **Remove from `main`** and leave it on the experiment/checkpoint branch.
2. Keep it, but add a clear diagnostic-only comment and guard; ensure it is
   covered by the guard's output check.

Prefer removal if no active experiment depends on it; the historical result is
already recorded in the full report and checkpoint docs.

### Acceptance

- Default compiler behavior unchanged.
- Documentation updated (report/experiment notes).
- No env-gated dead path remains on `main` unless explicitly marked
  experimental with a test.

## F5 — Long-term canonical Local/effect model (separate effort)

Not part of the immediate follow-up merge. Requires design decisions:

- explicit effect context vs loop-carried effect state;
- per-loop vs program-level effect storage;
- FP evaluation-order policy under `fast` math;
- eventual retirement of the carry/escaped selection to diagnostics.

Direction doc: `docs/plans/2026-10-21-canonical-local-effect-model.md`.

## Execution order

1. F1 — controlled A/B, record results.
2. F2 — per-txn `BatchShape`.
3. F3 — dead emitter cleanup.
4. F4 — E1 retirement/formalization.
5. Re-run full suite, guards, and update the research report.
6. Only then consider F5 design work.

## Verification checklist (every slice)

- `cargo test --lib`
- `bash benchmarks/effect_shape_guard.sh --quick`
- `bash benchmarks/effect_shape_guard.sh --timing` for selection changes
- full `bash benchmarks/build_and_bench.sh --runtime` for codegen/default
  changes
- output parity at print boundaries
- commit log updated in this plan
- checkpoint/rollback branch remains valid

## Commit log

| Date | Commit | Change |
|---|---|---|
| 2026-10-21 | `7fdb2b10` | main merge: selection + Local foundation |
| 2026-10-21 | `5c7e559b` | full research report |
| 2026-10-21 | `dd007598` | F1 controlled A/B (no confirmed regression) |
| 2026-10-21 | `118eb20c` | F2 per-txn BatchShape |
| 2026-10-21 | _pending_ | F3 dead batch emitter cleanup |
| 2026-10-21 | _pending_ | F4 E1 retirement/formalization |
