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
  deterministic emission.
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
