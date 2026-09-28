# E3 — mass wiring (`[*]` wildcard) + hierarchical KiCad sheets

Date: 2026-09-28
Gap: `2026-09-21-hardware-dialect-gaps.md` E3 (single A4 sheet, channel-routed
wires); E11 whole-bus residue; E1 plan line 50–52 (`r_pu[0..=1]` instance
buses claimed but never implemented)
Status: DONE
Branch: `feat/e14a-intent-synthesis`

## Problem

The emitter places instances on ONE A4 sheet in two columns
(`PLACE_X_LEFT`/`PLACE_X_RIGHT`, `PLACE_PITCH = 25.4`). Two consequences:

1. **Geometry**: 512 instances → `y = 50.8 + 511*25.4 ≈ 13 m` tall on an A4
   sheet. Loads (KiCad does not clip to paper) but is unusable — the gap
   statement's "thousands of instances unusable in KiCad".
2. **Wiring**: the only way to wire N instances today is N hand-written
   precondition equalities. There is NO bulk form.

The E3 gate is *"a 512-instance tile opens ERC-clean in KiCad"*. Two halves:

- **Authoring half** — express 512 parts + their common wiring compactly.
  Mass instantiation landed (E1: `let t[i:16][j:32]`). Mass wiring did NOT.
- **Emission half** — split 512 instances across hierarchical sheets so the
  board is human-usable. Authoring-side hierarchy is owned by CELLS (design
  record D10); the multi-sheet *emitter* is this plan's scope.

## What exists (the family this extends)

- **Instance arrays (E1, `2026-09-23`)**: `let t[i:16][j:32]: T = T { … };`
  parses to per-element `ComponentInstance`s named `t[0][0]`…`t[15][31]`
  (row-major, last index fastest). `ComponentInstance` stays one-per-element;
  netlist/emitter are array-blind (SPEC §5, "Instance arrays").
- **Whole-bus equality (E11, `2026-09-22`)**: `u2.gpio[0..=3].voltage ==
  u3.data[0..=3].voltage` expands element-wise. `expand_bus_pair`
  (`src/analysis/electronics.rs:3100`) requires BOTH sides range-indexed and
  rooted at a **pin array** (`range_index_pin:3144` needs base
  `Field(inst, arr)`).
- **Per-element wiring works**: `t[0][0].a.voltage == j1.p1.voltage` resolves
  through `resolve_pin` → `instance_array_name` (`:1289`).

### The actual gaps (verified by reading the code, not memory)

- `range_index_pin` (`:3144`) matches only `Field(Index(Field(Ident(inst),
  arr), Range), "voltage")` — a **pin array on a concrete instance**. An
  instance-array root (`t[0..15][0..31]`, shape `Index(Index(Ident, Range),
  Range)`) returns `None`.
- `expand_bus_pair` (`:3106`) requires BOTH sides ranged. `scalar == ranged`
  → `BusPair::NotBus` → single-pin fallback. **Broadcast is inexpressible.**
- `[*]` has vocabulary precedent only: `2026-09-23-ebv-e14b-drive-assignment.md:33`
  ("solver wires `r_pu[*]`"). It does not parse today.

## Design decisions

- **D1 — wildcard `[*]` = every element.** On an instance array
  (`t[*].a.voltage`) it ranges every element (any dims, declaration order);
  on a pin array (`u2.gpio[*].voltage`) it ranges every element pin. `[*]`
  is "all"; `[lo..hi]` stays *a bound* (Rule 21 delimiter-load: `[]` =
  containment/bound; `*` = the wildcard inside it). Mirrors the E14b doc's
  `r_pu[*]` intent. Chosen over explicit `t[0..15][0..31]`, which makes the
  author restate the `let`'s dims.
- **D2 — broadcast.** `scalar == wildcard` unions the scalar with EVERY
  expanded element (the rail-to-bank case). Symmetric in either operand
  position. This is the general form of the E11 rule (which was
  both-sides-ranged, pairwise).
- **D3 — element-wise for wildcard-vs-wildcard.** `t[*].a.voltage ==
  s[*].a.voltage` pairs elements by flattened index (same shape required;
  length mismatch is a hard error). Extends the E11 pairwise rule to
  instance-array roots.
- **D4 — pure expansion, analysis-time.** Like E1/E11: the netlist stays
  array-blind. `[*]` produces the same `PinRef` unions the hand-unrolled
  equalities produce. No runtime, no new netlist type.
- **D5 — no sheet syntax.** The emitter decides sheet grouping
  deterministically (declaration order, fixed bank size). Authoring-side
  hierarchy is a CELL concern (D10); the gate fixture declares a flat tile.
- **D6 — no compiler vocabulary.** `TileRes` is a generic pin-bearing type;
  the compiler knows no "resistor". The fixture mirrors `data_table_tile.ebv`
  (no constitutive law → no DC solve → ERC-clean).

## Slices

1. **Wildcard parser + AST** — `Expr::Wildcard`; the `[*]` index form parses
   (`src/parser/expressions.rs` index branch). `Display` renders `[*]`.
   Tests: `t[*]` and `u2.gpio[*]` parse to `Index(base, Wildcard)`.
2. **Analysis expansion** — `wildcard_bus_access` (two shapes: pin-array
   `Field(Index(Field(inst,arr),W),"voltage")`; instance-array
   `Field(Field(Index(Ident(arr),W…),"pin"),"voltage")`) +
   `expand_wildcard` → `Vec<PinRef>`; wire into `collect_pin_unions`
   (D2 broadcast, D3 pairwise). Determination: numeric-aware element order.
   Tests: scalar-broadcast unions all elements; pin-array wildcard unions
   all element pins; wildcard-vs-wildcard pairwise; length mismatch errors;
   the netlist equals the hand-unrolled form.
3. **512-tile gate fixture** — `tests/electronics/tile_512.ebv`:
   `let t[i:16][j:32]: TileRes = TileRes { value: "100R", package: "0603" };`
   + one connector, wired by two wildcard broadcast equalities. Emits a
   single sheet first (proves authoring); ERC-clean.
 4. **Hierarchical sheet emission** — `generate_hierarchical(netlist) ->
    Vec<(filename, content)>`. Partition the name-sorted components into banks
    of `BANK_SIZE = 64` (declaration/sort order). If `components.len() <= 64`,
    return the existing single sheet (byte-identical — no regression for small
    fixtures). Else emit a master + one child per bank:
    - **Child `tile_<i>.kicad_sch`** — header (sheet uuid = `uuid("sheet:<i>")`),
      `lib_symbols` for the types used in that bank, the bank's instances
      (placement restarts at `PLACE_Y0` per bank), and per net: *local* nets
      (all pins in-bank) route as wires (existing `emit_net`); *cross-sheet*
      nets emit a `(global_label "NET" (shape input) (at <pin>) …)` at each
      in-bank pin. Same label name across banks = connected.
    - **Master `tile.kicad_sch`** — header, one `(sheet "<stem>_<i>.kicad_sch"
      (at …) (units mm) (page_id N) (uuid …))` per bank + a `sheet_instances`
      table (root path = master uuid + one entry per child uuid). No instances,
      no lib_symbols.
 5. **Gate + docs closure** — `kicad-cli sch erc` on **each child**: 0 errors
    (kicad-cli cannot traverse hierarchy — see finding below, so the master is
    GUI-verified only); `cargo test --lib` green; two runs byte-identical
    (all files); gap E3 → CLOSED; ledger + design-record refresh.

## Finding: kicad-cli does not traverse hierarchical sheets (2026-09-28)

`kicad-cli sch erc` (10.0.6) **refuses to load any schematic containing a
`(sheet …)` reference** — "Failed to load schematic" on a master with even one
`(sheet …)` line; the identical master minus that line loads and ERCs clean.
Verified by bisection: the `(sheet …)` block alone is the load blocker; the
master's own content (header, labels, `sheet_instances`) is valid. This is a
kicad-cli limitation, not a syntax defect.

Consequence: the "ERC the master" gate is not CLI-verifiable. Each **child**
sheet is self-contained (its rails are global labels that connect to in-bank
pins), so the workable CLI gate is *per-child* `sch erc` = 0 errors. The master
is still emitted for human use in the KiCad GUI (which does traverse
hierarchy); its correctness is structural (Rust tests assert the sheet refs +
`sheet_instances` paths) and is recorded here + BUGS.md as the CLI gap.

## Gate (from the gap entry)

A 512-instance tile opens ERC-clean in KiCad. Compiler gates: `t[*]` netlist
is byte-identical to the hand-unrolled equalities (modulo component-name
order); two compiles are byte-identical; the hierarchical master + child
sheets load and ERC clean.

## Doc maintenance

- `spec/SPEC.md` §5 — `[*]` wildcard element access (both array kinds) +
  the broadcast rule; §"Whole-bus equality" gains the wildcard/broadcast
  sentences.
- `docs/architecture/electronics-frontend.md` — wildcard resolution notes.
- Ledger `2026-09-21-hardware-dialect-gaps.md` E3 row → status.
- `BUGS.md` — only if a defect surfaces.
