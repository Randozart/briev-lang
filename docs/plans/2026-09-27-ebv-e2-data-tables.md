# E2 — data tables feeding instance values

Date: 2026-09-27
Gap: `2026-09-21-hardware-dialect-gaps.md` E2 (data tables)
Status: OPEN — plan only; slices pending

## Problem

`collect_instances` (`src/analysis/electronics.rs:1060`) carries instance
`value` fields only as **literals** — `Quoted`/`Decimal`/`Float`/
`UnitLiteral` — into `ComponentInstance.properties` (a `Vec<(String,
String)>`). A crossbar tile (512 weights) cannot state 512 distinct
resistances by hand: the values must come from a **table** the emitter
walks, not 512 hand-written literals. E1 landed *mass instantiation*
(instance arrays — how many parts); E2 is the *data* half (what value each
part carries). T1 (the GGUF → instance-table tool) is out-of-tree; E2 is the
compiler surface T1 emits into.

## What exists (the family this extends)

- `PropertyValue` (`src/ast/types.rs:201`) already carries `Int`/`Float`/
  `String`/`Quantity`/`List(Vec<PropertyValue>)` — the table-cell type.
- `collect_instances` (`:1060`) maps instance-literal fields to string
  properties; the BOM (`generate_bom`, `src/backend/electronics/mod.rs:308`)
  reads `value`/`package`/`jlcpcn` from those properties.
- E1's instance-array desugar (parse-time, per-element `ComponentInstance`)
  is the exact pattern E2's table-lookup resolution mirrors: resolve to a
  literal at parse/analysis, keep `ComponentInstance` one-per-element.

## Design decisions

- **D1 — surface: a `data` constant table.**
  `data r: Int[16][8] = [ [ 1, 2, … ], … ];` declares a named
  constant-table binding. 1-D and N-D forms; cells are integer literals
  (the first class; float/string cells are a follow-on). The table is a
  *compile-time constant* — it is a data declaration, not a runtime value.
- **D2 — the table feeds a `value` position by index.**
  `let r[0]: Resistor = Resistor { value: r[0][0]; … };` — a `value`
  (and `package`) field may name a table element `table[idx…]`. The
  element must be a literal cell; a non-literal or out-of-range element is
  a hard error naming the table and index. This is the weight-matrix
  population: the table carries the values, the instance array (E1)
  carries the parts, the index binds them.
- **D3 — resolution is at analysis, not parse.** The table is a top-level
  `data` binding; `collect_instances` resolves a `value: table[i][j]`
  field to the cell's literal string (the same string a hand-written
  literal would produce). `ComponentInstance` stays one-per-element; the
  BOM/emitter are unchanged.
- **D4 — determinism.** Tables are value-ordered (declaration order); the
  emission walks them in declaration order, never `HashMap` order. Byte-
  deterministic across runs (the E1 determinism rule).
- **D5 — population *decisions* stay in the generator (T1).** The table
  carries only *values*. "Is this pad populated?" is a `unpop`/`populated`
  decision owned by the generator program (T1) — E2 does not invent a
  population predicate.
- **D6 — no compiler knowledge of "resistor"/"weight".** The table is a
  generic `Int` matrix; the instance type reads a cell the same way it
  reads any literal. The compiler knows no part vocabulary (Rule 15).

## Slices

1. **Parser + AST**: a `data name: Type[ dims ] = [ cells ];` binding → a
   new `TopLevel::Data` item (name, declared dims, `Vec<PropertyValue>`
   cells in row-major order). 1-D and 2-D first; N-D parses as chained
   dims → flattened row-major (the E1 multi-dim pattern). Cells must be
   integer literals for the first class. Tests: a 2×3 table parses with the
   right cell count/order; a malformed cell or ragged row is a hard error.
2. **Resolution**: `collect_instances` resolves a `value: table[i][j]`
   (and `package:`) field to the cell's literal string; a missing table,
   a non-literal cell, or an out-of-range index is a hard error naming the
   table and index. A flat `let` with a table-cell `value` emits the cell
   as the part's value in the BOM. Tests: a 4-part resistor array fed from
   a 1×4 table emits the four cell values in the BOM, byte-identical to the
   same board written with hand-written literals.
3. **Gate fixture + docs closure**: an `r[2][2]` tile fed from a `data`
   matrix; the BOM carries all four cell values; byte-deterministic across
   two runs; plan status → landed, gap E2 → CLOSED.

## Gate (from the gap entry)

Table-driven emission compiles; byte-deterministic (sorted iteration —
determinism rule). The compiler gate: a table-fed instance array emits a
BOM byte-identical to the same board written with hand-written literals,
and two runs of the same file are byte-identical.
