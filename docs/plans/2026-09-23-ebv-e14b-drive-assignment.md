# Plan 2026-09-23: E14b slice 4 — drive assignment solving

**Status:** building
**Branch:** `feat/e14a-intent-synthesis`

## Context

E14b slices 1–3 forced pull-ups, low-holds, and bus assembly from voltage
obligations. The usb node still pre-wires `u1.en = u2.gpio[0];` because
the `u1.en = true;` pin intent would be ambiguous across eight free gpio
pins at completion time, and `led1 = true` competes for the same supply.
This slice lands drive assignment: several open drive intents competing
for INTERCHANGEABLE free drive-capable pins are a perfect matching,
assigned deterministically — the same doctrine as identical pull-up parts
(slice 1). `u1.en` wiring is now fully inferred.

## Scope (E14b-4)

- **Batch drive matching:** when ≥2 drive intents (instance form or pin
  form) each have exactly one open pin, and the free drive-capable pins
  are all interchangeable (same PinClassProps), assign sorted-intent ↔
  sorted-pin deterministically and wire. Runs BEFORE the per-intent
  completions; matched intents skip the per-intent path.
- **Fall-through (D13 preserved):** supply < demand, or mixed-class
  supply → per-intent completions run and produce their existing errors
  (no completion / enumerated ambiguous). A single completable intent →
  per-intent path unchanged.
- Fixture: delete `u1.en = u2.gpio[0];` — the usb node drives `u1.en =
  true;` and `led1 = true;` and the batch wires en↔gpio[0], led1↔gpio[1]
  (gpio[2,3,4,5,6,7] already claimed by j2 facts and the button
  low-hold). Netlist identical to E14a.

## Out of scope (backlog)

- Rail inference, value-aware resistor matching, series-resistor
  placement synthesis, mechanism control from guard conditions.

## Gates (per-commit)

`cargo test --lib` green · no new Praetor diagnostics · fixture derives
clean · error-matrix case 1 reworded (removing the led1 driver leaves the
en intent ambiguous with enumerated candidates) · new tests: batch
resolves en+led1; shortage → no-completion error; mixed-class supply →
ambiguous error.

## Doc maintenance

Ledger Amendment (E14b-4 landed). Design record E14b row. Frontend doc
deferred refresh.