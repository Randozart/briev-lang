# Plan 2026-09-23: first-class quantities + annotation doctrine

**Status:** Phase 1 building (quantity foundation)
**Branch:** `feat/e14a-intent-synthesis`

## Doctrine (locked 2026-09-23)

**The one rule — compiler vs annotation:**

> If the compiler must READ it to prove the board works → physics →
> PascalCase spec key. If a human/manufacturer reads it to build the board
> → annotation → lowercase, opaque.

Decides every case: `value: "4k7"` is read by the person placing the part
(annotation); resistance is read by the compiler to derive current
(physics → a spec); `reference "U"`/`U8` is read by the person at the PCB
(annotation). The compiler carries annotations, never interprets them.

Three corollaries:
1. **PascalCase for compiler-intelligible surface.** `Supply`, `CanDrive`,
   `WiredAnd`, `Decouple`, `Decoupler`, `Tolerance`, `Rating`,
   `Resistance`, `MinCurrent`, `MaxCurrent`, `MinVoltage`, `MaxVoltage`,
   `Conducts` — every spec key the compiler reads as physics/property.
   Grammar keywords (`pin`, `node`, `when`, `budget`) are not spec keys —
   untouched.
2. **Quantities are bare, never quoted.** `spec MinCurrent: 2mA;`, not
   `"2mA"`. Quoted implies arbitrary. Unit-suffixed quantity notation
   (`2mA`, `3.3V`, `100n`, `4k7`) is core `.ebv` — `Expr::UnitLiteral`
   already lexes it; spec VALUES gain the same grammar.
3. **Resistance is physics, not a BOM label.** `derive_current` today
   reads instance `value` and `parse_ohms`es it (`electronics.rs:1064`) —
   interpreting an annotation as physics. It must move to a
   `spec Resistance` value; `value` becomes a pure annotation. (This is
   the semantic fix that makes E14b-5's "values are immaterial for
   obligation satisfaction" structurally true, not just a rule.)

## Decisions (locked)

- **Per-instance physics** lives in the instance literal:
  `let r1: Resistor = Resistor { value: "4k7", spec Resistance: 4.7kΩ };`
  — `value` stays the BOM label, resistance is the physics.
- **`any`** stays a bare identifier value: `spec Tolerance: any;`.
- **`Conducts`** (conduction condition / tuple specs) deferred — no
  consumer; trigger = a second class wanting a per-instance obligation.
- **Unit model:** scaling prefixes + base units + key-dimension
  interpretation + dimension-conflict hard error + E-series fraction.

## Unit grammar

`QuantityDim` (compiler-intrinsic enum, independent of stdlib type names,
Rule 15): `Volt, Amp, Ohm, Farad, Henry, Hertz, Watt, Kelvin`.

`PropertyValue::Quantity { si: f64, dimension: QuantityDim }` — SI is the
ground truth; diagnostics reuse `format_amps/volts/watts`.

Spec value = `[number][suffix]` | `[number]`:

- **Scaling prefixes** (case-sensitive): `p`1e-12 `n`1e-9 `u`1e-6 `m`1e-3
  `k`1e3 `M`1e6 `G`1e9. `m` = milli, `M` = mega, `k` = kilo (lowercase),
  `K` = Kelvin (uppercase base).
- **Base units:** `V A R Ω F H Hz W K`.
- **Suffix forms:**
  - `[prefix][base]` — `2mA` → 2e-3 A; `3.3V` → 3.3 V; `4.7kΩ` → 4.7e3 Ω.
  - `[prefix]` bare — `100n` on a Farad key → 1e-7 F (base from the key's
    dimension).
  - `[prefix][digits]` E-series fraction — `4k7` → 4.7e3 (digits = the
    fractional mantissa).
  - `[base]` — `330R` → 330 Ω.
  - bare number — base unit of the key's dimension.
- **Dimension resolution:** the spec registry carries the dimension per
  key. A suffix with an explicit base that CONFLICTS with the key's
  dimension is a hard error (`spec MinCurrent: 3.3V` → "expects a
  current, got V"). A bare prefix takes the key's dimension.
- **`any`** (Identifier) → no-constraint, for Tolerance/Rating only.

## Phases

### Phase 1 — quantity foundation (NOW)

- `QuantityDim` enum + `PropertyValue::Quantity`.
- `parse_spec_value` gains a quantity arm; unit parser (prefixes, bases,
  fraction, dimension-conflict errors).
- Registry: `decouple` → Farad quantity key (value was presence-only; now
  a real quantity).
- Migrate `spec Decouple: "100n"` → `spec Decouple: 100n;` — stdlib,
  fixture, analysis/backend tests (8 sites).
- Tests: unit grammar (bare prefix, prefix+base, fraction, base),
  dimension-conflict error, bare-number = base unit.

### Phase 2 — Tolerance/Rating → PascalCase specs (DONE 2026-09-24)

- Delete `tolerance`/`rating` clauses; `spec Tolerance: 3.3V;` /
  `spec Rating: 0.25W;` (dim Volt/Watt, `any` = Identifier). Landed per
  plan `2026-09-24-tolerance-rating-spec-migration.md` (Slices 1–4):
  clauses parse-error with the replacement spelling; AST enums/fields
  deleted; metadata is the single channel.
- `check_tolerance`/`derive_power` read specs; AST TypeInfo built from
  specs; migrate stdlib + fixture + all tests.
- Trigger: Phase 1 lands clean; this is a mechanical migration.

### Phase 3 — Resistance → spec (deferred)

- Instance-literal `spec Resistance`; `derive_current` reads it; `value`
  becomes pure annotation; no `parse_ohms`-on-annotation anywhere.
- Trigger: Phase 2 lands; per-instance physics home confirmed.

### Phase 4 — Envelope specs (DONE 2026-09-25 — plan
### `2026-09-25-quantities-phase4-envelopes.md`)

- `spec MaxCurrent: 20mA;` (dim Amp) — the absolute-maximum current
  envelope, unconditional, checked in every reachable state like
  Tolerance. Uniform or pin-qualified (`a: 4mA, vdd: 100mA`), and
  instance literals override (derating).
- `MinVoltage`/`MaxVoltage` deliberately have NO keys: MaxVoltage IS
  `spec Tolerance` (landed Phase 2); min bounds are node POSTconditions
  (`[d1.a.current > 0]` / voltage obligations) — state-scoped by the
  node guard, because a min-current requirement does not hold in every
  state (the LED legitimately carries 0 A when unpowered).
- Lower-bound proofs: already landed with the component-law DC solve;
  Phase 4 closed the vacuous-proof hole instead — a stated current
  bound with no derivable current is a hard error (absent-participation
  states exempt).
- Fixture LED bound: `spec MaxCurrent` on the instance + the minimum in
  the node postcondition (led_blinker migrated).

### Phase 5 — Docs

- Annotation-vs-physics doctrine recorded (in flight).
- Ledger Amendment; design record; SPEC notation section (bare quantities).

## Gates (per-commit)

`cargo test --lib` green · no new Praetor diagnostics in changed files ·
fixture + stdlib parse · quantity-grammar tests pass.

## Doc maintenance

This plan + the doctrine in `docs/architecture/electronics-frontend.md`.
Ledger Amendment VII (Phase 1 landed) + subsequent phases when they land.

## Amendment 2026-09-24: corollary 3 closed

Phase 3 (`spec Resistance`) landed with the component-laws plan, and the
legacy `parse_ohms`-on-annotation path it displaced was deleted the same day
— `docs/plans/2026-09-24-retire-legacy-value-physics.md`, ledger Amendment
XVI. `derive_current`'s series graph now reads only structured physics
(instance spec, type default). Corollary 3 is DONE: `value` is pure
annotation structurally, not by rule. Phase 4 landed 2026-09-25 (plan
`2026-09-25-quantities-phase4-envelopes.md`); the per-instance envelope
surface lifted with it — instance literals override type envelopes
(derating semantics), the question this amendment left deferred.