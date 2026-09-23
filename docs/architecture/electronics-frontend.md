# Electronics Briev — contract-inferred netlist

**2026-09-11 (Part C skeleton).** How `.ebv` sources become KiCad schematics,
and the two design decisions that shape the frontend.

## The two decisions

1. **No connection operator — nets are inferred from contracts.** The
   2026-09-09 plan originally proposed `r1.pin(1) <-> led1.pin(2);`. Replaced:
   because Briev is declarative and obligations already state how nodes
   relate, pin connections are *inferred from precondition equalities*, and
   nets are their transitive closure (union-find). What triggers what comes
   from the existing reactor trigger analysis for free.
2. **Pins are first-class, not metadata.** `!> Pins: [1, 2];` metadata was a
   shortcut; pins now have a keyword, mirroring `.sbv` cells' structural
   `input`/`output` ports. `pin a;` (auto-number: highest-so-far + 1) or
   `pin a = 7;` (explicit datasheet number, ≥ 1, unique per type).

## Pipeline

```
.ebv source
  → lexer (`pin` token, src/lexer.rs)
  → parser: PinDecl on TypeDefBody (src/parser/definitions.rs, parse_type_body)
  → typechecker: type_pins map, field-resolution only
     (src/typechecker/mod.rs — r1.a resolves to the prelude Pin type;
      struct literals never accept pins)
  → analysis: netlist derivation (src/analysis/electronics.rs)
       - instances: top-level `let n: T = T { … }` where T declares pins
       - edges: Eq-of-pin-access nodes in PREcondition trees
       - nets: union-find groups, named N1..Nn deterministically
       - dangling pins: hard diagnostics (what/why/fix)
  → AnalysisResults.electronics  (frontend-driven dispatch: computed ONCE)
  → backend: KiCad 7 schematic emission (src/backend/electronics/mod.rs)
       - consumes the netlist, never re-derives it
       - no type-name matches: geometry from pin lists, prefixes/values
         from `!>` metadata and instance literal fields (Rule 15)
       - refuses to emit an incomplete board (dangling → compile error)
```

## Precedence trap (SPEC §3.5)

Single `=` binds LOOSEST (assignment level, expressions.rs `parse_assignment`);
`&&` sits above it. So `a = b && c = d` parses as `a = (b && (c = d))` —
wrong tree for conjoined obligations. `==` sits at equality level, ABOVE
`&&`: conjoined topology obligations must use `==`.

## Deliberate scope lines

- Contract expressions are **proven, never executed** — comparisons and
  float literals are part of the electronics capability surface; executable
  surface (calls, statement bodies, runtime, concurrency) is not.
- Symbol graphics are generic boxes computed from pin counts. Real
  footprints/graphics stay KiCad-library data (config), not compiler
  knowledge.
- The `pin` clause is implemented for `type` bodies; obj/struct bodies are a
  follow-on if a need appears.

## Deferred (recorded, not built)

Electrical pin roles (`[power_in]`), footprint validation against KiCad
libraries, serializer support for `PinDecl` (deserialize defaults to empty
— beast round-trip of pins is a follow-on), per-pin tolerances, LLVM/GPU
representation (awaits simulation), PinDecl in cell bodies' beast
serialization, `derive` clauses on types (2026-09-23 — recorded in the
hardware-dialect ledger as OPEN after the E14a gate; the fixture states
the obligation as a use-site txn postcondition).

Landed since the list was written: named nets (`net <name>:` — 2026-09-12),
unit suffixes (`3.3V`/`20mA`/`330R` — 2026-09-12), instance arrays
(`let r[i:16][j:8]` — 2026-09-23, E1, Slice 1), pin-level drive intents
(`inst.pin = true;` — 2026-09-23, E14a gate), the E13 decoupling
convention, the E14a intent-completion machinery, and per-prefix
reference designators. The §3.2 USB-sensor gate fixture
(`examples/electronics/usb_sensor.ebv`) compiles to a `.kicad_sch`.

## Tests

- Parser: auto/explicit/high-water numbering, duplicate rejection,
  non-integer rejection (`parser::definitions` pin tests).
- Analysis: chain derivation, transitive closure, postcondition exclusion,
  dangling diagnostics, deterministic naming, non-electronics default
  (`analysis::electronics`).
- Backend: balanced S-expr, per-type designators, property mapping, dangling
  refusal, deterministic emission (`backend::electronics`).
- Demo: `examples/electronics/led_blinker.ebv` → `.kicad_sch` opens in KiCad.

---

## 2026-09-11 (fundamentals doctrine): the as-built world

Phase B of `2026-09-11-fundamentals-doctrine-and-electronics.md` landed:

- **Bases**: `Volt`/`Amp`/`Ohm`/`Farad`/`Henry`/`Hertz`/`Watt`/`Kelvin` are
  parentless declared types in `lib/std/electronics.bv`, self-rooted through
  the B1 mechanism (a parentless type registers base = own name; both
  category walks treat a self-base as the root). No name tables, no Float
  inheritance. Provenance-only: nothing executes.
- **Clauses**: `pin` / `reference` / `tolerance` parse via shared helpers in
  `parse_type_body`, `parse_obj_like`, and `parse_cell`. `reference` is
  parse-mandatory when pins exist. The `!> Reference`/`!> Tolerance`
  metadata path is DELETED — analysis reads `td.body.reference`/
  `td.body.tolerance` and `CellDef.pins/reference/tolerance`.
- **Proving**: drives from `[x.voltage == literal]`; shorted supplies;
  tolerance enforcement (rated-below-class violates; no-clause on a driven
  net violates; `any` never does); **Ohm's-law derivation** — I = V/R
  through two-pin valued parts, downstream current class proven against
  postcondition bounds. Derivations are recorded in
  `VoltageCheck.proved`.
- **Backend**: refuses electrically-violated boards like dangling ones.
- **Struct bodies** intentionally lack the clauses until a struct-declared
  component needs to exist (clauses that parse but land nowhere are dead
  surface).

**2026-09-11 (static physics completion):** the derivation is a fixpoint
over the part graph — series current (I = |ΔV| / R across the class
difference, so a stated 0 V ground participates), voltage dividers
(general two-resistor form on unclassed two-attachment nets with
different classed far sides), and Kirchhoff current summing (a net's
class is the SUM of contributing branches, replacing worst-case max).
Everything records as proof facts.

Also landed the same day: **library-level mutable state** — top-level
`let`s now import (item_name names them), which unblocked the buffered
stdout lane in cast_lanes.bv and closed the fasta ~100x regression
(BUGS.md 2026-09-11; fasta now 0.73x vs baseline).

**2026-09-12 (readability layer):** named nets — `net <name>:` prefixes a
precondition conjunct and names the equivalence class (`Expr::Named`,
parsed in `parse_and`/`parse_and_lhs`; names recorded per union-find root;
keywords are valid names since net names are contextual). Unit suffixes —
`3.3V`, `20mA`, `330R` (`Expr::UnitLiteral { value, unit }`, parsed after
adjacent numeric literals when `is_unit_suffix` matches; `mA` → /1000;
interpretation in `extract_voltage`/`extract_current`/`extract_resistance`;
instance values store the raw suffixed string for `parse_ohms`).
Names resolve against FINAL union-find roots (stale-root safe); two
DIFFERENT names on one node are a hard error refusing emission
(`net_conflicts`, checked like dangling pins); the same name twice is
redundant, not a conflict.

**2026-09-12 (power ratings):** `rating 0.25;` / `rating any;` on
type/obj/cell bodies. After the current fixpoint, `derive_power` computes
P = ΔV²/R for every valued two-pin part with both endpoints classed:
exceeding the declared rating is a violation; proven dissipation with no
clause is an undeclared decision; within-rating records a proof fact.
One-sided parts (no proven ΔV) and zero-drop straps force nothing.

Deferred: per-pin tolerances, LLVM/GPU representation (awaits
simulation), PinDecl in cell bodies' beast serialization, pin roles,
`derive` on types, bus assembly, en drive solving, rail inference,
value-aware resistor matching (E14b slices 3+).

**2026-09-23 (E14b slice 2, plan `2026-09-23-ebv-e14b-lowhold-forcing.md`):**
low-hold forcing. A MAX obligation (`inst.pin.voltage <= V;`) forces a
path to the return rail: through a free switchable part (Path-class pins;
the return side may already be wired by a guard equality) or directly for
an isolated net; a pulled-up net without a switchable part is a hard
error. The button node of `usb_sensor.ebv` uses the pure-intent form.

**2026-09-23 (E14b slice 1, plan `2026-09-23-ebv-e14b-pullup-forcing.md`):**
min-voltage pull-up forcing. A node body may state
`inst.pin.voltage >= <literal>V;` — a declarative obligation the analysis
consumes. The forcing pass wires a free `spec PullUp: true` part between
the obligation net and the lowest qualifying driven supply rail; D13
ambiguity rules apply when the pick matters (distinct-value parts, or
rails tied at the minimal volts). The i2c node of `usb_sensor.ebv` uses
the pure-intent form.

**2026-09-23 (E14a gate, plan `2026-09-23-ebv-gate-fixture.md`):** the
§3.2 USB-sensor fixture compiles to a `.kicad_sch` and its four-case
error matrix passes. The gate surfaced general fixes — see the hardware-
dialect ledger Amendment 2026-09-23: the prelude-electronics class
injection into import-less sources, bare indexed-pin resolution
(`u2.gpio[0]`), pin-level drive intents (previously a silent drop),
declarative-intent admission + instance-array base names in the
typechecker, per-prefix reference designators (duplicate references are
invalid KiCad), and the present-state (unpop-excluded) decoupling check.
The demo artifact is `examples/electronics/usb_sensor.ebv`.
