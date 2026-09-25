# Plan 2026-09-24: Tolerance/Rating become PascalCase specs

**Status:** landed 2026-09-24 (Slices 1–4, commits `1546f04c` → docs).
Gates green per slice: `cargo test --lib`, emits for usb_sensor /
led_blinker / `tests/electronics/*.ebv`, Praetor no-new-diagnostics on
touched surfaces, grep gates (no legacy clause in live sources; no
`ast::top::Tolerance`/`Rating` symbol anywhere).
**Branch:** `feat/e14a-intent-synthesis`
**Refines:** `docs/plans/2026-09-23-quantities-and-annotation-doctrine.md`
(Phase 2, deferred → now), hardware-dialect-gaps Amendment XVI

## Why

Phase 2 of the quantities plan: `tolerance 3.6;` and `rating 0.25;` are the
last compiler-read physics declarations living outside the `spec` surface.
The doctrine locks PascalCase for every spec key the compiler reads
(`Resistance`, `MinCurrent`, `Rating`, `Tolerance` are already named in the
corollaries). Migrating them removes the clause/spec split, makes the two
envelopes quantities with units (a bare number cannot be misread), and lets
`.beast` round-trip them through `metadata` instead of dropping the fields
(serialize.rs currently hard-codes `tolerance: None`).

## Decisions (locked)

- New surface (type bodies only — same scope as the old clauses):
  - `spec Tolerance: 3.6V;` — max volts, explicit ASCII unit required,
    dimension-checked to Volt (`3.6Ohm` is an error naming the key's unit).
  - `spec Tolerance: any;` — declared unrated (Identifier, unchanged
    semantics: `f64::INFINITY` in `TypeInfo`).
  - `spec Rating: 0.25W;` — max watts, dim-checked to Watt.
  - `spec Rating: any;` — declared unrated.
- Bare numbers are rejected on these keys (`spec Tolerance: 3.6;` → error
  "expects volts — e.g. `3.6V`, or `any`"): the explicit unit is the point
  of the migration; `parse_law_parameter_spec` already sets this precedent.
- Old clauses (`tolerance …;` / `rating …;`) are retired with a hard
  what/why/fix at the clause site, never a generic parse error.
- AST: delete `ast::top::Tolerance` / `Rating` enums and the
  `TypeDefBody.tolerance` / `rating` + `CellDef.tolerance` / `rating` fields.
  Values live in body `metadata` as `PropertyValue::Quantity { Volt | Watt }`
  or `PropertyValue::Identifier("any")` — the same channel as `Resistance`,
  so `.beast` serializes them for free.
- `TypeInfo.tolerance` / `TypeInfo.rating` keep their `Option<f64>` shape
  (volts / watts, `any` → `INFINITY`); `check_tolerance`, `derive_power`,
  `derive_law_power` are untouched consumers.
- Envelope specs stay TYPE-level in this plan. An instance-literal
  `spec Tolerance:` is a parse-time error naming the type body as its home
  (per-instance envelopes are Phase 4 design, not a silent no-op here).

## Slices (one commit each, gates green per commit)

### Slice 1 — accept the new surface (transitional)

- `spec_name_to_key`: `Tolerance` → `"tolerance"`, `Rating` → `"rating"`.
- `parse_spec_value` arms: `any` → Identifier; quantity with explicit
  unit + dimension check (Volt / Watt) → `PropertyValue::Quantity`.
- Instance-literal spec parsing: reject `Tolerance`/`Rating` with the
  type-body fix (see Decisions).
- `derive_netlist` TypeInfo build reads metadata first, falls back to the
  legacy field behind a `// TEMP: 2026-09-24:` comment naming this plan as
  the removal path (Rule 16 — the fallback dies in Slice 3).

### Slice 2 — migrate every source

Mechanical clause→spec rewrite across:
- `lib/std/*.bv`, `examples/**/*.ebv`, `tests/electronics/*.ebv`
- embedded Rust test sources in `src/analysis/electronics.rs`,
  `src/backend/electronics/mod.rs`, `src/parser/definitions.rs`, and any
  other `.rs` file containing `tolerance <n>;` / `rating any;` in a string.
- Diagnostics/fix texts gain the new spelling (`fix: add spec Tolerance:
  3.6V;`, `spec Rating: any;`) in the same sweep; tests asserting those
  messages update.
- Gate: `grep -rnE '^\s*(tolerance|rating)\s+(any|[0-9])' lib examples tests
  src` → empty (clause form gone from every live source).

### Slice 3 — retire the clauses

- Delete `at_/parse_tolerance_clause`, `at_/parse_rating_clause`, the
  `TypeBodyTargets.tolerance/rating` plumbing, the AST enums + four fields,
  and the Slice-1 TEMP fallback (TypeInfo reads metadata only).
- Clause-site detection emits the retirement error (what: removed 2026-09-
  24, why: physics is a PascalCase spec with a unit, fix: the new form).
- Update all struct-literal construction sites
  (`tolerance: None, rating: None` → gone) across analysis/backends/beast.
- Parser tests: clause-removed error test + spec-form tests replacing the
  field-assertion tests (`:5605`, `:5615`).

### Slice 4 — docs

- `spec/SPEC.md` §2 properties example + prose (`:252-266`) → spec forms.
- `docs/architecture/electronics-frontend.md` clause mentions.
- hardware-gaps ledger: Amendment XVII (Phase 2 landed).
- quantities plan: Phase 2 status note (deferred → done 2026-09-24).
- Syntax highlighter: no grammar entry exists for the clauses (verified) —
  no change; `learn-briev/04-functions.md` tolerance is FP derivation
  (unrelated).

## Gates (per-commit)

- `cargo test --lib` green.
- `brievc build` usb_sensor + led_blinker + `tests/electronics/*.ebv`.
- Praetor: no new diagnostics in `src/parser`, `src/analysis`,
  `src/backend/electronics`, `src/ast`.
- Grep gates: no legacy clause in live sources (Slice 2+), no
  `ast::top::Tolerance`/`Rating` symbol anywhere (Slice 3+).

## Non-goals

- Phase 4 envelope specs (`MinCurrent`/`MaxCurrent`/`MinVoltage`/
  `MaxVoltage`) — separate plan; needs the pin-semantics decision.
- Per-instance tolerance/rating overrides (see Decisions: rejected, not
  silently ignored).
- FP derivation `-> [tol]` examples, `values_within_tolerance`, and
  derive/mcmc `metadata["tolerance"]` — different features, untouched.
