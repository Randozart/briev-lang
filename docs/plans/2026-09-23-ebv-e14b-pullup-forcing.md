# Plan 2026-09-23: E14b slice 1 — min-voltage pull-up forcing (pure intent)

**Status:** building
**Branch:** `feat/e14a-intent-synthesis`

## Context

E14a closed its gate (`974705eb`): the §3.2 fixture compiles with explicit
equalities; every site marked `// E14b:` is the pure-intent backlog — the
compiler must infer topology from *behavior* instead of written wires. The
full §3.2 pure-intent form still depends on retracted/OPEN features
(`.up`/`.released`/`.closed` members, `derive on:` type clauses, drive
assignment solving, rail inference), so E14b is sliced. This slice lands the
**one physics-forcing rule that is fully general, needs no new keywords, and
has a clean D13 surface**: a min-voltage obligation on a released (WiredAnd)
net forces an external pull-up.

## Scope (E14b-1)

- New declarative body statement (already parses, no keyword):
  `inst.pin.voltage >= <literal>V;` — a MIN-voltage obligation.
- **Pull-up forcing:** for each obligation on an undriven WiredAnd-class net,
  wire a free `spec PullUp: true` part between that net and the lowest
  qualifying driven rail (`>= Vmin`). D13 rules: zero parts or zero rails →
  hard error naming the obligation; multiple parts → enumerated ambiguous
  error; multiple rails at the same minimal volts → enumerated ambiguous
  error. Same-root obligations dedup (one pull-up per net).
- Typechecker admits the comparison as a declarative fact (pin-`.voltage`
  side + voltage-literal side) — never executed, like `inst = true`.
- `spec PullUp: true` new spec key (`definitions.rs` registry) + on stdlib
  Resistor and the fixture's Resistor (fixture type wins the metadata
  tables).
- `<=`/`<` obligations (gnd-path forcing) are a hard error for now: state the
  gnd wire explicitly — recorded as E14b-2.

## Out of scope (E14b-2 backlog, recorded in the fixture)

- Bus assembly (`u2.sda = u3.sda` stays explicit in the fixture).
- Button gnd-path forcing (`u2.gpio[3] <= 0.3V` needs mechanism synthesis).
- `en` drive assignment solving (en intent is ambiguous across 8 free gpio).
- Rail inference (rails stay explicit guard equalities).
- `derive on:` type clauses (open gap in the hardware-dialect ledger).

## Fixture change

i2c node loses the four `r_pu[*]` wiring facts and gains four voltage
obligations (`u2.sda.voltage >= 2.7V;` ×2 and `u2.scl.voltage >= 2.7V;` ×2);
the `u2.sda = u3.sda` / `u2.scl = u3.scl` bus unions stay (E14b-2). The usb /
button nodes are unchanged (their deltas are recorded E14b-2 backlog).

## Gates (per-commit)

`cargo test --lib` green · no new Praetor diagnostics in changed files ·
fixture still derives clean (existing `assert_clean`) · new tests: obligation
forcing wires r_pu to the 3.3 V rail; zero pull-up parts → hard error;
ambiguous parts → enumerated error.

## Doc maintenance

Ledger Amendment (E14b-1 landed — pull-up forcing; E14b-2 backlog named).
Design record §4 E14b row: slice 1 landed note. `electronics-frontend.md`
deferred list: pull-up forcing landed.