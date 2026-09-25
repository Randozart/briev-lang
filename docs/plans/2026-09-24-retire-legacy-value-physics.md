# Plan 2026-09-24: retire the legacy numeric-`value` physics path

**Status:** complete — all three slices landed; 2647 lib tests green; `parse_ohms` gone from `src/`
**Branch:** `feat/e14a-intent-synthesis`
**Refines:** `docs/plans/2026-09-23-quantities-and-annotation-doctrine.md`
(corollary 3), `docs/plans/2026-09-24-electronics-component-laws.md`
(Slice 1 legacy note), hardware-gaps Amendment XV

## Why

The annotation-vs-physics doctrine locks one rule: if the compiler must read
it to prove the board works → physics → a PascalCase spec key; if a human
reads it to build the board → annotation → `value`, opaque. Corollary 3 says
`derive_current` must stop reading instance `value` and `parse_ohms`ing it.

Component laws, per-instance `spec Resistance`, and the SPST migration have
removed every shipped consumer of the legacy path: `usb_sensor.ebv` and
`led_blinker.ebv` both declare `spec Resistance`. The remaining
`.or_else(parse_ohms)` in `collect_series_parts`
(`src/analysis/electronics.rs:1422`) is the last place the compiler
interprets an annotation as physics. Retire it now — before E15 sizing and
Phase 4 envelope specs build on top of the wrong source of truth.

## Decisions (locked)

- Two physics sources survive: instance `spec Resistance: 330Ohm;` (SI +
  dimension, `ComponentInstance.specs`) and the type-level default
  (`TypeInfo.resistance`). Instance wins over type.
- `value: "330"` alone derives NOTHING: no current class, no voltage
  divider, no power dissipation, no bound proof. `value` stays a carried
  BOM label (KiCad `Value` field), never parsed.
- A value-only resistor on a board with current/power obligations still
  compiles; it simply produces no derived evidence (bounds on un-derived
  pins are skipped exactly as today). Never a new hard error in this plan.
- `SeriesPart.raw` stays — it is `format_ohms(ohms)` of the DERIVED value,
  not the annotation.

## Slices

### Slice 1 — delete the fallback

- Drop the `.or_else(parse_ohms)` arm in `collect_series_parts`
  (`src/analysis/electronics.rs:1414-1426`); delete `parse_ohms` (`:1298`).
- Rewrite the function doc + `TypeInfo.resistance`/`has_laws` doc comments
  (no "legacy migration path").
- Rework `spec_resistance_feeds_series_derivation_before_legacy_value` into
  `value_annotation_alone_never_derives_series_physics`: same circuit with
  ONLY `value: "999R"` must produce zero net currents.

### Slice 2 — migrate tests that were exercising the fallback

Analysis/backend tests that rely on value-derived resistance gain
`spec Resistance: <n>Ohm;` on the instance (and `spec Resistance: Ohm;`
on the type where the fixture declares type specs). The circuits keep
asserting the SAME physics — only the source of the number moves.

Sites (from `grep 'value: "[0-9]'`): `electronics.rs` ~16 instances
(LED_CIRCUIT `:4479`, `Res { value: "10k" }` `:4641`, divider/parallel
group `:6421-6623`), `electronics_laws.rs:804`, `backend/electronics/mod.rs`
~7 instances (`:721`, `:818`, `:972`, `:1340`, `:1374`, `:1402`, `:1439`).

### Slice 3 — docs

- `docs/architecture/electronics-frontend.md`: the four legacy mentions
  (`:111`, `:129`, `:228`, `:301`) become "value is never parsed" wording.
- hardware-gaps ledger: Amendment XVI records the retirement (E-series
  value-readers `collect_series_parts`/`parse_ohms` gone).
- quantities plan corollary 3 marked DONE; `derive_current` reference in
  the doctrine note updated to point at `collect_series_parts`.
- `spec/SPEC.md`: quantity section note that instance `value` is opaque.

## Gates (per-commit)

- `cargo test --lib` green.
- `brievc build` on `examples/electronics/usb_sensor.ebv` +
  `led_blinker.ebv` + `tests/electronics/*.ebv` — emit clean.
- `grep -rn parse_ohms src` returns nothing.
- Praetor: no new diagnostics in `src/analysis`, `src/backend/electronics`.
- Docs in the same commit as the structural change.

## Non-goals

- E15 series-part placement/sizing (E-series step selection) — builds on
  this plan, not in it.
- Phase 4 envelope specs (`MinCurrent`/`MaxCurrent`) — separate plan.
- `values_distinct` (`electronics.rs:3691`) still compares switch `value`
  labels conservatively for the multi-pole ambiguity error. That is an
  identity check, not a physics read; migrating it needs a capacity spec
  (`Poles`) with a real consumer — trigger: a second ambiguity form wants
  capacity, not label, as its discriminator.
- Rail inference (E14b backlog) untouched.
