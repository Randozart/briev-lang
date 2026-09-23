# Plan 2026-09-23: the `.ebv` gate fixture (E14a closure)

**Status:** building
**Branch:** `feat/e14a-intent-synthesis` (merged with main at `b5fd1a84`)

## Context

The intent-synthesis surface (E1–E7, E11–E14a slices, D16 phases, when-law,
participation, contention, relays, buses) is implemented and green — but the
E14a GATE from the design record (`2026-09-21-intent-synthesis-node-semantics.md`
§3.2, §4) has never been run: no `usb_sensor` fixture file exists. The ladder
says **E14a closes when §3.2 compiles and its error matrix passes**. This
plan closes that gate.

An audit of §3.2 against the landed surface found the fixture text predates
several locked decisions — it is written in *planned* syntax, not shipped
syntax. The fixture must be materialized as a translation, and every place
the translation must state MORE than §3.2 does marks a real delta between
E14a (intent-completion) and E14b (pure intent). Those deltas are recorded,
not hidden.

## Translation table (§3.2 → landed surface)

| §3.2 says | Landed form | Note |
|---|---|---|
| `pin in: power` | `pin in: Power` | E12 classes are stdlib **types** (PascalCase) |
| `spec decouple: 100n;` | `spec Decouple: "100n";` | E13 convention, string literal |
| `populated = false: c_dnp;` | `unpop c_dnp;` | Slice B locked the spelling |
| `let r_pu[2]: Resistor = …` / `let c[i:5]: …` | **E1 lands first** (Slice 1) | instance arrays are gap E1, not yet parsed |
| `derive on: a.current >= 2mA;` | txn **postcondition** at use site | no type-level `derive` clause landed — record as open gap |
| `u1.enabled = true;` | `u1.en = true;` | intent addresses the pin (`In`-class, test-proven at electronics.rs:3202) |
| `node … [usb_powered.up && sw1.closed]` | guard over `trg` + voltage facts | no `.up` / `.closed` members landed |
| io_od pull-up forcing (solver wires `r_pu[*]`) | **explicit equalities** | E14a allows explicit topology; solver-side forcing is the E14b delta |
| `led1 = true;` driving | landed as-is | drive intent, test-proven (`d1 = true`) |
| `type UsbMicro` | defined inline in the fixture | not in std — fixture-local type |

## Slices

### Slice 1 — E1: mass instantiation (bounded instance arrays)
- Syntax: `let r_pu[2]: Resistor = Resistor { … };` and
  `let c[i:5]: Capacitor = Capacitor { … };` (optional index name), parse
  desugar to per-element `ComponentInstance`s (`r_pu[0]`, `r_pu[1]`, …);
  designators suffixed by index (`R1`, `R2`, … per type counter + element).
  Multi-dim (`[i:16][j:8]`) parses as chained dims → flattened indices
  (`inst[i][j]` → element `i*16+j`, references `r[i][j]` resolve through the
  same postfix index path buses already use).
- `ComponentInstance` stays one-per-element — netlist derivation, unions,
  dangling, budgets, unpop all run per element **unchanged** (desugar at
  parse, exactly like pin arrays in E11).
- Instance accesses in contracts/bodies (`r_pu[0].a.voltage`,
  whole-bus `r_pu[0..=1].a.voltage`) resolve through the existing index
  postfix + `range_index_pin` machinery.
- **Gate:** netlist + KiCad emission for `let r[4]: T = …` byte-identical
  (modulo designator suffixes) to the same board written as four flat
  `let`s; length-`0` and out-of-range index are hard parse errors.
- **Effort:** M · **Ledger:** closes E1.

### Slice 2 — the fixture compiles
- Materialize `examples/electronics/usb_sensor.ebv` per §3.2 using the
  translation table. Explicit equalities wherever §3.2 relies on
  not-yet-landed solver forcing (i2c pull-ups, gnd paths) — each such site
  gets a `// E14b:` comment naming the equality the pure-intent form will
  delete.
- Fix every gap the compiler surfaces, **general-fix only** (Rules 14/15/24,
  doctrine: a fixture-shaped match arm is a bug). If a gap needs new
  surface (e.g. a member the guard wants), prefer the landed contract form
  (voltage facts) over new syntax; new syntax only when the contract form
  cannot express it — then it goes through the keyword-audit (Rule 2).
- **Gate:** `derive_netlist` on the fixture: zero errors (intent, class,
  convention, budget, dangling, contention, bus, participation), stable
  net names, KiCad emission succeeds.

### Slice 3 — the error matrix (the other half of the gate)
Four negatives, each a test asserting the hard error names the omission and
enumerates candidates (D13):
1. remove the `led1` driver → intent error with membership candidates;
2. drop the button's gnd path → dangling/membership error with candidates;
3. remove a decoupling cap bridging a supply pin → convention error;
4. `open` naming two already-wired pins → disconnection error.

### Slice 4 — determinism
- Same fixture compiled twice → byte-identical netlist dump + `.kicad_sch`
  (sorted iteration everywhere; designator counters deterministic).

### Slice 5 — E10 hygiene (ride-along, S)
- `docs/architecture/electronics-frontend.md` deferred list refresh (named
  nets / unit suffixes landed; instance arrays now landed; `derive`-on-type
  now open).
- Regenerate `examples/electronics/led_blinker.kicad_sch` from the current
  emitter.

## Ledger / record updates (final commit)
- Hardware-dialect-gaps: E1 closed; new open entry **`derive` on types**
  (owner: fixture; triggers when a second class wants a per-instance
  obligation — property-driven, `spec` form, per D6).
- Design record §4: E14a row marked gate-passed with the delta list
  (what §3.2 needed beyond pure intent = the E14b backlog: solver-side
  io_od forcing, `.up`/`.closed`-style behavior members if E14b keeps
  asking for them).

## Gates (per-commit)
`cargo test --lib` green · no new Praetor diagnostics in changed files ·
every fixture-exposed gap fixed generally, never fixture-specially.

## Doc maintenance
- Ledger + design record updated in the closing commit (above).
- SPEC §3.5 gains instance-array declaration syntax (`let name[N]` /
  `let name[i:N]`) when Slice 1 lands.
- `electronics-frontend.md` deferred list refreshed (Slice 5).
