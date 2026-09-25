# Plan 2026-09-23: the `fab` layer — physical board (placement + routing) for `.ebv`

**Status:** building — Slice 1 in progress
**Branch:** `feat/e14a-intent-synthesis`

## Context

The schematic/physics layer states what the board *is* (types, pins,
instances, obligations, netlist — the conceptual truth the compiler
proves). The `fab` layer states what the board *looks like* and how it is
built (outline, layer, part positions, rotation) — physical, non-
conceptual. This deliberately picks up part of the D15 non-goal ("PCB
layout") as a recorded revision; impedance, trace-length, and
transient/time-domain analysis stay non-goals.

The pattern follows `.rbv`: the view is an in-file, attached section
(`TopLevel::RenderBlock`, `src/parser/definitions.rs:206`) declaring the
presentation bound to the conceptual layer. `fab` is the same shape for
physical layout. It sits on the annotation side of the annotation-vs-
physics doctrine: the compiler *carries* the placement and *proves* what
is provable (containment, clearance), like it proves the schematic.

## Doctrine fit

- **Placement = the author's control** (Rule 2: pinned positions override;
  everything else auto-placed deterministically — a good default, never a
  keyword beaten by the author's placement).
- **Routing = compiler machinery** — no routing control surface; the
  compiler routes the copper. The author places parts; the compiler makes
  the board.
- **Provable vs choice:** containment/clearance are provable from the
  declared outline + footprints + positions → the compiler proves them
  (errors/warnings). Aesthetic/signal-integrity placement is choice → the
  author pins it.
- **Footprint geometry is data, not compiler knowledge** (Rules 14/15):
  a Data Briev `.dbvl` file, keyed by package id, never hardcoded.

## Decisions (locked 2026-09-23)

1. **Placement is the only user control; routing is compiler machinery.**
   Routing lands in a follow-on slice (the riskiest piece).
2. **In-file** `fab { … }` top-level section (mirrors `render { … }`).
3. **Deterministic auto-placer** for unplaced parts; explicit `place`
   overrides.
4. **Checks:** off-board containment = hard error; clearance = warning.
5. **Footprint library in `config/footprints.dbvl`** — Data Briev format
   (`include_str!`-baked, loaded via `crate::dbriev::config_db`, same
   pattern as `targets.dbvl`). `package: "0603"` looks it up; unknown
   package = compile error. The footprint DSL must fit the `key: value;`
   dbvl grammar; a small numeric-tuple value extension if geometry needs
   it (design against `config_db`).
6. **Units:** extend `QuantityDim` with `Length` (mm base) + `mm`/`cm`
   suffixes; positions are bare, dimension-checked quantities
   (`place u1 at (20mm, 10mm)`), consistent with quantities Phase 1.
7. **Board shape:** rectangle outline only (`board 30mm x 20mm;`);
   polygon/keep-outs later.

## Slices

1. **Footprints** — `Length` dimension + `mm`/`cm` units; `config/
   footprints.dbvl` (pads, outline per package id); loader + unknown-
   package compile error; emitter footprint output (closes the footprint
   half of E5).
2. **`fab` grammar + AST** — new `fab` keyword; `board W x H;`,
   `place <inst> at (<x>, <y>) [rot <deg>];`; `TopLevel::FabBlock`
   (mirrors `RenderBlock`).
3. **Auto-placement** — deterministic, sorted: decouplers near their
   decoupled part, connectors at board edges, remaining parts in a compact
   grid. Pinned wins; unplaced flows.
4. **Board emitter** — `.kicad_pcb`: outline, footprints at (x, y, rot),
   nets.
5. **Layout checks** — containment (error), clearance (warning).
6. **Auto-routing** (follow-on) — minimal deterministic two-layer
   point-to-point grid router over the placed footprints.
7. **Docs** — design record D15-reversal note; ledger (new gap, close the
   footprint half of E5); frontend deferred; SPEC; fixture gains a `fab`
   section.

## Gates (per commit)

Fixture + `fab` → a `.kicad_pcb` KiCad imports · byte-deterministic
across runs · off-board part → error · unknown package → error ·
`cargo test --lib` green · Praetor no new diagnostics in changed files.

## Doc maintenance

This plan; the design record D15 non-goal revision; the hardware-dialect
ledger (new gap entry + E5 footprint half closed); `electronics-frontend.md`
deferred list; SPEC notation (Length units, `fab` section); fixture gains a
`fab` block.