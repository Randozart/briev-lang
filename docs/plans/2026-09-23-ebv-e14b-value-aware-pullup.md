# Plan 2026-09-23: E14b slice 5 — value-aware pull-up matching

**Status:** building
**Branch:** `feat/e14a-intent-synthesis`

## Context

E14b slices 1–4 forced pull-ups, low-holds, bus assembly, and drive
assignment. The button node still wires its pull-up explicitly
(`r_btn.a = u1.vout; r_btn.b = u2.gpio[3];`) because the pull-up forcing
errors "ambiguous" when free parts carry distinct values — and the button
obligation competes with the two i2c buses for three parts of differing
value (4k7, 4k7, 10k).

The ambiguity is over-strict. A MIN-voltage obligation is satisfied by ANY
pull-up resistance: a released (high-Z) net sits at the rail regardless of
the resistor value. The choice therefore never matters for satisfaction —
the same doctrine as the slice-4 drive assignment (identical-capacity
parts assign deterministically).

## Scope (E14b-5)

- **Pull-up forcing:** drop the distinct-value ambiguity error. Every free
  `spec PullUp` part satisfies a min obligation equally; the forcing picks
  the first free part (sorted). The no-part (supply shortage) error stays.
- **Switches keep the ambiguity:** a low-hold switch can be multi-pole (a
  DPDT serves two low-holds) — different capacities, so the pick can
  matter; `force_low` is unchanged.
- Fixture button node: add `u2.gpio[3].voltage >= 2.7V;` and delete the
  two r_btn facts — the button pull-up is now inferred (the compiler may
  place a 4k7 there and the 10k on an i2c bus; the choice is immaterial,
  the netlist equivalent).

## Out of scope (backlog)

- Rail inference, series-resistor placement synthesis, mechanism control
  from guards.

## Gates (per-commit)

`cargo test --lib` green · no new Praetor diagnostics · fixture derives
clean · slice-1 test `e14b_distinct_value_pull_ups_are_ambiguous`
becomes `..._assign_deterministically` (no error, one pull-up) · new
test: 3 obligations × 3 distinct-value parts all assign, no dangling ·
`e14b_distinct_switches_are_ambiguous` unchanged (switches keep it).

## Doc maintenance

Ledger Amendment (E14b-5 landed). Design record E14b row. Frontend doc
deferred refresh.