# Hardware Dialect Gap Registry — Electronics (`.ebv`) + Silicon (`.sbv`)

**Date:** 2026-09-21
**Status:** REGISTRY ONLY. No execution is ordered, no sequencing is committed.
**Drivers:** the ternary-weight hardware concept (analog crossbar
demonstrator, JLCPCB-fabbable) and Project Bachi (Ternary Bonsai 2 27B on
Kria KV260), which doubles as the `.sbv` gap-filling vehicle per the
differential HDL workflow in `../Bachi/AGENTS.md`.
**Evidence method:** every `file:line` below was read against the working
tree at HEAD `65f0fa2b` on 2026-09-21 — not inherited from Bachi's
2026-09-20 audit. Line numbers drift; re-verify the arm when closing an
entry, and record the closure line in the Bachi ledger, not here.

---

## Purpose & Homes

This file is the **compiler-side source of truth** for hardware-dialect gaps.
The **module-facing view** (which Bachi module needs what, plus per-closure
parity evidence) lives in `../Bachi/reports/briev-gaps.md` under its
append-only Amendments section. Two ledgers, one per repo; this one owns
fix sketches and verification gates, that one owns blocking matrices and
evidence protocol results.

Sequencing (what to build in which order) is deliberately NOT here. It will
be written as a separate phase plan when execution is ordered.

## Doctrine guardrails (apply to every entry)

1. **General fixes only.** No entry may be closed with a Bachi-shaped or
   crossbar-shaped special case (Golden Rules 14/15/23/24). The compiler
   must never learn the words "crossbar", "ternary model", "TLMM", or
   "neural". Catalogs, patterns, and algorithm shapes live in stdlib/stdlib
   composites; the compiler keeps proofs, lowering machinery, and expressiveness.
2. **Differential HDL workflow governs silicon closures** (Bachi AGENTS.md):
   hand-written SV reference first, name the gap precisely, fix the compiler
   additively, cross-validate three ways (C ref ↔ hand SV ↔ generated
   Verilog), record per the Bachi evidence protocol.
3. **Gaps are not bugs.** `BUGS.md` is for root-caused defects. Feature
   gaps live here.
4. **Additive only** (Rule 6): new arms, new passes, new constructs —
   never modified existing optimization/emission paths.

## Effort legend

| Tag | Meaning |
|-----|---------|
| S | ≤ half day, focused |
| M | 1–3 days |
| L | multi-day to ~week; touches emitter core semantics |

---

## Electronics track (`.ebv` → analog ternary crossbar)

Context: the crossbar concept is a dual-rail resistor mesh — weight ∈
{-1, 0, +1} becomes "resistor to +bus / resistor to −bus / unpopulated pad";
Kirchhoff sums currents, a differential amplifier subtracts rails. The
language must express *mass, data-populated component layouts*; the
population *decisions* stay in generator programs, never in the compiler.
Honest scale ceiling for an analog crossbar board: ~1k–10k parameters
(≈9k weights ≈ 1.2k 8-packs on a 100×160 mm Eurocard). 27B parameters as
discrete resistors is physically impossible; 27B belongs to the silicon
track below.

### E1 — Mass instantiation absent (flat `let` instances only)
- **Status:** CLOSED 2026-09-23 (Slice 1 + E14a gate; commits `3de75d26`,
  Slice 2–5). Landed surface: `let r_pu[2]: Resistor = …;` and
  `let c[i:5]: Capacitor = …;` (optional index name), multi-dim
  (`[i:16][j:8]`) chained, desugared at parse to per-element
  `ComponentInstance`s; the netlist/emitter/BOM run per element unchanged.
- **Evidence:** instances are single top-level `let`s
  (`examples/electronics/led_blinker.ebv:46-48`); `ComponentInstance`
  (`src/analysis/electronics.rs:31`) carries no index dimensions;
  `derive_netlist` (`:936`) maps instances 1:1.
- **Blocks:** any real crossbar tile (512 weights = 512 hand-written lets).
- **General fix:** bounded-range array-of-instances —
  `let r[i:16][j:8]: Resistor = Resistor { value: "10k"; package: "0402"; };`
  desugars to per-element instances; reference designators suffixed by
  index; union-find netlist derivation runs per element unchanged.
- **Gate:** emitted netlist + KiCad for `i×j` instances identical
  (modulo designator suffixes) to a hand-unrolled reference file; byte-
  deterministic emission. Proven by
  `instance_array_emission_matches_hand_unrolled` and the
  `usb_sensor_emission_is_deterministic` gate test.
- **Effort:** M

### E2 — No data tables feeding instance values
- **Evidence:** no constant-table AST form exists; instance `value` fields
  are string literals only.
- **Blocks:** weight-matrix-driven population (which pad gets which value)
  from an external source (e.g. a decoded GGUF layer slice).
- **General fix:** general `data w: Int[16][8] = ...;` constant-table
  literal usable in instance value positions. Population *decisions*
  ("is this pad populated?") remain in generator programs (see T1); the
  table only carries values the emitter walks.
- **Gate:** table-driven emission compiles; byte-deterministic (sorted
  iteration — determinism rule).
- **Effort:** M

### E3 — KiCad emitter: single A4 sheet, channel-routed wires
- **Evidence:** `ElectronicsBackend::generate`
  (`src/backend/electronics/mod.rs:128`) emits one sheet, paper A4;
  `emit_net` (`:356`) routes one L-shaped wire per net through the
  inter-column channel — wire count grows O(nets).
- **Blocks:** thousands of instances (crossbar tiles) unusable in KiCad.
- **General fix:** per-bank hierarchical sheets + global labels (net
  naming already exists via `Expr::Named`).
- **Gate:** a 512-instance tile opens ERC-clean in KiCad.
- **Effort:** M

### E4 — No multi-unit symbols (op-amp = 2–3 units)
- **Evidence:** `emit_symbol_def` (`:291`) emits one rectangle per type;
  `pin_layout` (`:96`) splits pins left/right only.
- **Blocks:** op-amps (2 signal units + power unit), dual op-amps, relays —
  the entire active-component catalog.
- **General fix:** `unit` clause on pin declarations; per-unit symbol
  blocks in `lib_symbols` + per-unit placement instances. Benefits any
  multi-unit part, not just amplifiers.
- **Gate:** a 5-pin op-amp (unit A, unit B, power) renders correctly and
  ERCs clean in KiCad.
- **Effort:** M

### E5 — Invalid footprint identifiers; no BOM
- **Evidence:** `emit_instance` (`:321`) writes the `Footprint` property
  from the raw `package` string (`"0805"` — not a KiCad
  `Library:Footprint` ID); no BOM emission anywhere.
- **Blocks:** JLCPCB assembly BOM, KiCad footprint assignment.
- **General fix:** `footprint: "Lib:Name"` clause, format-validated at
  compile time (contract, not lint); optional JLCPCN property pass-through;
  CSV BOM emission (reference/value/footprint/JLCPCN).
- **Gate:** KiCad imports the BOM; JLCPCB's checker accepts the project.
- **Effort:** S

### E6 — No power symbols (rails as labeled wires)
- **Evidence:** `emit_net` (`:356`) emits wire + `label` only.
- **Blocks:** ERC-grade supply recognition; readability of rail-heavy sheets.
- **General fix:** power-symbol emission for supply-classified nets.
  Classification must be declared (component/net attribute or stdlib
  PowerFlag), never inferred from net names like "GND" (Rule 15).
- **Gate:** KiCad ERC recognizes the rails as power nets.
- **Effort:** S

### E7 — No supply-rail current roll-up
- **Evidence:** analysis proves per-net physics — Ohm fixpoint
  (`derive_current :736`), KCL (`contribution_pass :695`), power
  (`derive_power :777`), postcondition bounds (`check_current_bounds
  :822`) — but never sums total current drawn **from a driven rail** nor
  compares it against the source component's `rating`.
- **Blocks:** proving the board does not exceed its 3.3 V regulator.
- **General fix:** a rail is a net with a declared drive source; sum
  derived branch currents per rail; compare against the source
  component's rating clause (same mechanism as `check_current_bounds`).
- **Gate:** contract test — overbudget rail fails compilation with a
  proof-carrying diagnostic; within-budget board compiles.
- **Effort:** M

### E8 — No op-amp drive / fan-in model
- **Evidence:** non-ohmic parts are `rating any` black boxes
  (`parse_rating_clause :2366`; the Led model in
  `led_blinker.ebv:38-44`); no driven-node or fan-in analysis exists.
- **Blocks:** honest op-amp summing-node proofs (crossbar output columns).
- **General fix:** driven-node current bound = sum of contributing branch
  currents into an active input node, compared against the driving
  component's rating; fan-in count bound. Generic over any multi-pin
  active part.
- **Gate:** a 16-input summing node proves within rating; an overdriven
  node is a compile error.
- **Effort:** M

### E9 — `lib/std/hardware.bv` missing while the prelude imports it
- **Evidence:** `plugins/parsed/prelude-hw.bv:11` inserts
  `import "std/hardware.bv"`; `lib/std/` contains no `hardware.bv`
  (verified by listing).
- **Blocks:** any `.sbv` build that loads the hw prelude (verify the exact
  prelude-load path for `.sbv` when touching — `config/targets.dbvl` row
  decides).
- **General fix:** minimal honest `hardware.bv` containing only what the
  prelude contract needs — or drop the import until real content exists.
  No placeholder pretending to be work (Rule 8/ALWAYS FINISH applies to
  the file, whichever way it goes).
- **Gate:** `brievc build <x>.sbv` with default preludes succeeds.
- **Effort:** S

### E10 — Stale electronics docs + committed artifact drift
- **Status:** CLOSED 2026-09-23 (E14a gate Slice 5): deferred list
  refreshed in `docs/architecture/electronics-frontend.md`;
  `examples/electronics/led_blinker.kicad_sch` regenerated from the
  current emitter.
- **Evidence:** `docs/architecture/electronics-frontend.md` "Deferred"
  section claims named nets and unit suffixes are unbuilt — both landed
  (`src/parser/expressions.rs` `at_net_prefix :88`, `is_unit_suffix :14`,
  `UnitLiteral :781-806`). The committed
  `examples/electronics/led_blinker.kicad_sch` carries UUIDs from an older
  `uuid()` implementation (current output differs).
- **Blocks:** nothing (hygiene) — but stale docs are bugs per the docs
  maintenance rule.
- **General fix:** refresh the doc's deferred list; regenerate the demo
  artifact from the current emitter.
- **Effort:** S

### E15 — No series-part placement synthesis (the residue of §3.2's `derive on:`)
- **Status:** OPEN — deferred 2026-09-23, documented (cannot defer without
  documentation). Not built; recorded with a trigger.
- **What it is:** from a component's current obligation plus its intrinsic
  forward-voltage physics, the compiler should PLACE a current-limiting
  series part (topology) and SIZE it (value) so the current lands in range —
  §3.2's `led1 = true; // physics derives the series resistor`. Two
  separable features: **topology** (where the part goes — the anode needs a
  path to a driven rail through a free part) and **value synthesis /
  sizing** (solve `R = (Vrail − Vf) / I`, pick an E-series step).
- **Evidence:** the fixture wires `r_led` explicitly
  (`examples/electronics/usb_sensor.ebv:125`) with value `"TBD"`;
  `collect_series_parts` (`src/analysis/electronics.rs:1053`) reads the
  instance `value` and `parse_ohms`es it, so `"TBD"` drops the part and no
  current is derived; the LED is non-ohmic (no resistance), so even with a
  value the anode net stays unclassed and the part is one-sided
  (`derive_power` note, `:1225`); `check_current_bounds` (`:1284`) proves
  UPPER bounds only. Net effect: the fixture's LED bound
  `[led1.a.current >= 2mA && <= 20mA]` is VACUOUSLY proven.
- **Blocks:** nothing today (the board compiles); blocks the LED bound
  being actually proven, and any second component that needs
  current-limiting placement.
- **Prerequisites (all deferred):** per-instance `spec Resistance`
  (quantities plan Phase 3); a current-obligation form `spec MinCurrent`/
  `MaxCurrent` (Phase 4); a forward-voltage physics spec (`spec
  ForwardVoltage: 2.0V`); a non-ohmic clamp model in `derive_current`;
  lower-bound proofs in `check_current_bounds`; E-series value selection
  (config/stdlib, not compiler-hardcoded — Rules 14/15).
- **Surface analysis (2026-09-23):** `when` and `thru` are BOTH the
  conditional-mechanism family — `thru` selects a Control-bearing mechanism
  (`is_mechanism`, `:2283`), `when` either desugars to a mechanism or
  hard-errors "copper cannot be conditional" (`:2009`). The LED's series
  path is UNCONDITIONAL (the drive intent handles the on/off; the anode
  path is permanent copper), so both are the wrong family. The honest
  surface is the **obligation + forcing** family (slices 1–2: obligation
  on a net → place a free part), extended to a current obligation and a
  series part — no new keyword. `when`/`Conducts` remain candidates only if
  a future need is to express a part's INTRINSIC conduction.
- **Trigger:** a second component needs current-limiting placement, or the
  LED bound must be actually proven rather than vacuous.
- **Effort:** L

### T1 — GGUF → instance-table emitter tool (tooling, not compiler)
- **Evidence:** Bachi's `repack` crate
  (`../Bachi/repack/src/{gguf,tq1_0,ptq1_0,repack}.rs`, commit `8c58453`)
  already decodes GGUF + PTQ1_0 ternary blocks (28 B / 128 trits, truncation
  cascade decode).
- **Blocks:** feeding real model weights into E1/E2 syntax.
- **General fix:** extend repack (or a sibling tool) to emit instance
  tables (CSV or Briev `data`-literal form) for a chosen layer slice.
  Purely out-of-tree; no compiler knowledge gained.
- **Gate:** a real GGUF layer slice → 32×16 tile instance data; byte-
  deterministic across runs (Bachi determinism rule 10).
- **Effort:** S

---

## Silicon track (`.sbv` → CIRCT → Verilog → Bachi/KV260)

Context: Bachi's verified architecture already IS near-memory computing —
weights as index bits in DDR4 streaming into LUT-baked ternary cores
(`../Bachi/RESEARCH.md` §1.6, §4, §6). The gaps below serve that datapath.
Explicit non-gaps: the 5-trit unpacker and adder/subtractor trees are
expressible **today** (comb ops, `int_ops`, sized ints) — no entry is filed
for them, and none may be invented as a special case.

### S0 — G8 reclassification: route rewrite in the genuine surface
- **Evidence:** `../Bachi/hardware/route/bachi_route.sbv:7` uses
  `module`/`reg`/`wire`/`clk.rising()` — syntax `.sbv` never had (parse
  error "unknown top-level item 'module'"). Bachi's ledger row "no
  clock/reset concept" is wrong as stated: the backend ships `seq.firreg`/
  `seq.firmem`, MMIO `@addr` ports, contract-gated commit with `halt`
  port, and watchdog countdown monitors (`src/backend/circt/mod.rs:148`
  CAPABILITIES; `docs/plans/2026-08-23-circt-toolchain-validation.md` —
  xck26 synthesis; `docs/plans/2026-08-25-circt-seq-firmem.md`;
  `docs/architecture/backend-contracts.md` §6).
- **Blocks:** the entire Bachi ledger's blocking-matrix premise.
- **General fix:** rewrite `bachi_route.sbv` in the genuine silicon
  surface (reactive txn, sized ints, bounded arrays, sync blocks, MMIO);
  run the three-way parity loop; re-derive the true G8 remainder from
  what the rewrite actually exposes.
- **Gate:** C ref (`../Bachi/reference/`, `../Bachi/hardware/route/ref/`)
  ↔ hand-written SV ↔ `.sbv`-generated Verilog, all under verilator.
- **Effort:** M

### S1 — Pipeline stages / register enable
- **Evidence:** contract-as-hardware semantics commit per cycle
  (`backend-contracts.md` §6); no `stage` construct, no enable on
  `firreg`; the reactive model fires all txns every cycle
  (Bachi RESEARCH §5.2-3).
- **Blocks:** TAC MAC pipeline cannot close timing at target clock;
  blocks tac, alu, conv1d, deltanet (Bachi matrix).
- **General fix:** explicit stage construct or firreg enable — general
  mechanism; staging decisions computed in the frontend (frontend-driven
  dispatch doctrine), consumed by the emitter.
- **Gate:** staged MAC ≥200 MHz Vivado OOC + behavioral parity.
- **Effort:** L

### S2 — Parameterized cells ignored by the emitter
- **Evidence:** `Transaction`/cell AST carries `type_params`; zero reads
  in `src/backend/circt/` (only occurrence: test helper `make_txn`,
  `circt/mod.rs:1922`, always `vec![]`).
- **Blocks:** TAC needs one definition at N ∈ {5120, 6144, 17408}
  (Bachi RESEARCH §1.6).
- **General fix:** emit-time monomorphization first (matches
  frontend-decides doctrine); `hw.module` parameters later only if
  artifact size demands.
- **Gate:** one definition → three width instantiations, all synth +
  parity.
- **Effort:** M

### S3 — Memory: distributed-only, 1-port, 1-D, no init, no URAM
- **Evidence:** firmem companion template hardcodes
  `ram_style = "distributed"` (`circt/mod.rs:468`); bounded 1-D arrays
  only; single read+write port; no init-from-data; no URAM path
  (`src/backend/circt/mem_policy.rs` knobs: `firmem_min_depth`/
  `firmem_max_ports` only).
- **Blocks:** deltanet state [48][128][128] → BRAM; weight page buffers
  in BRAM/URAM (Bachi RESEARCH §5.2-1, §6).
- **General fix, staged:**
  - **S3a** — multi-dimensional arrays + `ram_style` policy in
    `mem_policy.rs` (M)
  - **S3b** — dual-port memories (M)
  - **S3c** — init from data literal (S; composes with the E2 table
    form's silicon analogue)
- **Gate (each stage):** deltanet-shape buffer → BRAM companion; Vivado
  OOC + parity.
- **Effort:** M + M + S

### S4 — No AXI surface
- **Evidence:** `hardware_lib` TOMLs declare axi4-stream/lite/full
  (`src/hardware/mod.rs`, `examples/hardware.toml`) but no emitter
  consumes them; extern blackboxes ARE supported (`extern_cells: true`,
  `circt/mod.rs:175-177`).
- **Blocks:** route + all module I/O; DMA integration (Bachi matrix).
- **General fix, staged:**
  - **Stage 1** — documented extern pattern + companion `.sv` for the
    AXI DMA (S; per Bachi rule: blackbox interim requires a dated ledger
    note with owner phase)
  - **Stage 2** — interface bundles → port lists + handshake FSM
    generation (L)
- **Gate:** stage 1 — DMA sweep passes against
  `../Bachi/hardware/common/sim/axi4_ddr_model.sv`; stage 2 — handshake
  FSM parity.
- **Effort:** S then L

### S5 — No XDC / constraint generation
- **Evidence:** `HardwareLib` carries max_frequency and constraint fields
  (`src/hardware/mod.rs`) — nothing emits XDC.
- **Blocks:** Vivado integration for every module.
- **General fix:** emit XDC from hardware_lib TOML + `@addr` pin map.
- **Gate:** Vivado accepts the generated XDC; target clock enforced.
- **Effort:** S

### S6 — Capability flags may lie
- **Evidence:** CIRCT `CAPABILITIES` sets `if_expr`/`match_expr`/
  `term_endprogram`/`match_stmt`/`trap_stmt` true (`circt/mod.rs:159-174`);
  Bachi RESEARCH §5.3 audit reports several lack emitter arms —
  `validate_program` catches at the gate, but direct `generate()` callers
  hit runtime `record_unsupported` (`:207`).
- **Blocks:** nothing directly; poisons trust in the capability contract.
- **General fix:** per-flag audit — flag true ⇔ emitter arm + unit test
  exist; otherwise flip the flag or add the arm. Honest-by-construction
  matrix.
- **Gate:** flag ⇒ arm ⇒ test matrix green; `validate_program` and
  `generate` agree on every construct.
- **Effort:** S

### S7 — Silent fallbacks in the emitter
- **Evidence (verified at current HEAD):** statement-drop `_ => {}` at
  `circt/mod.rs:349` and `:1787`; constant-init `_ => "0".to_string()` at
  `:1000`; postcondition-default `_ => hw.constant 1 : i1` (~`:421-423`);
  unnormalized-type `format!("i64")` (~`:1785-1790`, whose own comment
  says "record it loudly" — verify it actually records). Each can emit
  wrong hardware silently — the exact failure mode the emitter's own
  doctrine rejects (see the `:1160` comment: "rejects downstream, or
  worse: unverifiable silence").
- **Blocks:** trustworthy three-way parity (a silent fallback can fake a
  pass or hide a divergence).
- **General fix:** replace each with `record_unsupported` hard errors +
  negative tests.
- **Gate:** negative tests — each unsupported construct errors loudly,
  never emits.
- **Effort:** S

### S8 — Ternary domain type: DECIDED, no new fundamental type
- **Decision (2026-09-21):** no `Trit` fundamental type. `Int<2>` +
  stdlib decode composites cover the TLMM LUT-index datapath. A new
  fundamental type risks vocabulary baking (Rules 15/23).
- **Reversal requirements:** a documented construct needed by the
  datapath that cannot be expressed with sized ints + stdlib without a
  compiler change — recorded here with evidence before any type is added.
- **Effort:** S (this entry is the record; nothing to build)

### S9 — No `.sbv` examples committed in briev-lang
- **Evidence:** fixtures live only in `tmp_fixtures/hw/*.bv`;
  `examples/` has `electronics/` but no silicon directory.
- **Blocks:** discoverability; Bachi S0 needs a canonical example anyway.
- **General fix:** commit the S0 route rewrite + the existing counter
  fixture as `examples/silicon/` with documented build commands.
- **Effort:** S

---

## Streaming datapath (near-memory) — CANDIDATE features

Reality preamble (so no one re-hypes this later): the KV260 already is the
reference implementation of the near-memory architecture — hard DDR4
controller, 64-bit bus, 13–16 GB/s practical, layer-selective streaming.
A €100-class SODIMM carrier board (16-bit DDR3, soft MIG) streams 3–6 GB/s
and yields a naive full-model decode ceiling near ~1 tok/s vs Bachi's
≥4 tok/s layer-selective target. RESEARCH §1.6's numbers govern: the
problem is bandwidth, never the adders. The candidates below serve Bachi
**Phase 3** (integration); nothing pulls them before then.

### S10 — FIFO / stream primitive (CANDIDATE)
- **Why:** every MIG→unpacker→adder datapath needs elastic decoupling
  (skid buffers, valid/ready); `.sbv` has no stream/FIFO construct.
- **General fix shape:** bounded FIFO primitive lowering to shift-register
  RAM or `seq.firmem`, with valid/ready handshake ports; composes with S4
  stage 2 bundles.
- **Gate:** FIFO parity vs C model; synthesizes to LUT/BRAM per depth
  policy.
- **Effort:** M — **status: candidate; pulled by Bachi Phase 3.**

### S11 — Burst address generator / AXI read-master in `.sbv` (CANDIDATE)
- **Why:** the "long-term Briev-native" DMA option already recorded in
  Bachi RESEARCH §7; without an expressible burst read master, that
  option cannot exist.
- **General fix shape:** address-counter + AXI handshake FSM expressible
  via S4 stage 2 bundles (or as the bundle's first consumer); no
  algorithm vocabulary.
- **Gate:** burst reads of a known address pattern from
  `axi4_ddr_model.sv`; address-sequence parity vs C model.
- **Effort:** M — **status: candidate; pulled by Bachi Phase 3.**

### S12 — Double-buffer (ping-pong) pattern — STDLIB COMPOSITE, NOT A GAP
- **Note:** ping-pong buffering composes S3b (dual-port) + S3c (init) +
  control. It is a declared stdlib composite over existing machinery
  (Rule 24: the language keeps the temporal). Filed here only so nobody
  opens a compiler gap for it later.

---

## Cross-references

- Bachi module-facing ledger + per-closure evidence protocol:
  `../Bachi/reports/briev-gaps.md` (Amendments, append-only).
- Mapping used there: G1→S1, G2→S2, G3→S3, G4→S4, G5→S6, G6→S7, G7→S5,
  G8→S0 (reclassified 2026-09-21).
- Sequencing / phase plan: intentionally absent from this registry; to be
  written as its own dated plan when execution is ordered.
- Related prior art in-tree: `docs/plans/2026-08-23-backend-scaffolding-foundation.md`,
  `docs/plans/2026-08-23-circt-toolchain-validation.md`,
  `docs/plans/2026-08-25-circt-seq-firmem.md`,
  `docs/plans/2026-08-27-cbv-foreign-hardware-and-mmio.md`,
  `docs/architecture/backend-contracts.md` §6,
  `docs/architecture/electronics-frontend.md`.

---

## Amendment 2026-09-21 (same-day): intent-synthesis reframe

The design record `2026-09-21-intent-synthesis-node-semantics.md`
(landed in the same commit batch) supersedes the FRAMING of the
electronics track above. The silicon track (S0–S9, S10–S12 candidates,
S8 decision) is untouched by this amendment.

- **E1–E5, E8–E10 stand** as emitter/analysis work, with reframes:
  E3's authoring-side hierarchy is owned by CELLS (design record D10) —
  the multi-sheet KiCad *emitter* scope remains; E7's budgets attach to
  SOURCE PINS, not nets (design record D4) — the roll-up analysis
  machinery is unchanged; E13 becomes `spec` clauses on catalog types
  (D5).
- **E14 is replaced** by the staged ladder **E14a** (intent completion —
  explicit equalities still allowed, drive-map intents infer remaining
  memberships) → **E14b** (pure intent — guards and behaviors only).
  Foundation: existing primitives only (reactor nodes, cells, trg,
  Rule 22 classification, per-firing physics fixpoint). New machinery =
  membership inference + behavioral-equivalence-class enumeration. Gate
  fixture and the ambiguity-surface enumerator spec live in the design
  record (§2 D13, §3.2). Effort: L each; strict generalization order.
- **Ambiguity-lifting keywords are SLOTS** (persist-tighten,
  commit-select), names provisional until implementation — no gate
  depends on a spelling.

### Amendment 2026-09-21 (evening): E12 realization note

E12 is realized as **stdlib-declared fundamentals with property-driven
behavior** (design record D6 amendment): `Power`/`Ground`/`In`/`Out`/
`Io`/`IoOd`/`Nc` are parentless types in `std/electronics.bv` carrying
`spec KicadType` / `spec NoConnect` properties; the compiler resolves pin
class references against imported types and consumes properties
generically — no class names in Rust. Gate unchanged in substance
(`nc` pin compiles clean from the dangling error; class-keyed violations
compile errors once ERC consumes the properties in later slices).
Effort S→M (type-universe resolution plumbing in analysis + emitter).

### Amendment 2026-09-21 (evening, II): E11 landed in stages

Array declaration + per-element indexed wiring are IMPLEMENTED
(`pin gpio[8]: Io;` expands to `gpio[0]…gpio[7]`; contracts address
elements as `inst.gpio[3].voltage`; netlist/BEAST/emitter stay
array-blind — the parser expands eagerly). The whole-bus equality
sugar (`j1.data == mcu.data` expanding to per-element equalities)
remains OPEN under E11 until the gate fixture demands it; the E11 gate
("64-bit bus netlist ≡ manual per-pin clauses") is already satisfiable
via indexed equalities.

### Amendment 2026-09-21 (evening, III): E13 landed — decoupling convention

IMPLEMENTED: `spec Decouple: "100n";` on a component type requires, per
instance, a part whose type declares `spec Decoupler: true;` bridging each
`spec Supply: true;` pin to a `spec Return: true;` pin (rail roles live on
the class fundamentals `Power`/`Ground` in std/electronics.bv; the catalog
`Capacitor` declares `Decoupler`). The checker reads the property
interface generically — the compiler knows no type or class names
(Rules 14/15). Violations are hard `convention_errors` that refuse
emission. Value matching (cap value vs. the stated "100n") remains OPEN
under E13 — presence check only in this slice.

### Amendment 2026-09-21 (night): E7 landed — source-pin budgets

IMPLEMENTED per design record D4: `budget u1.out <= 250mA;` attaches to a
source PIN (not a net). The roll-up is the KCL boundary sum over the B4
part graph (direction-agnostic — a source net has current LEAVING it,
which the sink-side per-net map cannot express). No derived draw =
vacuous pass. Exceeded budgets are hard `budget_errors` refusing
emission. OPEN under E7: black-box draws (IC internals) are not yet
derivable — the intent machinery (E14a) owns making them provable.

With this, every E-track prerequisite for the usb_sensor gate fixture is
in place: classes (E12), arrays (E11), decoupling (E13), budgets (E7).
E14a — intent completion: node guards, drive maps, keep/store ambiguity
machinery — is the next and final slice before the fixture can compile.

### Amendment 2026-09-23: E14a GATE PASSED — usb_sensor fixture compiles

The §3.2 gate fixture (`examples/electronics/usb_sensor.ebv`) compiles
end-to-end to a `.kicad_sch` (plan `2026-09-23-ebv-gate-fixture.md`).
**E14a is CLOSED** by its gate: fixture compiles, error matrix passes
(four negatives — ambiguous intent enumerates candidates, dropped gnd
path dangles, removed decap breaks the E13 convention, `open` on wired
pins refuses), netlist + emission byte-deterministic. General fixes the
fixture surfaced (none fixture-shaped, Rules 14/15/24):

- **prelude-electronics** now injects `std/electronics.bv` into
  import-less sources (anchored on the first type declaration). Without
  it every class ascription resolved to a not-in-scope class error and
  defaulted to `can_drive: false` — a real fixture with no imports never
  saw the class fundamentals.
- **resolve_pin** handles a BARE indexed pin (`u2.gpio[0]`, `j2.sig[0]` —
  shape `Index(Field(inst, arr), i)` without an outer property access);
  extracted a shared `lookup_pin` tail.
- **Pin-level drive intents** `u1.en = true;` landed (previously the
  field form was silently dropped by `body_facts`): `FactSink` now
  routes every body assignment through a form check — mechanism bridge,
  conditional error, instance intent, pin intent, or pin-to-pin wire —
  and anything unrecognized is a hard error, never a silent no-op. The
  pin intent completes against the sole free drive-capable pin elsewhere
  (same D13 ambiguity enumeration as the instance form); an
  already-connected pin records.
- **Typechecker** admits declarative electronics intents (`inst = true;`,
  `inst.pin = true;` assign Bool to a pin/instance target) and registers
  instance-array base names (`r_pu` from `r_pu[0]`) as vectors so
  `r_pu[0].a` typechecks at the top level.
- **Reference designators** number per PREFIX across the sheet (two
  types sharing `reference "U"` must read U1, U2, U3 — three U1s are
  invalid KiCad); the old per-type counters were a latent bug.
- **E13 convention** is a PRESENT-state obligation: an `unpop` decoupler
  no longer satisfies it (its pads remain on the sheet; an absent part
  carries no capacitance).

CLOSED 2026-09-23 — **`derive on:` was superseded, not built.** The per-
instance obligation §3.2 wrote as `type Led { derive on: a.current >=
2mA; }` is captured by the property-driven spec form instead: `spec
MinCurrent: 2mA;` / `spec MaxCurrent: 20mA;` (Phase 4 of plan
`2026-09-23-quantities-and-annotation-doctrine.md`), PascalCase physics
per the annotation-vs-physics doctrine. The fixture states the bound as a
use-site txn postcondition today. The one thing the `derive on:` idea
left behind is series-resistor PLACEMENT synthesis (choosing r_led so the
current lands in range) — a synthesis/machinery concern, not a syntax
clause; tracked separately in the E14b gate delta, to be discussed.

### Amendment 2026-09-23 (II): E14b slice 1 landed — min-voltage pull-up forcing

The first pure-intent slice is IN: a node body may state a MIN-voltage
obligation — `inst.pin.voltage >= <literal>V;` — and the compiler wires a
free `spec PullUp: true` part (new spec key + stdlib Resistor) between the
obligation net and the lowest qualifying driven rail. D13 preserved: no
part or no rail → hard error; distinct-value free parts (or rails tied at
the minimal volts) → enumerated ambiguous error; identical parts assign
deterministically (any bijection is equivalent — the pick is immaterial).
The typechecker admits the comparison as a declarative physics fact
(never executed, like `inst = true`). The i2c node of the usb_sensor
fixture now uses the pure-intent form; the bus unions (`u2.sda = u3.sda`)
stay explicit — E14b-2.

E14b remaining (backlog, each marked in the fixture): bus assembly,
gnd-path forcing (`pin.voltage <= V` is a hard error today — "state the
wire explicitly"), `en` drive assignment solving, rail inference (rails
stay explicit guard equalities).

### Amendment 2026-09-23 (III): E14b slice 2 landed — low-hold forcing

A MAX obligation (`inst.pin.voltage <= V;`) is no longer an error: the
net must be held at or below V, forcing a path to the return rail.
- Net already on the return rail → satisfied.
- Net already switchable to the return rail → satisfied.
- Else a free switchable part (type with ≥2 Switchable-class pins, ≥1
  free) wires net→p1, p2→return (the return side may already be wired by
  a guard equality). Distinct-value parts → enumerated ambiguous error.
- No switchable part: a pulled-up net is a hard error (it cannot be held
  low without a mechanism — that is the error's message); an isolated net
  wires directly to return.
The fixture's `Switch` pins ascribe `: Path` (E12 class, passive KiCad
type — emission unchanged); the button node is now pure-intent
(`u2.gpio[3].voltage <= 0.3V;` forces the sw1 low path; the r_btn
pull-up stays explicit — value-aware resistor matching is backlog).

E14b remaining (backlog, each marked in the fixture): bus assembly,
`en` drive assignment solving, rail inference, value-aware resistor
matching.

### Amendment 2026-09-23 (IV): E14b slice 3 landed — bus assembly

Open-drain bus assembly: MIN obligations on same-name WiredAnd-class
pins at the SAME voltage union into one net before pull-up forcing. The
pin name is the author's signal identity; WiredAnd is the class that may
share a driven net; the shared obligation is the coupling. Different
names, different voltages, or non-WiredAnd pins never union (each net
keeps its own pull-up — honest D13). Voltage obligations now realize
BEFORE drive completions: an unassembled open-drain pin must never appear
as a drive candidate for a later instance intent (led1 had seen five
"free" IoOd pins and gone ambiguous). The forcing consumes passive parts
(pull-up resistors, switches), never CanDrive pins, so it cannot steal a
completion. The i2c node of usb_sensor is now obligations ONLY — the
explicit `u2.sda = u3.sda` / `u2.scl = u3.scl` unions are deleted; the
emitted sheet stays byte-identical to the E14a explicit-facts form.

E14b remaining (backlog, each marked in the fixture): `en` drive
assignment solving, rail inference, value-aware resistor matching,


### Amendment 2026-09-23 (V): E14b slice 4 landed — drive assignment

The usb node no longer pre-wires the en line: `u1.en = true;` and
`led1 = true;` are resolved by a batch drive assignment. Several open
drive intents (instance form or pin form, each with exactly one open pin)
competing for INTERCHANGEABLE free drive-capable pins are a perfect
matching — same PinClassProps → any bijection is equivalent → assigned
deterministically (sorted intent ↔ sorted pin), never an ambiguity error.
Fall-through preserves D13: a single completable intent, supply < demand,
or mixed-class supply all go to the per-intent completions whose
no-completion / enumerated-ambiguous errors stand. Voltage obligations
(which consume passive parts only) still realize before the assignment,
so a bus-assembled open-drain pin never pollutes the drive supply. The
fixture's en↔gpio[0] and led1.k↔gpio[1] wires are now fully inferred;
gpio[2..7] were already claimed by the j2 facts and the button low-hold.

E14b remaining (backlog, each marked in the fixture): rail inference,
value-aware resistor matching.

### Amendment 2026-09-23 (VI): E14b slice 5 landed — value-aware pull-up matching

Pull-up forcing no longer errors on distinct-value free parts. A MIN
obligation is satisfied by ANY pull-up resistance — a released (high-Z)
net sits at the rail regardless of the resistor value — so the value
choice never matters for satisfaction and the pick is deterministic
(same doctrine as the slice-4 drive assignment). Switches keep the
distinct-value ambiguity: a multi-pole switch has different capacity, so
the pick can matter. The button node of usb_sensor is now fully
pure-intent (`u2.gpio[3].voltage >= 2.7V;` + `<= 0.3V;`) — the r_btn
pull-up is inferred. The compiler may place a 4k7 on the button net and
the 10k on an i2c bus; the choice is immaterial, the netlist equivalent.

E14b remaining (backlog, each marked in the fixture): rail inference.

### Amendment 2026-09-23 (VIII): the `fab` layer — board placement landed

The physical-layout section landed (plan `2026-09-23-ebv-fab-layer.md`):
`fab { board 40mm x 20mm; place u1 @ (20mm, 10mm) rot 90; }` — an in-file
section mirroring `.rbv`'s render, on the annotation side of the
annotation-vs-physics doctrine. The compiler carries the placement and
PROVES what is provable: containment (off-board = hard error) and
clearance (warning) from the declared outline + `config/footprints.dbvl`
geometry + positions. Pinned placements win; everything unplaced flows to
a deterministic, board-aware auto-placer (grid + decoupler proximity).
Emits a `.kicad_pcb` alongside the schematic with physics-derived net
labels (GND, V3.3). New surface: `Length` dimension + `mm`/`cm` units;
`fab`/`board`/`place`/`rot`/`x` (only `fab` is a token). Deliberately
picks up the D15 placement non-goal; routing (compiler machinery),
length matching, impedance, and transient remain non-goals.

Remaining (backlog, each marked in the fixture): rail inference,
**auto-routing** (the fab plan's follow-on slice — the author controls
placement only).

### Amendment 2026-09-23 (VII): quantities + annotation doctrine — Phase 1 landed

The compiler-vs-annotation doctrine is locked (plan
`2026-09-23-quantities-and-annotation-doctrine.md`): if the compiler must
READ it to prove the board works → physics → PascalCase spec; if a
human/manufacturer reads it to build the board → annotation → lowercase,
opaque. Quantities are bare (`spec Decouple: 100n;`), never quoted
(quotes imply arbitrary). Phase 1 (quantity foundation) landed:
`PropertyValue::Quantity { si, dimension }` + `QuantityDim`, the spec
unit grammar (scaling prefixes p/n/u/m/k/M/G case-sensitive, base units,
key-dimension resolution, E-series `4k7` fraction, dimension-conflict
hard errors), and `spec Decouple: 100n;` across stdlib/fixture/tests
(the old `"100n"` string form is rejected). Phases 2–4 (Tolerance/Rating
→ specs, Resistance → spec, Min/MaxCurrent envelope) are recorded with
triggers in the plan.

E14b (pure intent — no explicit equalities) remains OPEN; every
explicit equality the fixture needed beyond the drive map is marked
`// E14b:` in the file and recorded in the design record's gate delta.

### Amendment 2026-09-21 (night, II): E14a slice 1 — node-body intents

IMPLEMENTED (first vertical of E14a): `node` guards already carry
topology (they are Transactions); bodies now carry two fact forms —
pin-to-pin assignments are wiring facts, and `inst = true;` is a drive
intent completing the last open pin against unconnected `spec CanDrive`
pins (property on Out/Io/IoOd fundamentals). Ambiguity enumerates
candidates and hard-errors; every synthesized connection carries
provenance in `intent_proofs` (design record D3). INTENT ERRORS REFUSE
EMISSION FIRST — they outrank downstream diagnostics on an incomplete
board.

Deferred (named, owned): `keep`/`store` syntax (the D14 lifting slots —
names provisional; slice 1's body-assignments and guard facts cover the
wiring surface), multi-pin intent completion, class-semantics ERC
(drive-single, contention), `chain`/`await` sugar, whole-bus equality.
The usb_sensor fixture compiles after these land.

### Amendment 2026-09-21 (night, IV): D16 phase 1 — guarded body facts

IMPLEMENTED: `when` bodies in nodes are SEEN and CLASSIFIED (the
silent-skip hole is closed). Pin-referencing conditions are signal-level
→ the D7 gate fires: conditional wiring demands a mechanism, hard error
naming the compound condition and both fixes (region carve in the
guard, or switching part). Pin-free conditions (`true`) carve a region:
facts apply as ordinary wiring. Nested whens compound. Phase 2
(mechanism synthesis via switch-part `spec Control` vocabulary) and
phase 3 (cross-region complement check) remain open under D16.

### Amendment 2026-09-21 (night, V): D16 phase 2 plan + D17 model boundaries

D16 phase 2 (mechanism synthesis) is PLANNED and finalized in the design
record: `when <cond> { … } via <Name>;` strategy clause (trailing,
type-first narrowing, instance-second), Control/Path class fundamentals
(+ two spec-gate keys), bridge-request synthesis into three unions,
`conditional_bridges` record, emitter untouched. This closes the
D7-gate into a constructive mechanism: signal-level when-clauses stop
erroring and start synthesizing once the vocabulary lands.

D17 (model boundaries) is the standing honesty ledger: parasitics (B1),
SI/EMI (B2), thermal spreading (B3), thresholds (B4), test & safety
provisions (B5), layout reality (B6). Each named with its trigger and
eventual home; the compiler claims schematic-level truth only. D17 is a
standing obligation: any slice approaching a boundary re-states it in
that slice's output rather than silently crossing.

### Amendment 2026-09-21 (night, VI): D16 phase 2 — mechanism synthesis LANDED

`when <cond> { … } via <Name>;` synthesizes conditional connections
through a declared switching part: the condition pin's net drives the
`Control`-class pin; `Path`-class pins bridge the wired pins. The D7
gate is now constructive for single pin voltage-comparison conditions —
compound signal-level conditions still hard-error ("copper cannot be
conditional"). Control/Path are class fundamentals; any type with one
Control + two Path pins is a mechanism (property interface, no part-name
knowledge). `via` narrows by type/instance; ambiguity enumerates and
requests the strategy; zero candidates names the missing declaration.
Bridges are recorded on the netlist as `conditional_bridges` (the
phase-3 cross-region complement check and per-region physics consume
them). The broad `instances` consolidation into NetlistContext landed in
this slice (walker/synthesizer parameter gates held).

Open under D16: phase 3 (cross-region complement check), asymmetric
path assignment (relays with coil/contact distinction), mechanism
condition validation (non-pin operand must be a voltage literal).

### Amendment 2026-09-22 (D16 phase 3): redundancy gate

A bridge whose pins are already unconditionally connected (same
union-find root before synthesis) is redundant — the switch can never
open them. The compiler emits a hard error naming the unconditional
wiring that defeats the mechanism. `conditional_bridges` now reports only
successfully synthesized bridges (previously it included errored requests
too — fixed in this slice).

Phase-3 scope clarification: this is the "connected in region A +
required-disconnected in region B → mechanism demand" rule in its
implementable first form: unconditional copper (region A = always) makes
a mechanism (region B = conditional) meaningless. The complementary
form — author asserts disconnection via syntax — requires a disconnection
syntax (phase-3b, deferred).

Open under D16: phase-3b (author-expressed disconnection), mechanism
condition validation (non-pin operand must be a voltage literal),
asymmetric switch parts, keep/store syntax, ERC class semantics,
chain/await, whole-bus equality.

### Convention note 2026-09-22: spec keys are PascalCase

All `spec` keys in the electronics dialect are PascalCase (`KicadType`,
`NoConnect`, `Supply`, `Return`, `CanDrive`, `Control`, `Switchable`,
`Decouple`, `Decoupler`, `Budget`). Any future spec name must follow.

### Decision 2026-09-22: `DefaultLevel` / `x = high` deferred — no consumer

The `spec default_level` / `x = high` abstraction (design-record D5,
line 404) is **deferred indefinitely**, not renamed. Rationale:

- Mechanism synthesis needs exactly one thing from a `when` condition:
  the **control pin**. The voltage/level value is never read downstream —
  D15 keeps conduction physics black-box; the compiler proves wiring only.
- `x = high` would therefore carry a level the compiler consumes nowhere —
  pure readability sugar, not intent-synthesis substance.
- Rule 15/23: a level vocabulary (even stdlib-taught, e.g. `spec
  DefaultLevel: "high"`) is domain knowledge the compiler must not carry
  unless a pass reads it. None does.
- The real defect it was supposed to fix — `condition_control` accepting
  `u1.gpio0 = banana` — has a smaller, general fix: **the non-pin operand
  of a mechanism condition must be a voltage literal**. See the
  validation slice below.

Revisit only if ERC drive-level checks land and need a resting-level
concept; then the level vocabulary grows from stdlib declarations, never
hardcoded in Rust.

### Amendment 2026-09-22 (D16 mechanism condition validation)

`condition_control` previously pulled a pin from either side of a `when`
condition's `BinaryOp` and ignored the other operand — so
`u1.gpio0 = banana` was indistinguishable from a voltage comparison and
silently synthesized a bridge. The gate: the non-pin operand must be a
voltage literal (`UnitLiteral`, e.g. `u1.gpio0.voltage == 3.3V`).
Anything else — an identifier (`banana`, `high`, `low`), a plain number
(`= 3.3` without `.voltage`), an expression — is a hard error naming the
expected shape: *"mechanism condition must be a single pin voltage
comparison (`u1.gpio0.voltage == 3.3V`), or a mechanism-bodied
`when … via Type;`"*. The `.voltage` field access is what makes a
condition a *voltage* claim.

`x = high` / `x = low` level sugar remains deferred (Decision above):
`high`/`low` are identifiers, so they fail this gate with the shape error —
the fix is general and needs no level vocabulary.

### Amendment 2026-09-22 (VI): `chain`/`into` landed in the CORE

The electronics `chain`/`await` sequencing sugar is now a core-language
construct (plan 2026-09-22-core-chain-into). `chain name [base-guard] {
action; into cond; action; ... };` desugars at parse time to ordinary
reactor nodes — one per step, step N's pre = base ∧ all prior `into`
conditions, post `[true]`. Works in `.bv` (software) and `.ebv`
(hardware); `derive_netlist` consumes the desugared nodes unchanged.

**Keyword unification (one keyword, one meaning, language-wide):**

| Concept | Keyword | Decision |
|---|---|---|
| Chain block | `chain` | core keyword (new) |
| Chain sign-off | `into` | core keyword (new) — NOT `await` (task-await stays main-only) |
| Persist-tighten | `bind` | settled — NOT `keep` (ownership stays main-only); `fix` rejected as a false friend ("repair") |
| Commit-select | `store` | settled |
| Disconnection (D16 p3b) | `open` | settled — "these two would interact if connected, but the wire is open; analyse as such" |

The D9 bare-`trg` sign-off shorthand is removed: authors write
`into pwr_btn;`, never bare `pwr_btn;`. `bind`/`store`/`open` remain
future slices (the keywords are settled here, not implemented).

Open under D16: phase-3b (author-expressed disconnection via `open`),
`bind`/`store` lifting slots, asymmetric switch parts, ERC class
semantics, whole-bus equality.

### Amendment 2026-09-22 (VII): `bind`/`store`/`open` IMPLEMENTED

The settled keywords are now live in the electronics dialect (node bodies):

- `bind a.pin = b.pin;` — persist-tighten: the net membership must hold in
  every solution. Unions the pins with `bound` provenance (D14 slot 1).
- `store inst.field = value;` — commit-select: picks one BOM value,
  recorded with provenance (D14 slot 2).
- `store net(pin) = "name";` — names the derived equivalence class; the
  name folds into net resolution with the same one-net-one-name conflict
  rule as `net <name>:` annotations.
- `open a.pin, b.pin;` — author-expressed disconnection (D16 p3b): the
  pins are NOT connected. If a wiring fact, bind, or mechanism bridge
  would tie them, it is a hard error — the complement gate to the
  phase-3 redundancy check (which catches unconditional copper defeating
  a mechanism; `open` catches a declared disconnection defeated by copper).

All are contextual keywords in node-body statement position (like
`yield`/`check`). `bind`/`store`/`open` now SHADOW function/identifier
names in node bodies — an `open()` action inside a chain must be renamed
(use `release()`, etc.). This is the intended cost of the unified
vocabulary; the design record's fixture uses the new spellings.

D16 p3b is now closed: the cross-region complement check has both halves —
mechanism redundancy (unconditional copper defeating a switch) and
author-expressed disconnection (`open`).

Open under D16: asymmetric switch parts (relay coil/contact), ERC class
semantics, whole-bus equality.

### Amendment 2026-09-22 (VIII): RETRACT the lifting slots — honest subtraction

`bind`, `store` (both forms), and the `net <name>:` annotation were
rejected on review (plan 2026-09-22-retract-lifting-slots.md). The
engine is deterministic — single solution, no value solver, no candidate
enumeration — so "persist-tighten" and "commit-select" presume machinery
that does not exist. Rule 2: if the compiler could have inferred it, the
keyword is a bug report — here the keywords themselves were the bug.

| Construct | Why removed |
|---|---|
| `bind a.pin = b.pin` | A plain body wiring fact `a = b` (both pins) does the identical union. `bind` only changed proof wording. |
| `store inst.field = value` | Inert: physics (series Ohm's law) and the emitter read the `let` literal, never the store. |
| `store net(pin) = "name"` | Duplicated `net <name>:`, which was itself redundant. |
| `net <name>:` annotation | Nets are identified by physics — derived voltage (`net_voltage`) and `Supply`/`Return` pin classes. A name asserts what the compiler derives; a name contradicting physics would be a lie we'd trust. |

Nets are now named by WHAT they are: the emitter labels a return-class net
`GND`, a driven supply net `V{volts}`, everything else `N#`. The
`net_conflicts` check is gone (physics cannot conflict with itself).

**Kept: `open a.pin, b.pin;`** — the sole genuinely non-inferable
construct: a NEGATIVE constraint. The netlist is the transitive closure
of positive wiring facts; "these two must NOT connect" can never be
inferred from absence. The complement gate to the phase-3 redundancy
check. D16 p3b remains closed.

D14's persist-tighten / commit-select slots are **deferred until a
solver exists** — then, and only then, do the keywords have a substrate.

### Amendment 2026-09-22 (IX): unpop/shortcircuit landed; sacrificial deferred

Slice B of plan 2026-09-22-electronics-participation-and-when-law.md:

- `unpop <inst>;` — participation fact: part excluded from the BOM
  (`in_bom no`) but the board is verified in BOTH present and absent
  configurations. Absent-state pins are open, Nc-exempt from dangling.
- `shortcircuit unpop <inst>: <Type>;` — acknowledged intentional short:
  suppresses the present-state shorted-supply error. `shortcircuit` on a
  populated part → warning with the suggest-`unpop` hint.
- `type Wire` in std/electronics.bv — a jumper is an unpop'd Wire.

**DEFERRED — `sacrificial` / melting point (B3/D17):** the future
suppressor of the `shortcircuit`-on-populated warning. Requires a thermal
model (proven dissipation exists per part; heat SPREADING and melt ordering
do not — the compiler claims schematic-level truth only). Trigger: a part
whose rating passes electrically but fails thermally, or a fuse/convention
slice. When it lands, a `sacrificial` part with a declared melting point
below every other part on its net suppresses the populated-short warning
only if the compiler can PROVE it is first to go — the same
convention-checker pattern as E13.

Open under the when-law (Slice C): electronics conditional drives and the
software member-fact law, both at top level / type / obj only (defn/node/
txn when unchanged).

### Amendment 2026-09-22 (X): the static when law LANDED (Slice C)

The `when` law is live in BOTH worlds at top level / obj / type — X implies
Y, the compiler makes it so (position decides semantics; defn/node/txn when
unchanged). Electronics facts are conditional drives (guard-aware
shorted-supply, per-instance type-body inheritance); software facts force
members (`analysis/when_law.rs`), contradicted by any node assignment or
other law under a jointly-satisfiable guard → refusal. The shared
satisfiability probe now understands unit literals, pin-access chains, and
opposite numeric comparisons on the same lhs (the Rule-22 precision slice).

The when-law is the conditional twin of `spec` (static property vs.
conditional fact) and of the D14 lifting slots (which were retracted for
lacking a solver substrate — the when-law HAS one: contradiction
detection, which is a proof, not a solver). D15 boundary holds: a law must
be algebraic/conditional, never a sequential firmware model.

Open under D18: whole-bus equality (ranges over pin arrays), asymmetric
switch parts (relay coil/contact), ERC class semantics — all unchanged from
the pre-when-law ledger.

### Amendment 2026-09-22 (XI): the three remaining gaps LANDED

Plan 2026-09-22-erc-relays-bus-equality.md. The last electronics-dialect
open items are closed:

1. **ERC contention (D6/D12).** `IoOd` gains `spec WiredAnd: true;`
   (open-drain wired-AND — released = high-Z, so multiple IoOd pins may
   SHARE a driven net). `check_contention` refuses a net with two
   drive-capable pins where at least one is not WiredAnd — a short, the
   machinery D6 promised. A `shortcircuit unpop`-acknowledged net is
   exempt. Property-driven: no class names in the compiler.
2. **Asymmetric switch parts.** The mechanism rule accepts 1 or 2 Control
   pins: a gate/FET uses one; a relay's two-pin coil is one element across
   both coil pins — `synthesize_one` unions the condition net to every
   control pin. `is_mechanism` requires every control pin unconnected or on
   the condition net. A relay is just a type with 2 Control + 2 Path pins;
   no new vocabulary.
3. **Whole-bus equality.** `u2.gpio[0..=3].voltage == u3.data[0..=3].
   voltage` in a precondition expands to N element unions. Half-open
   `[0..3]` = 3 elements; inclusive `[0..=3]` = 4. A length mismatch or
   empty/reversed range is a hard `bus_error`.

Ledger fully closed: the electronics dialect's intent-synthesis surface
(node/chain/into/when-law/unpop/shortcircuit/open/Wire, classes,
contention, relays, buses) is implemented. D18 remains open only for
ERC-property breadth (more class behaviors when a consumer needs them) —
the machinery is generic.

### Amendment 2026-09-24 (XII): component laws — physics parameter foundation

Plan `2026-09-24-electronics-component-laws.md`. The electronics behavior
direction shifts from implicit part recognition to author-declared
constitutive equations in type-body `when` laws. The compiler learns
equations, dimensions, guards, KCL, and operating-point diagnostics;
component types remain stdlib vocabulary.

Landed in Slice 1:

- centralized physical-suffix parsing for specs and expressions, with
  canonical full-word units (`330Ohm`, `4.7kOhm`, `20mAmp`);
- `spec Resistance: Ohm;` as a dimensioned parameter declaration and
  `spec Resistance: 4.7kOhm;` as a typed default;
- component literals accept `spec <Name>: <quantity>;` and carry the
  value as structured SI + dimension, separate from opaque BOM fields;
- structured resistance takes precedence over the legacy numeric-`value`
  heuristic (which remains only as a migration path).

Next slices: law IR/linear elaboration, unguarded DC solve, guarded
piecewise modes, stdlib/fixture migration, then retirement of the
`value`-as-ohms path. This is the substrate rail inference should build
on, not a new keyword family.

**Slice 2 addendum (same day):** `analysis/electronics_laws.rs` now turns
pin-bearing type-body laws into validated per-instance linear IR. It
supports linear +,-,*,/ over pin voltage/current and Volt/Amp/Ohm spec
constants, single linear comparison guards, and polymorphic literal zero.
It rejects nonlinear terms, dimension conflicts, missing parameters, empty
laws, and duplicate equations as hard law errors. The netlist carries the
IR; the emitter refuses invalid laws. The DC solver is not yet attached.

**Slice 3 addendum (same day):** `analysis/electronics_dc.rs` now solves
always-active law groups deterministically. Contract drives are boundary
constants; non-boundary nets get KCL; Gaussian elimination reports
contradictory and underdetermined groups instead of choosing a point.
Solved voltages/currents feed tolerance/current checks, and law-bearing
types are excluded from the legacy series heuristic to prevent double
counting. Guarded modes remain for Slice 4.

**Slice 4 addendum (same day):** the DC solver now enumerates guarded
branch modes deterministically (bounded at 12 guarded laws per connected
group). Each candidate must satisfy every guard at its selected active /
inactive truth value; zero modes and multiple distinct modes are hard
diagnostics. Negative unit drives are recognized as boundary conditions.
A ideal-diode forward/reverse pair solves to the correct branch in tests.
`spec Bistable` remains future surface until multi-state contract checking
lands; the solver refuses to choose among distinct states.

**Slice 5 addendum (same day):** generic component law parameters landed
(`spec ForwardVoltage: Volt;`, `spec DynamicResistance: Ohm;`, quantity
defaults) with component-body scoping and declaration-shadowing semantics.
Stdlib Resistor, Wire, Diode, and Led are now law-bearing declarations.
`led_blinker.ebv` runs the law solver end-to-end and proves its LED
current. `usb_sensor.ebv` uses structured resistance but deliberately
keeps its local Resistor law-free until an SPST button can express
conditional-topology state; therefore the legacy series-value fallback is
still present, now narrowly as that migration path.

### Doctrine note 2026-09-24: ASCII units + physics layer split

The electronics physics split is two-layer. The compiler owns the eternal
substrate: dimensions, quantity normalization, dimensional algebra
(including `Ohm == Volt / Amp`), law elaboration, guards, KCL, and the
deterministic DC solve. Component declarations own their own constitutive
equations; stdlib defines `Resistor`, `Wire`, `Diode`, and `Led`, and the
compiler has no catalog match arms for component names.

Unit spelling is ASCII-first, not verbose-first. `V`, `mA`, `R`, `kR`,
`4k7`, and full-word aliases such as `mAmp` and `kOhm` are all valid.
`Ω` and other non-ASCII/Greek-like unit symbols are rejected; the ASCII
metre prefix `u` replaces `µ`. This supersedes the temporary spelled-
`Ohm`-only rule from the first component-law slice.

### Amendment 2026-09-24 (XIII): law-proof closure

The component-law DC solve is now upstream of all current/power proof
passes. Law-bearing components derive `P = Σ V·I` from their solved
operating point and are subject to rating decisions; source budgets
consume solved branch-current magnitudes; lower current bounds are checked
against signed pin current. Proof facts record component-law provenance.
An unpopulated law-bearing part is rejected until dual-state DC solving
exists — never passed by silently solving only its absent state.

### Amendment 2026-09-24 (XIV): multi-state law solving

The DC substrate now models explicit operating states. Guarded group
candidates combine into a bounded Cartesian product of global states;
`spec Bistable: true` is the authority required on every guarded-law
contributor in an ambiguous group. Tolerance, current-bound, law-power,
and budget proofs run per state, and diagnostics carry deterministic
state labels. A law-bearing `unpop` component is solved in mandatory
present and absent participation states; the absent state removes its
laws and records the omission. The compiler never selects among states
and never certifies a contract from one state while another violates it.
