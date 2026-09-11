# Fundamentals Doctrine: the #Category Blast + Electronics Briev

**2026-09-11.** Authoritative plan. Supersedes the `<->`-era portions of
`2026-09-09-briev-native-runtime-and-family-realignment.md` (§4.3) and
amendment 2's grammar sketch. Status: APPROVED, not started. Baseline:
branch `feat/briev-native-runtime` at merge `666342e1`, 2127 tests green.

---

## 0. Doctrine (what this plan locks in)

These are permanent design decisions. Every step below implements them;
no step may weaken them.

**D0 — Same code, mapped meaning (the .rbv principle).** You type
(roughly) the same Briev code in every family; the compiler turns those
declarations into their final shape based on how your intentions map onto
the backend. Unique per-family syntax is allowed (`.rbv` has the most),
but the declaration core is shared. Electronics must not fork the
language.

**D1 — Forms are syntax.** `obj`, `struct`, `cell`, `type` carry no
domain semantics. They are declaration shapes: syntax for "a thing with
properties." An obj can exist in `.ebv`, so can a struct and a cell.
Electronics-ness lives in the *fundamentals* (types), the *properties*
(clauses), and the *backend mapping* — never in the form keyword.

**D2 — Fundamentals stand alone.** `Float` is both the base type AND the
protocol. One name, two roles. `Float32: Float` derives by plain parent;
`Float<Posit>` carries the variant. The category hashwords — `#Float`,
`#Int`, `#UInt`, `#String`, `#Bool`, `#Char`, `#Blob`, `#Bit`, `#Data` —
are blasted from existence: hard compile error, fix message, no
deprecation alias, conclusive. `#Float` was a mistake; this absorbs it
structurally: a fundamental needs no second name.

**D3 — Electrical quantities are their own bases.** Electronics Briev
has fundamentally different bases. `Volt` is NOT a `Float` with a unit —
no IEEE semantics, no numeric tower, no `Cast.Float` inheritance, no
programming baggage. Briev expresses a state machine; `.ebv` literally
declares an electronics state machine: reactor machinery (txn / trg /
contract / proof) over electrical state. Quantities are proven at compile
time, never executed (provenance values; no bits surface until
simulation exists, which is deferred).

**D4 — Universality split.** The compiler owns the *mechanism* (pin
clauses, mandatory-property rule, net derivation, electrical proving).
The stdlib owns the *catalog* (the Device-set of components). Users own
everything beyond it. Nothing electronic is compiler-known by name.

**D5 — Inference over wiring.** There is no connection operator. Nets are
the transitive closure of pin-equality obligations in contracts; the
compiler derives, proves, and refuses incomplete boards. (Supersedes the
`<->` model of the 2026-09-09 plan.)

**D6 — Properties are declarations, not metadata.** Pin lists, reference
prefixes, tolerances: first-class named clauses on the declaration
surface, enforced, structurally read. The `!>` annotation path for these
is deleted, not shimmed.

---

## Phase 0 — Land the in-flight work (preblast hygiene)

Uncommitted work in the tree must land green before the blast touches
anything.

0.1 **Fix the voltage-test parse bug.** The new voltage tests fail at the
`txn` line (`expected identifier, found '['`). Almost certainly `pin in;`
— `in` is a reserved word, same trap as `out` (which bit the first spike).
Rename to `vin`/`vout` in test fixtures.

0.2 **Commit the in-flight electrical work** (all currently uncommitted):
- `src/beast/serialize.rs` + `deserialize.rs`: `PinDecl` round-trip, plus
  the restored slots/metadata parse loop (the old flat-parts loop never
  matched the nested emit shape — typedef BEAST round-trips silently lost
  slots AND metadata; test `test_roundtrip_typedef_pins` covers all three).
- `src/analysis/electronics.rs`: voltage drives from
  `[x.voltage == <literal>]` obligations (pre or post), disagreeing
  drives = shorted-supply violation, `VoltageCheck` on the netlist,
  `format_volts` diagnostics.
- `src/parser/statements.rs`: float metadata values
  (`Some(Token::Float(f)) => PropertyValue::Float(f)` in
  `parse_metadata_value_standalone`).
- Five new analysis tests (drive derivation, overvoltage, shorted supply,
  unrated-unconstrained, agreeing drives).

Gate: `cargo test --lib` green, then commit.

---

## Phase A — The #Category Blast

Goal: fundamentals are base type + protocol; category hashwords die as
surface syntax. `Float<Posit>` works; `#Float<Posit>` is a compile error
that names the fix. Evidence anchors from the 2026-09-11 survey.

### A1 — Parser: fundamental-with-variant, and the hard error

- `parse_type` (`src/parser/types.rs:29-105`): teach
  `Float<Posit>` / `String<C_String>` — identifier + `<ident>` where the
  identifier is a fundamental parses as (fundamental, variant). Today
  `Float<Posit>` is unparseable (width path covers `Int|UInt|Bool` only,
  types.rs:85-105).
- Fix bare-with-variant parent misroute: `type X: String<C_String>` goes
  to `traits` instead of parent (definitions.rs:2130-2133) — route to
  parent like `#String<C_String>` does.
- **Hard error**: any `#Cat` category hashword in a type, parent, op-param,
  or variant position is a parse error naming the fix:
  `"#Float is retired — write Float"` (variants: `"#Float<Posit> is
  retired — write Float<Posit>"`). No alias. (Decision: hard error,
  2026-09-11.)
- Deduplicate the default-variant tables: `types.rs:56-61` and
  `expressions.rs:761-774` (`simple_type_from_name`) carry the same
  `#String→UTF8 / #Float→IEEE754 / #Char→unicode` copy — one table.
- `#` spellings that are NOT categories are untouched: `#System`, `#Link`,
  `#Lh/#Rh/#T`, field markers (`#Stack`/`#Heap`/`#Scalar`), `#pragma`.
  `#Bits` is NOT a category hashword — it is a lane key; see A5.

### A2 — Casting graph: Applied-variant peel

- `type_to_protocol` (`src/casting/graph.rs:978-1070`): peel
  `Type::Applied(fundamental, [variant])` → `(fundamental, variant)`
  before the universe walk. Today `Float<Posit>` resolves to
  `(Float, "")` — the variant silently drops into the IEEE754 default
  lane (graph.rs:1005, `universe_key()` = "Float").
- `parse_protocol_base` (graph.rs:1075-1079) already accepts bare
  `Cat<Variant>` strings — the lane key format needs no change.
- Remove the `HashWord`/`HashWordVariant` arms (they die with A4).

### A3 — Op coverage on bare params

Op signatures change from `op Add(#Int, #Int)` to `op Add(Int, Int)`.

- `param_covers` (`src/typechecker/mod.rs:233,348,472-480`): today
  requires `Type::HashWord(param)` for protocol coverage (`#Bit`
  universal at 475). Re-derive the category from the universe for bare
  params instead of shape-matching: `Custom("Float")` routes through
  `operand_implements_protocol`. `protocol_binding` keys are already
  bare (`operators.rs:90-112`); `protocol_category` already strips `#`
  (operators.rs:123-127).
- `validate_constraints` (`src/pipeline.rs:1015-1046`): bound/op-param
  equality check switches from `HashWord` string equality to bare-name
  equality.
- `matches_form` (`mod.rs:686-704`): Parse discriminators accept bare.
- `compile.rs:1099` is already dual (`"#Bit" || "Bit"`) — collapses to
  bare.

### A4 — Delete `Type::HashWord` / `Type::HashWordVariant`

`src/ast/types.rs:66,70`. 19 files, 85 references, ~40 match arms:
`analysis/boundary_ownership.rs` (15), `parser/definitions.rs` (11),
`casting/graph.rs` (10), `backend/spirv/builder.rs` (8), `typechecker/mod.rs`
(5), `pipeline.rs` (4), `ast/display.rs` (4), `analysis/frgn_dispatch.rs`
(4), `interpreter/casts.rs` (4), rest 1-3. Everything flowing through the
variants is a category or legacy `#Bits` — the out-of-scope hashwords
never used them. After A1-A3 no production path constructs them; delete
the variants and all arms. The AST display formatter (display.rs) loses
the `#` spellings with them.

### A5 — Internal literals to bare keys

- `#Bits` cast-lane keys → `Bits`: `src/analysis/frgn_dispatch.rs:340`,
  `src/analysis/layout_optimizer.rs:255,285-289,589`. (`Type::Bits(N)`
  itself STAYS — compiler construct, Rule 19 exception; only the lane
  KEY spelling changes.)
- `interpreter/casts.rs:31-32,65-66`: `name == "#Bit"` → `"Bit"`.
- `typechecker/mod.rs`: `#Bit` universal (475), `#Float`/`#Int`/`#UInt`
  protocol checks (1667, 1707-1709, 3132), `#String` builtin dispatch
  (5956, 6046, 6078) → bare names.
- Synthetic TypeDef `protocol: Some("#...")` seeds → bare:
  `backend/{spirv,vm,circt,webstack}/normalizer.rs`,
  `backend/register_types.rs:359`, `backend/spirv/mod.rs:847,1030`.
  Consumers already `trim_start_matches('#')` — tolerant both ways during
  migration.

### A6 — Surface migration (~200 occurrences)

By count: `#String` 73, `#Float` 53, `#Int` 45, `#Char` 12, `#Data` 12,
`#Bool` 11, `#Bit` 4, `#UInt` 1, `#Blob` 0.

- `lib/**/*.bv` (~30; glue types.bv: python 9, c 8, rust 6,
  std/string.bv 4, java/go 1 each)
- `lib/glue/*/glue.dbv` protocol keys (~60 `"#String": {...}` style) →
  bare keys (decision 2026-09-11: migrate; consumers strip `#`, so both
  spellings work during the window)
- benchmarks (12), examples (3), tests/ (5), `lib/runtime/briev_rt.c`
  comments
- `lib/compiler/**` as flagged by grep

### A7 — Docs (same-commit rule)

- `spec/SPEC.md`: hashword table (~line 310), proto examples (874-876),
  §1946 list — `#Cat` entries removed, `Fundamental<Variant>` documented,
  the D2 doctrine stated.
- `docs/architecture/hash-words.md`: rewritten — categories are not
  hashwords anymore; the remaining hashword inventory (#System, #Link,
  #Lh/#Rh/#T, field markers, #pragma) documented as the complete set.
- `docs/architecture/`: agent-reference.md, casting-protocol.md,
  protocol-model.md, protocol-types.md, backend-type-dispatch.md,
  bits-thesis.md, primordial-types.md, intrinsics-vs-stdlib.md,
  iterable-protocol.md, glue-ffi.md, ctd-and-alu.md.
- `docs/guides/ffi-and-export.md`, `docs/guides/add-an-ffi-target.md`,
  `learn-briev/` (7 files).
- `syntax-highlighter/syntaxes/briev.tmLanguage.json`: category-hashword
  patterns removed (other hashword rules stay).

**Tests for Phase A**: parser accepts `Float<Posit>`, `String<C_String>`,
bare-with-variant parents; rejects every `#Cat` spelling with the fix in
the message; graph resolves `Float<Posit>` to the Posit lane (not
IEEE754 default); op coverage fires on bare params
(`op Add(Int, Int)` dispatches); `grep -r '#Float\|#Int\|#UInt\|#String\|#Bool\|#Char\|#Blob'`
over src/ (non-test) returns zero; full gate per commit.

---

## Phase B — Electronics Briev in the clean world

Volt is born after the blast: no `#Voltage` ever exists.

### B1 — Parentless-root mechanism

Relax the fundamental gate to pure mechanism (no electrical names in
Rust): a declared type with NO parent is its own casting-graph root.

- `graph.rs:1052-1053` (category gate `"Float"|"UInt"|...`), plus the
  same gate at `operators.rs:148-149`, and `derive_type_protocols`
  (graph.rs:1324, via `FUNDAMENTAL_TYPES` mod.rs:41-45): parentless
  declared types join the root set. `Float` itself is parentless — the
  doctrine covers the existing fundamentals with the same rule.
- No lanes, no LLVM resolvers, no print dispatch, no GPU/SPIR-V seeding
  for electrical categories: those fire only when values EXECUTE, and
  D3 says electronics proves instead. Recorded as deferred (simulation).

### B2 — Prelude SI fundamentals (full set)

`lib/std/electronics.bv` declares, parentless, `spec Bits: 32` provenance
types: `Volt`, `Amp`, `Ohm`, `Farad`, `Henry`, `Hertz`, `Watt`, `Kelvin`.
The `prelude-electronics` plugin injects them — the user never writes the
declarations; Volt is "just there," exactly like Float in `.bv`.
`Pin.voltage: Volt`, `Pin.current: Amp`. Numerals are admitted where a
quantity type is expected (contract positions, clause values) via the
declared-width literal admission (verify the admission path reads
parentless roots' `spec Bits`; if it walks `#Float`-specific code
(mod.rs float-admission), extend it to parentless roots — mechanism, not
names).

### B3 — Uniform property clauses, all four forms

One shared clause helper, called from every declaration body loop:

- `pin a;` / `pin a = 7;` — exists in type bodies (definitions.rs:2164-
  2194); extract and share.
- `reference "D";` — mandatory reference-designator prefix.
- `tolerance 3.3;` (Volt context) / `tolerance any;` — declared rating;
  `any` = declared unrated.
- `in`/`out` remain reserved — clause names are context-sensitive, no new
  global keywords.

Call sites:
- `parse_type_body` (definitions.rs:2102) — has pins; gains
  reference/tolerance.
- `parse_obj_like` (definitions.rs:2585-2690) — body loop at 2635-2673;
  `pins: vec![]` hardcoded at 2687 → real clauses.
- `parse_cell` (definitions.rs:1064, body 1112-1153) — gains all three;
  `CellDef` (ast/top.rs:242-266) gains `pins`/`reference`/`tolerance`
  fields; construction sites updated (mechanical).
- struct body loop (definitions.rs ~2900) — gains the clauses.

**Mandatory rule (parse-time, content-triggered)**: any declaration with
`pin` clauses and no `reference` clause is a parse error — what/why/fix:
`"component 'Led' declares pins but no reference clause — every
schematic symbol needs a reference prefix (e.g. reference \"D\");"`.
Content-triggered, not family-config: pins anywhere mean electronics
content.

**Metadata deletion**: the `!> Reference` / `!> Tolerance` lookup paths
in `src/analysis/electronics.rs` (`collect_type_pins`) are deleted —
structural clause fields only. `PropertyValue::Float` metadata support
(Phase 0) stays — it is general parser capability, not electronics.

### B4 — Proving (the electronics state machine)

Existing from Phase 0: drives from `[x.voltage == literal]`, disagreeing
drives = shorted-supply violation. Adds:

- **Tolerance enforcement**: driven net + rated pin below class = error;
  driven net + unrated pin = error unless the type declared
  `tolerance any` (decision 2026-09-11: unrated is a declared decision,
  never a silent omission).
- **Ohm's-law flagship**: from the drive class and a series Resistance
  value, derive the current class (I = V / R through series resistors)
  and PROVE numeric-bound postconditions
  (`[d1.a.current <= 0.02]` compiler-proven, not asserted). This is the
  literal expression of D3. Kirchhoff/parallel laws are follow-ons.
- Comparisons already typecheck: same-type obligation operands are legal
  for any custom type (typechecker mod.rs:2582-2653) — no new comparison
  machinery.
- All diagnostics follow house style: what / why / fix, B-coded where
  they join the validator family.

### B5 — Catalog + demo

- `lib/std/electronics.bv`: the Device-set — R, C, L, D, LED, J, SW —
  passives as structs, scriptable parts as cells (catalog convention
  only; D1 forbids form semantics). Values typed
  (`value: 330` in Ohm context).
- Demo `examples/electronics/led_blinker.ebv`: JST connector → 330 Ω
  series → LED; one `txn powered` with topology preconditions, one
  `3.3` Volt drive, postcondition current PROVEN via V=IR; compiles to
  `.kicad_sch` that opens in KiCad. Backend interface unchanged
  (consumes `AnalysisResults.electronics`).

### B6 — Docs

- `spec/SPEC.md` §3.5 rewritten: D0-D6 doctrine, fundamentals table,
  uniform clauses, mandatory reference, proving model, precedence trap.
- `docs/architecture/electronics-frontend.md`: rewrite for the new world
  (parentless roots, clause fields, V=IR proof path).
- Plan amendment 3 on the 2026-09-09 doc pointing here.
- learn-briev electronics chapter if present.

---

## Sequencing & commits

Phase 0 → A1 → A2 → A3 → A4 → A5 → A6 → A7 → B1 → B2 → B3 → B4 → B5 →
B6. One commit per step where green; A1-A3 may batch if intermediate
states cannot hold the gate. Full gate per commit: `cargo test --lib`,
Praetor on changed dirs, conformance sweep (covers `examples/`
automatically). Phase A is a language-surface change: SPEC + highlighter
land within the phase, not after.

Rule 12b applies if any benchmark-visible path changes (it should not —
the blast is surface syntax + dispatch keys, not codegen strategy).

## Decision ledger (what supersedes what)

| # | Decision | Supersedes |
|---|---|---|
| D1 | `<->` dead; nets inferred from contracts | 2026-09-09 plan §4.3 |
| D2 | Pins = first-class keyword, all four forms | `!> Pins` metadata; "retire from type bodies"; "obj=passive/cell=active" |
| D3 | reference/tolerance = named clauses | `!> Reference` / `!> Tolerance` metadata |
| D4 | Fundamentals = base type + protocol; `#Cat` blasted, hard error | the `#Float` category-hashword choice |
| D5 | `Float<Posit>` variant syntax | `#Float<Posit>` |
| D6 | Electrical bases parentless, prelude-injected, full SI set | "root Volt in the Float protocol" |
| D7 | Provenance-only quantities | "declare spec Bits representation now" |
| D8 | Tolerance: error unless `tolerance any` | warning option |
| D9 | Ohm's-law proof in scope | follow-on |
| D10 | glue.dbv keys migrate bare | leave-as-is option |

## Deferred ledger (recorded, not built)

- Unit-suffix literals (`3.3V`, `20mA`) — sugar over typed contexts.
- Per-pin tolerances (`pin a [3.3];`) for mixed-voltage ICs.
- Named nets (opt-in binding for contract/metadata use); nets are N1..Nn.
- Board-as-obj containers (`obj Blinker` holding instances + member txns).
- Config-driven required-clause registry per family (mechanism exists as
  parse-time content-trigger; no family config surface).
- LLVM/GPU/SPIR-V representation of electrical categories, print/ISR/
  webstack/GLUE category fallthrough sweep — waits for simulation
  (D3: nothing executes yet).
- Dimensional algebra beyond Ohm's law (V·A→W, Kirchhoff) — analysis-side
  growth; may need declared dimension metadata later.
- GLUE ABI conversion tables keyed `#<Category>` (glue/export.rs) — kept
  tolerant; full bare-key migration lands with A6's config pass where
  flagged.

## Execution notes (traps already identified)

- `in`, `out` are reserved — never use as pin/field names in fixtures.
- `=` binds LOOSEST (below `&&`); conjoined obligations use `==`
  (expressions.rs:19-34 vs 47-70).
- The conformance sweep compiles everything under `examples/` — a failing
  fixture there breaks the gate; violation fixtures live in tests.
- `git checkout --`/`restore` destroy work — never; targeted `git add`
  only; commit after each green step.
- Praetor `--target` takes a DIRECTORY.
