# Plan 2026-09-24: multi-state law solving — bistable and unpop

**Status:** approved / building
**Branch:** `feat/e14a-intent-synthesis`
**Builds on:** component laws Slices 2–6 and ASCII unit policy.

## Problem

The DC solver already enumerates guarded branch modes, but refuses more than
one valid mode. It also rejects law-bearing `unpop` parts instead of solving
their present and absent configurations. Both limitations are the same
missing concept: a board may have multiple **operating states**, and every
contract must be proved over every state.

This plan does **not** let the compiler choose a convenient state. A state is
a solution only when its equations and guards are consistent. Every emitted
proof and every violation names the state that produced it.

## State model

An operating state is:

```text
participation state × branch modes of each connected law group
```

- **Participation state**
  - `present`: every declared component contributes its laws.
  - `absent`: every component marked `unpop` contributes no laws; its pins
    are open.
  - If no law-bearing component is `unpop`, only `present` is solved.
- **Branch mode**
  - Within a connected law group, each guarded law is active or inactive.
  - A candidate mode is solved, then accepted only if every guard has the
    truth value selected by the mode.
- **Global state**
  - Independent connected groups may each have valid modes.
  - Global states are the deterministic Cartesian product of group candidates.
  - Product size is bounded. Crossing the bound is a hard diagnostic, not a
    truncated search.
- **State label**
  - Every solution carries a deterministic label such as
    `participation=present; mode=01; mode=10`.
  - Labels enter proofs, violations, and budget errors.

## Bistable semantics

`spec Bistable: true;` is an authority declaration, not a hint that lets the
compiler pick one state.

Rules:

1. The parser stores `Bistable` like other boolean component specs.
2. A connected law group may keep multiple candidates only when **every**
   law-bearing component in that group declares `spec Bistable: true;`.
3. If any component in the ambiguous group is not declared bistable, multiple
   valid modes remain a hard error.
4. Zero valid modes is still a hard error.
5. For an acknowledged bistable group, every valid state is retained.
6. Contracts, power ratings, and budgets must hold in every retained state.
   A bound satisfied in one state and violated in another is violated.

## Unpop semantics

For a law-bearing component marked `unpop`:

1. **Present state:** its component laws participate normally.
2. **Absent state:** its laws and KCL contributions are removed; its pins are
   open.
3. Both states are solved with the same contract voltage boundaries.
4. Tolerance checks are re-run on solved voltages in each state.
5. Current bounds, law power, and budgets are checked per state.
6. Absent-state omission of a component is not proof that its present state
   is safe; the present state is mandatory.

## Solver surface

- `DcSolution` becomes a labeled global operating state.
- `solve_dc_laws` returns all valid global states.
- Group candidates are combined deterministically.
- `DcContext` receives type info so it can inspect `Bistable`.
- State caps:
  - per connected group: existing 12 guarded-law budget;
  - whole board: 64 operating states.
- Crossing either cap is a hard diagnostic naming the count and fix.

## Proof integration

`post_solve_checks` becomes state-aware:

1. Classify contract drives as today.
2. Solve all operating states.
3. For each state:
   - clone the drive/tolerance base;
   - merge that state’s solved voltages/currents;
   - run tolerance, legacy current derivation where applicable, law power,
     current bounds, and budgets;
   - prefix proofs/diagnostics with the state label.
4. Aggregate all state violations and proofs into the board check.
5. Keep the first deterministic state as the emitter’s representative
   voltage view. Schematic labels are representative; correctness is proved
   across all states.

## Tests

1. **Ambiguity remains an error**
   - A two-mode current-source law without `Bistable` errors naming the state
     count.
2. **Acknowledged bistability retains both states**
   - The same law with `spec Bistable: true;` solves two states.
   - A bound satisfied by both states is proved with state provenance.
   - A bound violated by one state is a hard violation naming that state.
3. **Unpop law solves both states**
   - Present state solves the law operating point.
   - Absent state removes the law contribution.
   - No unsupported-dual-state diagnostic remains.
4. **All-state bound enforcement**
   - A bound passing in the absent state but failing in the present state is
     rejected.
5. **Existing boards**
   - Non-unpop, single-state boards retain their current behavior.
   - `led_blinker.ebv` and `usb_sensor.ebv` continue to emit.

## Documentation

Update SPEC, architecture, ledger, and this plan:

- operating states are explicit, bounded, and all checked;
- `Bistable` is per connected group authority;
- unpop means mandatory present + absent law states;
- no solver state choice is implicit.

## Gates

- `cargo build`
- `cargo test --lib`
- both electronics fixtures emit
- no new Praetor diagnostics in touched analysis/parser files
- deterministic state ordering and diagnostics
