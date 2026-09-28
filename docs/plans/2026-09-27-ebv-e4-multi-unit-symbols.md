# E4 — multi-unit symbols (op-amp = 2–3 units)

Date: 2026-09-27
Gap: `2026-09-21-hardware-dialect-gaps.md` E4 (multi-unit symbols)
Status: ALL SLICES LANDED 2026-09-27 — slice 1 `794d5ba6`, slice 2 `0587a027`, slice 3 this commit; gap E4 CLOSED

## Problem

`emit_symbol_def` (`src/backend/electronics/mod.rs:633`) emits ONE symbol
body per type — a single rectangle holding every pin, split left/right by
`pin_layout` (`:106`). `emit_instance` (`:674`) places it with `(unit 1)`.
KiCad's multi-unit symbol model — one library symbol carrying several
`(symbol "Name_X_Y")` unit blocks, each placed as a separate instance
`(unit N)` with its own reference (`U1A`, `U1B`) — is exactly how the
entire active-component catalog is drawn: a 5-pin op-amp is signal unit A
(inverting/non-inverting/input pair + output), signal unit B (dual op-amp),
and a power unit. Today every such part collapses to one rectangle with
every pin on the body — KiCad shows it, but ERC cross-references the wrong
pins and the schematic is unreadable.

## What exists (the family this extends)

- `pin_layout` (`:106`) already computes the shared pin geometry used by
  BOTH the symbol definition and the instance wire routing, so the two can
  never disagree — the unit split reuses it per unit.
- `TypeInfo` (`src/analysis/electronics.rs:53`) carries `pins` + index-aligned
  `pin_classes`; the unit is a new index-aligned field on the same pattern.
- `emit_symbol_def` already emits `(symbol "Name_0_1")` body +
  `(symbol "Name_1_1")` pins — the unit-N block is the same shape with a
  different suffix and a different pin subset.
- `emit_instance` already writes `(unit 1)` and the `(instances (project …
  (path … (unit 1))))` provenance block — per-unit placement is the same
  emission with a unit number and per-unit reference.

## Design decisions

- **D1 — surface: `unit <name>` on the pin clause.**
  `pin in+ = 1: In unit A;` — the unit is a PIN property, not a type
  property, because one unit can host pins of different classes and a pin
  always belongs to exactly one unit. Pins without `unit` belong to unit
  `""` (the single-unit default). The parser stores it verbatim on
  `PinDecl`; resolution of "is this a unit name or a class ref" is
  positional (`unit` is a new keyword, unambiguous).
- **D2 — grouping is declarative, not positional.** Units are the distinct
  `unit` values in declaration order (first-seen order), numbered 1..N in
  that order. A type with no `unit` clauses is unit 1 only — existing
  output is byte-identical.
- **D3 — per-unit geometry reuses `pin_layout`.** Each unit's pins are
  laid out by `pin_layout` over the unit's SUBSET (declaration order within
  the unit), so intra-unit geometry is identical to today's single-unit
  geometry. Units are stacked vertically in the symbol definition (unit N's
  block origin at `y = (N-1) * unit_height`), matching KiCad's own
  convention that `(unit N)` instances sit one symbol-height apart.
- **D4 — per-unit placement instances.** Each `(unit N)` placement is a
  separate `(symbol (lib_id …) (at X Y 0) (unit N))` with its own reference
  (`<ref>A`, `<ref>B`, … — the unit's first-seen name uppercased, the
  first unit gets the bare reference). The instance's `value`/`footprint`
  properties are carried once on unit 1 only (KiCad convention: the
  footprint lives on the first unit); units 2..N carry no footprint
  property (they are not separately footprinted — one part, one footprint).
- **D5 — wire routing is unit-aware.** A pin on unit N resolves to the
  placement origin of unit N (the unit 1 origin + the unit offset), so nets
  route to the correct physical pin. The per-pin `pin_xy` map already keys
  on (component, pin) — it now stores the unit-N origin for that pin.
- **D6 — ERC stays clean by construction.** Each unit's pins are a
  complete symbol block with its own `(symbol "Name_X_Y")` pins, so KiCad's
  ERC reads each unit's pins independently; cross-unit nets (e.g. a power
  net joining both signal units' V+ pins) route through the unit-N origins.
- **D7 — no compiler knowledge of "op-amp".** The unit name is data
  (`unit A`, `unit B`, `unit PWR`); the compiler reads it generically and
  knows no part vocabulary (Rule 15). A relay (coil unit + contact units)
  and a dual op-amp (two signal units) use the same mechanism.

## Slices

1. **Parser + analysis**: `unit <name>` on the pin clause → `PinDecl.unit:
   Option<String>`; `TypeInfo` gains `pin_units: Vec<String>` (index-aligned
   to `pins`) + `unit_groups: Vec<Vec<usize>>` (first-seen unit order → pin
   indices); a type with no unit clauses is a single group. Tests: a
   5-pin op-amp parses into two units (A: pins 1,2,3; B: pins 4,5) with the
   correct groupings; a single-unit type is unchanged.
2. **Emitter**: `emit_symbol_def` emits one `(symbol "Name_X_Y")` body per
   unit (stacked) + per-unit pin blocks; `emit_instance` emits one
   `(symbol (lib_id …) (unit N))` per unit with per-unit reference and
   unit-1-only footprint; `pin_xy` stores per-unit origins. Test: a 5-pin
   op-amp (unit A, unit B) renders two symbol blocks and two placement
   instances with references `U1`/`U1A`/`U1B`-style, and a single-unit
   fixture's output is byte-identical to before.
3. **Gate fixture + docs closure**: an op-amp `.ebv` fixture with two
   signal units + a power unit; the schematic opens ERC-clean in KiCad
   (manual vendor gate — the compiler gate is the unit-N emission + byte-
   determinism); plan status → landed, gap E4 → CLOSED.

## Gate (from the gap entry)

A 5-pin op-amp (unit A, unit B, power) renders correctly and ERCs clean in
KiCad. The compiler gate: the emitted `.kicad_sch` carries per-unit symbol
blocks and per-unit placement instances, is byte-deterministic (sorted
iteration), and a single-unit fixture is byte-identical to the pre-E4
output (no regression).

## Landing record (2026-09-27)

- **Slice 1** (`794d5ba6`): `unit <name>` on the pin clause → `PinDecl.unit`;
  analysis groups pins into unit blocks in first-seen declaration order
  (`TypeInfo.pin_units` index-aligned to `pins`, `TypeInfo.unit_groups`
  first-seen unit order → pin indices); a no-unit type is one default group
  (the type's first letter). The parser rejects a second `unit` on one pin.
  BEAST (de)serialization carries `unit: None`. Tests: a 5-pin two-unit
  op-amp groups A/B correctly; a single-unit type is one group; the
  duplicate-unit form parse-errors.
- **Slice 2** (`0587a027`): `emit_symbol_def` emits one body+pin block pair
  per unit (unit N at `y = N * unit_height`, KiCad's per-unit stacking);
  single-unit types keep the original `Name_0_1`/`Name_1_1` block suffixes
  — verified byte-identical against four fixtures. `emit_instances` emits
  one placement instance per unit: unit 1 carries the bare reference + the
  footprint, unit N>1 carries `<ref><UnitName>` and no footprint. `pin_xy`
  stores each pin at its unit's origin so nets route to the correct pin.
  Unit/props tables bundled into structs (`UnitTables`, `PlacementProps`,
  `ComponentPlacement`) to stay under the parameter gate; `emit_unit_block`
  / `emit_component_units` split out to stay under the cognitive gate.
  Tests: a 5-pin two-unit op-amp renders per-unit blocks + instances; a
  single-unit type emits the original suffixes and no unit-2 instances.
- **Slice 3** (this commit): the `opamp_gate.ebv` gate fixture — a dual
  op-amp (signal A, signal B, power unit) emits three symbol units and
  three placement instances (`U1`/`U1B`/`U1P`) with the power net touching
  both signal units' supply pins. Plan + gap ledger closed.
