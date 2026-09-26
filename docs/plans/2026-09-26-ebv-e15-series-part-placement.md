# E15 — series-part placement and sizing (the §3.2 `derive on:` residue)

Date: 2026-09-26
Gap: `2026-09-21-hardware-dialect-gaps.md` E15 (series-part placement synthesis)
Status: planned — slices not started

## Problem

The gate fixture still states the LED's series resistor twice by hand:
its value (`r_led: … spec Resistance: 330Ohm` — "the provisional
datasheet pick") and its wiring (`r_led.a = u1.vout; r_led.b = led1.a`
drive-maps). §3.2's vision is `led1 = true;` — physics derives the
series resistor. E14b inferred every OTHER wire; this is the last
explicit signal wiring on the board.

## What exists (the forcing family this extends)

The E14b forcing passes already solve the shape of this problem for
VOLTAGE obligations: `force_pull_up` (min-voltage obligation) wires a
free `spec PullUp` part between the obligation net and the lowest
qualifying driven rail, with D13 determinism (sorted pick, enumerated
ambiguity) and hard errors when no part or no rail qualifies. E15 is
the CURRENT-obligation sibling: a minimum-current obligation on a
non-ohmic part's pin forces a free series part between the part's
anode-side net and its driving rail, in the LOAD PATH (series), not as
a shunt.

## Design decisions

- **D1 — obligation surface.** The node postcondition
  `led1.a.current >= 2mAmp;` is the current obligation (same surface
  the anti-vacuity work gave current bounds: state-scoped, no new
  keyword). The forcing family consumes it exactly as it consumes
  `voltage >= 2.7V` today.
- **D2 — the part is free, not synthesized (slices 1–2).** The author
  declares `r_led` (it is in the BOM; someone buys it) but does NOT
  wire it. Forcing wires it: `r_led.<pin0> <-> the LED's driven
  anode net`, `r_led.<pin1> <-> the rail that feeds the obligation`.
  Full instance synthesis (a part no `let` declares) stays OUT — it
  drags naming, packaging, and BOM identity behind it, and no gate
  needs it yet.
- **D3 — property interface, never part names (Rule 15).** The
  forcing pass consumes `spec SeriesPart: true` on a two-pin type the
  same way pull-up forcing consumes `spec PullUp: true`. The compiler
  never reads "Resistor".
- **D4 — stated value: verify, don't invent (slice 1).** With
  `spec Resistance` stated, forcing proves the window: for every
  operating state, `I = (Vrail − Vled) / (R + Rled)` must satisfy the
  obligation min and the part's `spec MaxCurrent` envelope. Out of
  window → hard error naming the window. The datasheet pick stays the
  author's contract; the compiler proves it.
- **D5 — unstated value: synthesize from the E-series (slice 2).**
  With no stated resistance, the compiler computes the window
  `R ∈ [(Vrail − Vled)/Imax, (Vrail − Vled)/Imin]` from the LED's law
  physics (`spec ForwardVoltage`, `spec DynamicResistance` — the
  led_blinker model) and picks a value from an E-series table in
  CONFIG (`config/e_series.dbvl`, the footprints.dbvl pattern —
  Rules 14/15: a data refinement, never hardcoded). The chosen value
  appears in the proof line and the BOM.
- **D6 — the LED needs physics before this is honest.** The gate
  fixture's `Led` type carries only `Tolerance`/`Rating` — a black
  box. Slice 1 migrates it to the led_blinker law model
  (`ForwardVoltage` + `DynamicResistance` laws) so the derived current
  is a solved operating-point quantity, not a vacuous pass.

## Slices

1. **Series forcing with stated value**: `spec SeriesPart` key
   (parser + property interface), current-obligation forcing arm,
   fixture `Led` law migration, `r_led` unwired and forced, window
   verification with hard out-of-window error, tests (in-window
   proves; out-of-window refuses; no free part = the D13-style hard
   error naming the fix).
2. **Value synthesis**: `config/e_series.dbvl` + loader; window
   computation from law physics; deterministic pick (lowest E-series
   step inside the window); tests (obligation picks 330R-class value;
   tighter obligation picks the next step; no table entry in window =
   hard error).
3. **Docs + gap closure**: fixture comments updated (the explicit
   wiring lines deleted), design record, E15 → CLOSED, ledger
   amendment if the closure changes doctrine.

## Gate (from the gap entry)

The fixture compiles with NO explicit r_led wiring; the LED's current
bound is proven from derived physics through the forced series part;
deleting r_led from the source is the D13 hard error naming the
obligation it abandoned.
