# Plan 2026-09-23: E14b slice 2 — low-hold forcing (gnd path)

**Status:** building
**Branch:** `feat/e14a-intent-synthesis`

## Context

E14b slice 1 (`fab69c19`) landed min-voltage pull-up forcing. A MAX
obligation (`inst.pin.voltage <= V`) is still a hard error ("state the
wire explicitly"). This slice lands the low-hold counterpart: the net must
be held at or below Vmax, forcing a path to the return rail.

## Scope (E14b-2)

- `inst.pin.voltage <= <literal>V;` (Le/Lt) becomes a declarative LOW-HOLD
  obligation instead of an error.
- **Low-hold forcing:** for each max obligation on a net not already on the
  return rail, wire a free switchable part (type with ≥2 Switchable-class
  pins — Path) between the net and the instance's return rail
  (Return-class pin; the net's own instance's return first, else any
  return root).
  - Net already on the return root → satisfied.
  - Net already has a switchable part to the return root → satisfied.
  - Free switchable parts: distinct-value → enumerated ambiguous error;
    one → wire net→p1, p2→return; none → if the net is pulled up (a
    PullUp part already on it) → hard error (a pulled-up net cannot be
    held low without a mechanism); else wire the net directly to return.
- Fixture: `type Switch` pins become `: Path` (E12 class, passive KiCad
  type — emission unchanged); the button node keeps its explicit r_btn
  pull-up (value-aware resistor matching is backlog) and replaces the two
  sw1 facts with `u2.gpio[3].voltage <= 0.3V;`. Netlist identical to E14a.

## Out of scope (backlog)

- Value-aware resistor matching (pull-up choice among distinct values).
- Mechanism *control* wiring from guard conditions (sw1 is wired as
  always-conducting copper, matching the E14a state model).
- Bus assembly, en drive solving, rail inference, series-resistor
  placement synthesis.

## Gates (per-commit)

`cargo test --lib` green · no new Praetor diagnostics · fixture still
derives clean · new tests: max obligation forces the switch path; pulled-up
net with no switchable part → hard error; isolated net wires directly to
return; distinct-value switchable parts → ambiguous error.

## Doc maintenance

Ledger Amendment (E14b-2 landed). Design record E14b row: slice 2 note.
`electronics-frontend.md` deferred list: low-hold forcing landed.