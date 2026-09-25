# Plan 2026-09-25: E14b slice 7 — supply-rail membership via `net<>` / `stdnet<>`

**Status:** landed 2026-09-25 (Slices 1–3)
**Branch:** `feat/e14a-intent-synthesis`
**Design record:** E14b gate delta item 1 — the rail-membership half of
"rails and returns stated as guard equalities". Slice 6 (2026-09-25,
`2026-09-25-ebv-e14b-return-net.md`) closed the return half; this slice
closes membership.

## Why

After slice 6, the gate fixture's guard still carries supply-rail
MEMBERSHIP equalities (`j1.vbus == u1.in`, `u1.vout == u2.vdd`,
`u1.vout == u3.vdd`, `j2.vcc == u1.vout`). These are design decisions,
not physics: several supply pins are tolerance-compatible with multiple
driven rails, and different memberships are different physical boards.
The compiler must not guess (D13) — but it must also not demand a wire
when physics or declared convention forces the answer.

Ladder (the house pattern: infer; else demand; error when critical):

1. **Expectation** — a declared standard-net name constrains candidates.
2. **Refutation** — tolerance filters candidates; a unique survivor
   infers silently (proof line with provenance, D3).
3. **Propagation** — an ambiguous pin with exactly one bound-named
   candidate rail joins it.
4. **Error-that-asks** — residual ambiguity enumerates candidates and
   names the `net<>`/`stdnet<>` fix. On supply copper a wrong guess is
   a fried board, so there is no warn-and-continue rung; the "ask" IS
   the error text.

## Design (decisions locked with the author, 2026-09-25)

### Two keywords, two kinds of meaning

```briev
stdnet<VBUS> let j1: UsbMicro = UsbMicro { ... };   // registry-backed
stdnet<V3V3> let u2: Mcu     = Mcu     { ... };     // sole supply pin
stdnet<in: VBUS, vout: V3V3> let u1: Ldo = Ldo { ... };  // pin-qualified
net<div_mid> let x: Divider  = Divider { ... };     // board-local, opaque
```

- **`stdnet<Ident>`** — registry-backed. The registry is stdlib TYPES
  beside `Power`/`Ground` (the class-fundamental pattern; Rule 14 — the
  stdlib learns, the compiler reads properties generically):

  ```briev
  type V3V3 { spec NetVoltage: 3.3V; spec KicadLabel: "+3V3"; };
  ```

  `stdnet<X>` = "a type named X declaring `spec NetVoltage` is in
  scope" (same lookup shape as `pin vdd: Power`). The expectation
  filters membership candidates to rails driven at the declared
  voltage, and is CHECKED against existing drives (contradiction =
  hard convention error). Unknown stdnet name → error telling the
  author to declare it or use `net<>`. Seeds: VBUS (5V), V5V, V3V3,
  V12V. Return stays class-inferred — no stdnet<GND>; the registry is
  supply-family only.
- **`net<ident>`** — board-local, OPAQUE. The compiler never consults
  the registry for it (even if the string collides with a declared
  standard net) and never reads it as physics (Rule 15). It binds to a
  rail only via a physics-forced pin (the anchor rule), then propagates.

Rule 15 compliance: the compiler reads DECLARED properties (NetVoltage,
KicadLabel) or matches identifiers for equality; it never parses a net
name for meaning.

### Pin-qualified form

`<>` carries either one bare name (requires the type to have exactly
one supply-class pin) or a comma list of `pin: name` pairs. Qualifier
rules (all hard errors): unknown pin; non-supply-class pin; duplicate
pin; qualifying a Return-class pin (return is class-inferred);
uncovered supply pin (falls to the membership pass, which errors on
it).

### Anchoring and propagation

A name (either keyword) binds to a driven rail only through a pin
whose membership physics forces: the rail's own drive-fact source pin,
a tolerance-refuted pin, or an expectation-forced pin. The compiler
never binds a name to a rail by reading the name. Ambiguous pins then
propagate toward bound-named rails: exactly one named candidate →
join (proof line); zero or ≥2 → D13 enumerated error.

### Expectations constrain, never drive

A rail is born from a boundary drive (guard fact, e.g.
`j1.vbus.voltage == 5.0V`) or a component law — never from an
expectation. `stdnet<V3V3>` on an undriven rail is not a voltage
source; the rail-birth facts stay in the guard until component laws
(`spec Output` on the LDO) replace them. The fixture's guard therefore
ends at pure rail births — §3.2's exact form.

### Labels

A bound name flows through `Net` into the KiCad emitter: author names
become net labels (stdnet labels from `spec KicadLabel`); unnamed
rails keep the physics-derived labels (GND, V3.3).

### Relationship to the 2026-09-22 retirement

`net <name>:` / `store net(pin) = "name"` were retired as LABEL
annotations duplicating derived knowledge (`2026-09-22-retract-lifting-slots.md`):
"a name that contradicts physics would be a lie we'd trust". This
slice reintroduces the surface as MEMBERSHIP SEMANTICS the engine acts
on: selection + expectation checks + propagation, with physics
refusing what the author may not assert. The honest-keyword rule stands:
where inference alone suffices, the keywords only NAME (they are "not
strictly required").

## Scope

### Slice 1 — membership-by-refutation pass (analysis-only)

- New pass in the return-topology pipeline (after return-net union,
  before decoupler auto-bridging — supply nets must exist for the
  bridge test): for each populated instance's supply-class pin whose
  net is unconnected, candidates = driven rails filtered by pin
  tolerance.
- Unique survivor → union + proof ("membership inferred: only <v>V
  rail within tolerance"). Zero candidates → hard error (no driven
  rail within tolerance / none at all). Multiple → new ambiguity
  diagnostic enumerating the rails (fix text this slice: "state the
  wire"; slice 2 upgrades it to the keywords).

### Slice 2 — `net<>` / `stdnet<>` modifiers end-to-end

- Parser: `consume_modifier_prefix` arm for `net`/`stdnet` + `<...>`
  (bare name or `pin: name` list) on `let`. Contextual, statement-safe
  (guards keep `<` for less-than; the modifier form exists only in the
  declaration prefix position).
- AST: the let item carries the net annotations.
- Registry lookup: declared types with `spec NetVoltage` (+ optional
  `spec KicadLabel`); unknown name error.
- Membership pass upgrade: expectation filter (before tolerance),
  convention-mismatch errors, name binding via forced pins,
  propagation, and the full error matrix (unknown stdnet name,
  expectation-vs-drive contradiction, bare-form-on-multi-supply,
  qualifier errors, unbound `net<>`, ≥2 named candidates).
- Labels: `Net` carries the author label; KiCad `emit_net` uses it.

### Slice 3 — registry seeds + fixture + docs

- `lib/std/electronics.bv`: VBUS, V5V, V3V3, V12V declarations.
- `usb_sensor.ebv`: lets carry `stdnet<>` (`u1` pin-qualified); the
  `usb_powered` guard shrinks to `usb_attached && rail births`.
- Docs: ledger Amendment XIX; design-record E14b row (gate delta item 1
  CLOSED except rail births); frontend doc dated entry; SPEC section
  (membership ladder, both keywords, expectations-constrain-never-drive,
  registry declaration); this plan's Status.

## Out of scope (backlog)

- The LDO output law (`spec Output` + `when` law) so `u1.vout == 3.3V`
  stops being a guard fact — component-laws follow-on; the dropout law
  would also refute `u1.in` self-feed independently of expectations.
- Return-net naming / split returns (Return class is inferred).
- Series-resistor placement (E15, trigger-gated).
- Signal-net naming at declaration (membership pass is supply-only).

## Gates (per-commit)

`cargo test --lib` green · no new Praetor diagnostics on touched
surfaces (normalized baseline diff) · fixture derives clean · emits for
`led_blinker` + `tests/electronics` positives unchanged, the two
expected negative fixtures still fail · new tests: unique-survivor
inference; tolerance-refuted candidate excluded; zero-candidate error;
ambiguity enumerates; expectation filters and binds; expectation-drive
contradiction errors; unknown stdnet name; bare-form-on-multi-supply;
qualifier errors; anchor-via-forced-pin; propagation; unbound net<>;
≥2 named candidates; identifier opacity; label reaches KiCad; unpop
exempt.

## Doc maintenance

Ledger Amendment XIX. Design-record E14b row + gate-delta closure note.
Frontend doc entry. SPEC membership section. Fixture comments.
