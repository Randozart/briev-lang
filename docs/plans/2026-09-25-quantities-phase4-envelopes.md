# Plan 2026-09-25: quantities Phase 4 — envelope specs & current bounds

**Status:** building
**Branch:** `feat/e14a-intent-synthesis`
**Plan chain:** `2026-09-23-quantities-and-annotation-doctrine.md` Phase 4
(deferred there, trigger now met), E15 prerequisites (gap registry).

## Why

The fixture's LED bound (`[d1.a.current > 0 && <= 20mAmp]` in
led_blinker) lives in a txn postcondition — a per-board restatement of
what is really a DATASHEET property. Phase 4 moves maxima into the
envelope channel (type-level, like `Tolerance`/`Rating`), gives
asymmetric parts per-pin envelopes, lets instances derate, and closes
the vacuous-proof hole: a stated current bound the solver cannot even
attempt is a silent skip today.

Also corrected at design time (2026-09-25, with the author):

- `MaxVoltage` needs no new key — it IS `spec Tolerance` (max volts per
  pin, landed). `MinVoltage` obligations already exist (voltage
  obligations, E14b-1). Phase 4 adds only the current side.
- Min bounds are NOT a spec key. Max ratings are unconditional physics
  (absolute maximum, every state); min current is state-dependent (the
  LED legitimately carries 0 A when the button is open), so its surface
  is the node POSTcondition — the proven-property bracket, not the
  forcing body. Nodes desugar to `Transaction`, so
  `collect_current_bounds` already reads them; no parser machinery.
- `when`-law canonical spelling carries the trailing `;`
  (`parse_when_law` eats it optionally) — swept in touched files.

## Decisions (locked with the author, 2026-09-25)

1. **`spec MaxCurrent` = unconditional envelope** — absolute-maximum
   rating, checked in every reachable state like Tolerance/Rating.
2. **Min bounds = node postconditions** — `[d1.a.current > 0]` in the
   proven bracket, state-scoped by the node guard; NOT a body statement
   (body comparisons are the forcing family).
3. **Pin-qualified + uniform** — `spec MaxCurrent: 20mA;` (all pins) or
   `spec MaxCurrent: a: 20mA, vdd: 100mA;` (asymmetric parts). Same
   grammar family as `stdnet<in: VBUS>`. Applies to Tolerance/Rating
   too (the VIN-24V-vs-VDD-3.6V problem).
4. **Per-instance envelopes lift** — the parse-time error on
   instance-literal `spec Tolerance`/`Rating`/`MaxCurrent` goes away;
   the instance value REPLACES the type default for that instance
   (derating: datasheet typical vs board derating).

## Scope

### Slice 1 — `spec MaxCurrent` + per-instance lift

- Parser: `spec_name_to_key` gains `"MaxCurrent" => "max_current"`;
  `parse_spec_value` arm for an Amp envelope (uniform + pin-qualified
  list; metadata keys `max_current` / `max_current:<pin>`). The
  instance-literal rejection at `expressions.rs:1276` lifts for
  `Tolerance`/`Rating`/`MaxCurrent` — envelope specs on lets flow into
  the instance's property map (the `spec Resistance` path).
- Analysis: resolution `max_current_for(inst, ti, pin)` — instance
  pin-qualified → instance uniform → type pin-qualified → type uniform.
  `check_max_current` in `post_solve_checks` (beside `check_tolerance`):
  every populated pin with an envelope, law-exact current first, net
  fallback; violation = what/why/fix; clean = a proof line ("absolute
  maximum").
- Tests: envelope proven; violation errors; instance override wins;
  pin-qualified beats uniform; unpop exempt.

### Slice 2 — pin-qualified envelopes for `Tolerance`/`Rating`

- Same parse mechanism for the two landed envelopes
  (`tolerance:<pin>`, `rating:<pin>`); `check_tolerance` reads the
  per-pin bound with uniform fallback; `derive_power` reads
  `rating:<pin>` where it reads per-pin dissipation.
- Tests: per-pin tolerance resolution; uniform fallback; asymmetric
  board proves.

### Slice 3 — anti-vacuity + fixture migration

- `check_current_bounds`: the silent `else { continue }` on a bound
  with no derivable current becomes a hard error — "no law physics
  solves this pin; declare the component's laws" (what/why/fix). The
  vacuous-proof hole E15 flagged is closed.
- led_blinker: `spec MaxCurrent: 20mAmp` on the Led instance; the
  min bound moves from the txn postcondition to a node postcondition
  (`async node lit [guard] [d1.a.current > 0] { }`).
- usb_sensor: led1's "txn postcondition today" comment updates.
- `when ... };` trailing-semicolon sweep in touched files only
  (usb_sensor, led_blinker, lib/std/electronics.bv).

### Slice 4 — docs

- Quantities plan Phase 4 → DONE + dated amendment (decisions 1-4).
- Ledger Amendment XX; frontend doc dated entry; SPEC envelope section
  (MaxCurrent + pin-qualified forms + per-instance derating + the
  min-in-postcondition rule); this plan's Status.

## Out of scope (backlog)

- E15 placement/sizing synthesis (now un-gated: its prerequisites are
  envelope specs + ForwardVoltage law + lower-bound proofs — all landed
  or landed by this plan; E-series selection remains the open piece).
- `spec Output` LDO law (E14b-8, separate slice).
- Min* spec keys — deliberately absent (decision 2).

## Gates (per-commit)

`cargo test --lib` green · no new Praetor diagnostics on touched
surfaces (normalized baseline diff) · fixture derives clean · emits for
`led_blinker` + positives unchanged, the two expected negatives still
fail · new tests per slice above.

## Doc maintenance

Quantities plan amendment + Phase 4 status. Ledger Amendment XX.
Frontend doc entry. SPEC envelope section. Fixture comments.
