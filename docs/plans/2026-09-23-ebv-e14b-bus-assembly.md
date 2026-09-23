# Plan 2026-09-23: E14b slice 3 — bus assembly (open-drain nets)

**Status:** building
**Branch:** `feat/e14a-intent-synthesis`

## Context

E14b slices 1–2 landed pull-up and low-hold forcing. The i2c node still
carries two explicit bus unions (`u2.sda = u3.sda;`, `u2.scl = u3.scl;`).
Without them the four min obligations on sda/scl would demand four
pull-ups from two free parts and fail. This slice assembles the bus from
the shared obligation.

## Scope (E14b-3)

- **Bus assembly:** MIN obligations on same-name WiredAnd-class pins
  (open-drain, released = high-Z) at the SAME voltage union into one net
  before pull-up forcing. General rule, not vocabulary: the pin name is
  the author's signal identity; WiredAnd is the class that may share a
  driven net; the shared obligation is the coupling that justifies the
  union. Different names, different voltages, or non-WiredAnd pins never
  union (separate nets each get their own pull-up — honest D13).
- Runs first in `ObligationForcing::run`, before the per-root dedup, so
  the assembled bus is one net needing one pull-up.
- Fixture: the two `u2.sda = u3.sda` / `u2.scl = u3.scl` facts are deleted
  — the i2c node is now obligations only. Netlist identical to E14a.

## Out of scope (backlog)

- en drive assignment solving, rail inference, value-aware resistor
  matching, `derive on:` types.

## Gates (per-commit)

`cargo test --lib` green · no new Praetor diagnostics · fixture still
derives clean · new tests: same-name WiredAnd same-volt obligations union
(one proof + one pull-up); different names stay separate; non-WiredAnd
pins never union; different voltages never union.

## Doc maintenance

Ledger Amendment (E14b-3 landed). Design record E14b row. Frontend doc
deferred refresh.