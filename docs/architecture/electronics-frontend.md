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

Named nets (opt-in binding for contract/metadata use), electrical pin roles
(`[power_in]`), unit suffixes (`2A`, `3.3V`) as literal syntax, footprint
validation against KiCad libraries, serializer support for `PinDecl`
(deserialize defaults to empty — beast round-trip of pins is a follow-on).

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

Deferred: per-pin tolerances, LLVM/GPU representation (awaits
simulation), PinDecl in cell bodies' beast serialization, power ratings
(needs a `rating` clause surface), named-net conflicts (two names on one
net currently take the first — should be an error), pin roles.
