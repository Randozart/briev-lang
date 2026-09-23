# Intent-Synthesis Node Semantics — Electronics Briev Design Record

**Date:** 2026-09-21
**Status:** DESIGN RECORD — decisions locked, implementation staged (E14a/E14b).
Grammar spellings are provisional until implementation lands; this document is
the spec-of-record for the future `.ebv` surface.
**Companion:** `2026-09-21-hardware-dialect-gaps.md` (gap registry — E-track
reframed by this record; S-track unchanged).
**Drivers:** ternary crossbar demonstrator (JLCPCB-fabbable), Project Bachi
(KV260, silicon), and a real IdeaPad 330 motherboard as the board-class
ground truth.

---

## 0. Doctrine

The electronics surface is **reactor-native by construction**. No new
execution model, no temporal engine, no board-state machinery: the board
*is* a reactor; nodes fire on guards; the frontier of eligible nodes is the
board state. Everything new is inference (membership search) and proof
(per-firing physics fixpoint) — both extensions of existing machinery.

Expressiveness closure holds: the compiler's optimum must be expressible in
the language, so the electronics surface consumes briev's own primitives —
nodes, cells, `trg`, Rule 22 classification, `vol` semantics — and adds
nothing that a dialect special case could own (Rules 14/15/23/24). The
compiler never learns the words "crossbar", "rail", "sequencing", or
"power-on". Catalog knowledge lives in stdlib types and cells; the compiler
keeps proofs and inference machinery.

## 1. Final author surface

```
types      pins (classes, vol, spec facts) + physics vocab (tolerance/rating)
cells      reusable composites (ports + internal instances/relations/nodes)
instances  mass arrays + singles + population facts
trg        board inputs (external reality; wake sources)
nodes      guards + drive maps  (chain sugar available)
lift       RETRACTED (2026-09-22): persist-tighten/commit-select had no
           solver substrate; bind/store were the keywords, now gone
```

**Nets are never declared. Never written. Fully inferred.** Net labels are
derived from physics (return-class → `GND`, driven → `V{volts}`, else
`N#`) — emitter concern only.

### What this kills (recorded so it stays dead)

| Rejected design | Why |
|---|---|
| Net/node declarations with membership | inference is sole topology truth; declarations duplicate it |
| Budgets on nets | budgets attach to source **pins** |
| Bus-resolution keywords | pin classes answer it: `io_od` = wired-AND by definition; two `io` on one net = error |
| Board-state/txn-family machinery | reactor frontier already is the state |
| Hierarchy-as-sheets (authoring) | cells own composition; emitter-side multi-sheet output remains backend work |

## 2. Decision record

**D1 — Volatile pins.** `pin` declarations may carry `vol`: the value is
owned by external reality (chip pin, rail sense). The compiler re-reads per
firing, never assumes constancy across firings, and requires a declared
domain (levels/ranges). The board does not own `chip.POWER_ON`'s internal
state — it owns the *drive relation under guards*.

**D2 — Nodes are the board region.** `node name [guard] { drive-maps };`
— guard defines when the region is live; body states outcomes that must
hold during firing. Wiring and physics are inferred per firing.

**D3 — Nets fully inferred.** Equivalence classes derive from every
constraint that forces two pins to share potential (drive relations,
correlation invariants, mechanism paths). Author-visible only in the
wiring report and annotated KiCad output, each connection carrying
provenance (which fact forced it).

**D4 — Budgets on source pins.** `budget u1.out <= 250mA;` — roll-up =
sum of all loads reachable from a source pin vs. its budget (analysis
machinery unchanged from E7's original design; only the attachment point
moved).

**D5 — `spec` clauses are the datasheet channel.** Catalog types carry
solver-consumable facts: `spec fb_ref: 0.8V;`, `spec decouple: 100n;`.
The compiler consumes these generically — never a part name (Rule 15).

**D6 — Pin classes are the physics vocabulary.**
`power` / `ground` / `in` / `out` / `io` / `io_od` / `nc`. Class semantics:
`io_od` = open-drain = wired-AND resolution **by definition** (released =
high-Z); two `io` pins on one net = contention error; `nc` exempt from
dangling-pin checks; classes key ERC and map to KiCad electrical pin types.

**D7 — Conditional-mechanism rule.** Copper is unconditional; a tie can
only carry a **state-level** condition. A guard term that *varies within a
firing* and gates an outcome forces a **mechanism**: a switching element
(transistor, gate) from the declared instances whose pin contracts
implement the conditional. No eligible part = compile error naming the
missing capability. Detection = cross-node complements: if any node's body
says `y = low` under a guard not implied by `y = high`'s guard, the
relation is conditional.

**D8 — Bistability classification.** A DC fixpoint may have multiple
solutions (Schmitt, latch-up). The solver must detect non-uniqueness and
demand a `bistable` declaration or a uniqueness proof. Over-approximate
detection is acceptable: a false positive costs one declaration; a false
negative costs a false proof. Analog of Rule 22: NO IMPLICIT BISTABILITY.

**D9 — Chains.** `chain name [base-guard] { ... };` desugars to plain
nodes; two statement kinds: *actions* (`u.en = high;` → auto-node with
accumulated guard) and *sign-offs* (`into u.pgood;` → ANDed into all
subsequent guards; the bare-`trg` shorthand `pwr_btn;` is REMOVED —
electronics authors write `into pwr_btn;`). Sign-offs name **real pins**
(`out`-class), never invented pseudo-facts. Steps are ordinary nodes
post-desugar:

- outcomes **broadcast wakes** to every subscriber (wake sets) — a fact
  guards the next step, a fan-controller node, and a different chain with
  equal standing; cross-chain causal edges are existing reactor machinery
- chains are **resumable by construction**: guards reference physical
  facts, so warm boot / brown-out lights the frontier mid-chain; ordering
  cannot be violated because every action still requires its full
  accumulated guard
- chains carry **zero private semantics** — pure grouping ergonomics;
  general across dialects (core desugarer pass, additive)
- **2026-09-22 (plan 2026-09-22-core-chain-into): IMPLEMENTED in the
  core.** `chain`/`into` are core keywords, not electronics-specific.
  Parse-time desugar (pipe-chaining precedent): `chain` never reaches the
  AST; each step becomes a reactive node `name_1, name_2, …`, step N's
  pre = `base ∧ s₁ ∧ … ∧ s_{N-1}`, post `[true]`. `await`/`keep` are NOT
  reused — they already mean task-await and ownership-transfer in the
  core (one keyword, one meaning). A trailing `into ...;` is an error (a
  sign-off gates a later step; a final one gates nothing).

**D10 — Cells are hierarchy and catalog.** `cell` = reusable composite:
ports, internal instances, internal relations/nodes. The component catalog
upgrades from part types to reference designs — a USB port *with* its ESD
and CC logic, a buck stage *with* its divider, an x86 power-up *chain*. An
app note is a cell. Temporal knowledge lives in the library; the compiler
stays eternal (Rule 24).

**D11 — `trg` = board inputs.** External power source, button, attach
event: wake sources, volatile by nature.

**D12 — Rule 22 governs contention.** Two simultaneously-eligible nodes
with conflicting drive maps on one pin = XOR overlap → classification
demand: `async` (= wired-AND legality, `io_od`) or `sync<g>` (= mutual
exclusion by construction). Nothing new; the concurrency machinery *is*
the board machinery.

**D13 — Ambiguity policy.** Hard error + candidate enumeration. The
enumeration runs over **behavioral equivalence classes**: two wirings are
one candidate iff no contract distinguishes them (under-enumerate = false
proofs; over-enumerate = noise that makes users ignore diagnostics). Two
standing guards: **the solver never invents components** (it wires declared
instances only; adding parts is author work), and the **well-posedness
gate** — every pin must be constrained by ≥1 behavior/invariant/bind/
store/population-fact, else error listing the unconstrained pins (copper
the spec doesn't cover must not ship).

### The ambiguity surface (enumerator spec, ordered by resolution stage)

| # | Site | Resolution stage |
|---|---|---|
| 1 | Structure existence (is an element forced) | physics proofs — free |
| 2 | Net membership (which pins connect) | a body wiring fact (`a = b;`) states it; `bind` retracted 2026-09-22 (duplicated the wiring fact) |
| 3 | Value choice (which R; free variable with bounds) | the `let` literal carries it; `store` retracted 2026-09-22 (inert until a value solver exists) |
| 4 | Participation vs DNP (unconnected instance) | population fact required for exemption; else error listing *populate-or-DNP* |
| 5 | Cross-node drive conflicts | Rule 22 classification, else error |
| 6 | Polarity/orientation | usually derived from intent (led "on" ⇒ forward bias); under-constrained parts surface candidates |
| 7 | Over-constraint (keeps that kill all solutions) | UNSAT error naming the **minimal conflicting set** |
| 8 | Equivalent variants (series order, symmetric pins) | canonicalize silently, deterministically |

**D14 — Lifting slots.** *persist-tighten* and *commit-select* were
proposed to resolve the ambiguity surface (which pins connect; which
value). **2026-09-22 (plan 2026-09-22-retract-lifting-slots): RETRACTED.
** The engine is deterministic — single solution, no value solver, no
candidate enumeration — so the slots presume machinery that does not
exist. `bind` duplicated a plain body wiring fact; `store` was inert
(the `let` literal carries values and physics reads them); `store
net(...)` and `net <name>:` asserted names the compiler derives from
physics. Per Rule 2, the keywords themselves were the bug. **Deferred
until a solver exists.** The one survivor is the D16 p3b disconnection:
`open a.pin, b.pin;` — a NEGATIVE constraint, never inferable from the
positive wiring closure; the complement gate to the phase-3 redundancy
check. Derived facts (`derive on: a.current >= 2mA;`) remain a separate
construct — proven properties, not intents.

**D15 — Firmware = extern boundary.** Chip-internal logic (EC firmware)
is outside the proof surface: the EC appears as a component whose pins
obey declared per-state pin contracts — a volatile black box, the board
language's `extern`. Non-goals, recorded: PCB layout, length matching,
impedance/signal integrity, transient/time-domain simulation.

**D18 — The static `when` law (2026-09-22, plan 2026-09-22-electronics-
participation-and-when-law).** `when G { F₁; …; Fₙ }` declares
`G ⟹ F₁ ∧ … ∧ Fₙ`, and the compiler must make it so. Meaning is decided by
**position**:

- Inside a `defn`/`node`/`txn` → guarded/reactive behavior (unchanged).
- At top level / in an `obj` / in a `type` → a **static forced fact**: the
  compiler propagates the consequence and verifies consistency — anything
  that contradicts an in-force fact under a satisfiable guard is a refusal.
  If even one satisfiable state escapes, the compile refuses.

"Make it so" = propagate + verify, never synthesis (the solver adds no
parts — D13). Two consumers, same law:

- **Electronics** (C1): law facts are conditional drives joined into
  `classify_drives` — two in-force drives at different voltages under
  jointly-satisfiable guards are a shorted supply; mutually-exclusive
  guards never conflict. Type-body laws are inherited per instance
  (bare pin refs qualified to `inst.pin`).
- **Software** (C2): a law fact forces a member; a node/txn body assignment
  or another law that forces it differently under a jointly-satisfiable
  guard is a refusal (`analysis/when_law.rs`).

The guard-satisfiability probe (`check_satisfiable`) understands unit
literals, pin-access chains, and opposite numeric comparisons on the same
lhs (`x > 100` vs `x <= 100` disjoint) — this is the Slice-A Rule-22
precision work, shared by the concurrency gate and the when-law gate.

## 3. Fixtures (gate evidence)

### 3.1 Chain example (sequencing, D9)

```ebv
trg dc_present;
trg pwr_btn;

chain power_up [dc_present] {
    u_buck5.en = high;          // action  → node power_up_1
    into u_buck5.pgood;         // sign-off → guard term for all later steps
    u_buck3.en = high;
    into u_buck3.pgood;
    into pwr_btn;               // external trg joins the guard (no bare form)
    u_core.en = high;           // final step → node power_up_3
};
// power_up_3 (pgood fact) is a wake source for a fan chain, a charger
// chain, and the next step — equal standing, no fan-out syntax.
```

### 3.2 USB sensor board (E14a/E14b gate fixture — pure intent form)

```ebv
// usb_sensor.ebv — NO nets declared. NO wiring written.
type Ldo {
    pin in: power;  pin gnd: ground;  pin out: power;  pin en: in;
    reference "U";  tolerance 0.33;  rating 0.9;
    spec decouple: 100n;
};
type Mcu {
    pin vdd: power;  pin gnd: ground;
    pin gpio[8]: io;                    // E11 pin array
    pin sda: io_od;  pin scl: io_od;    // wired-AND by class definition
    pin swdio: io;   pin swclk: in;     pin pa0_unused: nc;
    reference "U";  tolerance 0.33;  rating any;
    spec decouple: 100n;
};
type Sensor {
    pin vdd: power;  pin gnd: ground;
    pin sda: io_od;  pin scl: io_od;    pin addr: in;
    reference "U";  tolerance 0.3;  rating any;
    spec decouple: 100n;
};
type Resistor  { pin a; pin b; reference "R"; tolerance any; rating 0.25; };
type Capacitor { pin a; pin b; reference "C"; tolerance any; rating any; };
type Switch    { pin p1; pin p2; reference "SW"; tolerance any; rating any; };
type Led {
    pin a;  pin k;
    reference "D";  tolerance 3.6;  rating any;
    derive on: a.current >= 2mA;        // derived fact: proven, not asserted
};

let j1: UsbMicro = UsbMicro { value: "USB-Micro-B"; package: "SMD" };
let u1: Ldo      = Ldo      { value: "AP2112K-3.3"; package: "SOT-25" };
let u2: Mcu      = Mcu      { value: "STM32F042";   package: "QFN-32" };
let u3: Sensor   = Sensor   { value: "SHT40";       package: "DFN-4"  };
let led1: Led    = Led      { value: "green";       package: "0603"   };
let r_led: Resistor = Resistor { value: "TBD";       package: "0603"   };
let r_pu[2]: Resistor = Resistor { value: "4k7";     package: "0603"   };
let r_btn: Resistor = Resistor { value: "10k";       package: "0603"   };
let sw1: Switch  = Switch   { value: "SPST";        package: "SMD"    };
let c[i:5]: Capacitor = Capacitor { value: "100n";     package: "0402"   };
let c_dnp: Capacitor  = Capacitor { value: "10u";      package: "0805"   };
populated = false: c_dnp;               // pad exists, part absent

budget u1.out <= 250mA;
budget j1.vbus <= 500mA;

trg usb_attached;

node usb_powered [usb_attached && j1.vbus.voltage == 5.0V] {
    u1.enabled = true;          // en wiring inferred (candidates exist)
    led1 = true;                // physics derives the series resistor;
                                // io-class pruning picks the driver
}
node i2c_idle [usb_powered.up && u2.sda.released] {
    u2.sda.voltage >= 2.7V;  u3.scl.voltage >= 2.7V;
    // io_od physics: released = high-Z ⇒ pull-ups to a ≥2.7V source are
    // FORCED; solver wires r_pu[*] to u1.out. No resolution keywords exist.
}
node button_pressed [usb_powered.up && sw1.closed] {
    u2.gpio[3].voltage <= 0.3V; // forces a gnd path
}
```

**Gate:** compiles with the stated intents; each stated omission is a hard
error with enumerated candidates (undeclared driver for `led1` →
membership candidates; `sw1` path without an explicit gnd wiring →
membership candidates; remove a decoupling cap → convention error; an
`open` naming two wired pins → disconnection error).

### 3.3 IdeaPad power tree (motherboard-class fixture — STUB)

To be materialized from probing the real board (first implementation-phase
task). Skeleton from initial inspection: DC-in 20 V → protection → buck
stages (20→5 ALW, →3.3 ALW/RUN, →1.8) as buck **cells** with FB dividers
derived from `spec fb_ref` + `rail = 5.0V ± 5%` behaviors; EC as volatile
chip (firmware boundary); pgood→en chains as `chain` sugar; signal-level
conditions (buttons, PGOK drops) forcing mechanisms per D7. Target: ~150
instances, multi-sheet KiCad output, BOM. This fixture is the
motherboard-class gate.

## 4. Implementation ladder

| Stage | Scope | Gate |
|---|---|---|
| **E14a** | intent-*completion*: explicit equalities still allowed; drive-map intents (`led1 = true`) infer the remaining memberships; ambiguity = enumerated-candidate errors; pin classes, vol, spec, population, chain desugar; lifting slots RETRACTED (2026-09-22) — no solver substrate | §3.2 fixture compiles + its error matrix; netlist/KiCad deterministic — **GATE PASSED 2026-09-23** (see gate delta below) |
| **E14b** | pure intent: no explicit wiring equalities; guards + behaviors only | §3.2 written without any net/`==` topology; §3.3 materialized — **slices 1–2 landed 2026-09-23** (min pull-up forcing, max low-hold forcing); remaining: bus assembly, en drive solving, rail inference, value-aware resistor matching, `derive on:` |

Both stages: `L`. E14a ships useful even if E14b stalls (strict
generalization order). Prerequisite gap work (E11 pin arrays, E12 pin
classes, E13 spec-clause convention checks, E7 budgets-on-pins) lands as
separate additive passes per the gap registry.

### E14a gate delta (2026-09-23) — what the fixture needed beyond pure intent

The §3.2 fixture (`examples/electronics/usb_sensor.ebv`) compiled with the
translation table of plan `2026-09-23-ebv-gate-fixture.md`. Every site
where E14a had to state MORE than §3.2's pure-intent form is an E14b
backlog item, each marked `// E14b:` in the fixture file:

1. **Rails and returns** (VBUS, 3v3, gnd) stated as guard equalities —
   E14b deletes them (pure intent states behavior only).
2. **io_od pull-up placement** (`r_pu[*]` to `u1.vout`) stated explicitly —
   §3.2 relied on solver-side forcing of the released (high-Z) net; E14b
   wires `r_pu[*]` from the pull-up physics alone.
3. **`u1.en` wiring** — stated as an explicit fact so the `u1.en = true;`
   pin intent records "already connected". §3.2's `u1.enabled = true;`
   intended the en line to infer; with the LDO input driven, E14b's
   solver is the honest home for that inference (pin-level drive intents
   DO complete when the pin is open — the fixture pre-wires to keep the
   completion unambiguous).
4. **Button gnd path** (`u2.gpio[3] → sw1.p1 → u2.gnd`) stated explicitly —
   §3.2 relied on `u2.gpio[3].voltage <= 0.3V` forcing the gnd path.
5. **LED series resistor placement** (`r_led` between 3v3 and led1.a)
   explicit — §3.2's derive-on-type obligation would place it.
6. **SWD header (j2)** ADDED: §3.2 declares `swdio`/`swclk` but no
   connector — as written they would dangle. The header is the fixture's
   one structural addition.
7. **`derive on:` (type-level) has no landed clause** — the fixture states
   the obligation as a use-site txn postcondition; recorded as an open
   gap in the hardware-dialect ledger.

Behavior members §3.2 wanted (`.up`, `.closed`, `.released`) stayed
retracted — guards restate them as voltage facts (locked 2026-09-22).

## 5. Documentation chain (at implementation time)

SPEC §3.5 gains a non-normative "Planned: intent synthesis" pointer (done
in the same commit batch as this record); syntax-highlighter rules,
learn-briev tutorial, and `docs/architecture/electronics-frontend.md`
refresh land with the E14a commits per the docs-same-commit rule.

---

## Amendment 2026-09-21 — D6 reworked: pin classes are fundamentals, not keywords

The first implementation slice (E12) began with pin classes as a compiler
keyword set (`pin vbus: power;` — lowercase clause vocabulary). Review
flagged the casing inconsistency, and underneath it the wrong ontology.
D6 is restated:

- The initial `PinClass` enum and parser keyword set are **retracted**
  (never shipped past a worktree).
- Seven parentless fundamentals join `std/electronics.bv` beside
  `Volt`/`Amp`, prelude-injected: **`Power`, `Ground`, `In`, `Out`,
  `Io`, `IoOd`, `Nc`**.
- Grammar: `pin <name> [: <TypeName>] [= <int>];` — the parser stores a
  type reference and nothing more; resolution happens in analysis against
  the imported fundamentals.
- Emitter and ERC behavior are **property-driven**: each fundamental
  declares its facts as `spec` clauses on the type —
  `spec KicadType: "power_in";` (`input`, `output`, `bidirectional`,
  `open_collector`, `no_connect`, `passive`), and `Nc` adds
  `spec NoConnect: "true";` (dangling-pin exemption).
- The compiler learns **zero class names** (Rules 14/15): the emitter asks
  the resolved type for properties, generically. Authors may declare new
  pin-class fundamentals with properties later — no compiler release.
- D6's behavioral semantics land as spec properties **with their consumer
  slices**: `IoOd` gains `spec wired_and: true;` when contention
  classification consumes it; `io` exclusivity likewise. Properties-only
  now; semantics ship with the machinery that enforces them.
- Effort re-rate: E12 S→M (type-universe resolution plumbing in analysis
  + emitter).

Rationale: `pin vbus: Power;` reads as a type ascription, exactly as §3.5
promises for fundamentals ("the fundamentals are physical quantities").
A closed keyword set would be compiler vocabulary the language cannot
extend — the precise failure Rule 14 exists to prevent.

---

## Amendment 2026-09-21 — D16: conditional wiring (`when` in node bodies)

Body facts are wiring; a `when` around them asks for CONDITIONAL wiring.
Two honest readings, one trap:

**The trap.** Copper cannot vary. A `when x.voltage == 3.3V { a = b; }` that
silently produced an unconditional union would lie — always-connected
copper emitted for a conditional request. The gate: conditional wiring
facts whose condition is SIGNAL-LEVEL (references a pin — pins are
volatile reality) require a MECHANISM; without one, hard error naming
the missing capability (D7 made constructive). Region-level conditions
(state facts, no pin reference) carve a sub-region: the facts are
ordinary wiring, unconditional in that region — exactly the nested-node
decomposition `node N [G] { when H { f } } ≡ node N_H [G ∧ H] { f }`.

**Reading 1 — region assertion.** The when carves the node's firing
region; facts inside apply to it. Where nothing requires disconnection
elsewhere, always-connected copper is a valid implementation.

**Reading 2 — mechanism synthesis (the prize).** The condition drives a
declared switching part: control pin ← the condition's net, path
terminals ← the wired pins. This is the enable-chain semantics
(`when u_en.voltage == 3.3V { chip.POWER_ON = u1.gnd; }`) made
constructive. Needs switch-part vocabulary: `spec Control: true;` on
gate-class pins, switchable path pins — a later property slice.

**Plan.** Phase 1: guarded facts seen and classified (closing the
silent-skip hole in body_facts); pin-referencing conditions demand a
mechanism (D7 error); region-level conditions (`true`, pin-free) apply
as ordinary facts. Phase 2: mechanism synthesis with the switch-part
property vocabulary. Phase 3: the cross-region complement check
formalized (connected in region A + required-disconnected in region B →
mechanism demand). Condition vocabulary: single pin voltage-comparisons
(`x.voltage == 3.3V`, `rail.ok`); level-name sugar deferred.

Guarded statements are `Statement::Guarded` — already parsed by the
core; this slice is analysis-only. Nested whens compound their
conditions (`when a { when b { f } }` ≡ conditioned on `a && b`).

---

## Amendment 2026-09-21 — D16 phase 2 finalized: mechanism synthesis via strategy clause

**Grammar** (additive, in the shared when-statement parse):
`when <cond> { <facts> } via <Name>;`
`via` is a contextual identifier; the selection desugars into the body as
a marker statement the analysis reads — zero new Statement variants.

**Selection semantics** (narrowing filter, never a silent pick — D13):
- No `via` → enumerate qualifying mechanisms: declared instances with
  exactly one `Control`-class pin + ≥2 `Path`-class pins, control
  unconnected or already on the condition's net. 1 → synthesize; 0/n →
  hard error with candidates + the `via Type` fix.
- `via T` → narrow to instances **of type T**: 1 → synthesize; 0 → error
  ("declare `let sw: T = …`"); n → error listing instances (pre-wire a
  control pin to disambiguate).
- Name resolution: type first, instance second (bare `via sw1;` exact-pick
  also works). Synthesis wires **declared instances only**.

**Vocabulary** (stdlib + two new spec-gate keys):
- `type Control { spec KicadType: "input"; spec Control: true; };`
- `type Path { spec KicadType: "passive"; spec Switchable: true; };`
- A switch type: exactly one Control pin, ≥2 Path pins. Property interface
  decides — the compiler never knows "MOSFET" (Rules 14/15).

**Synthesis**: condition = single pin voltage-comparison (a
`u1.gpio0.voltage == 3.3V` shape — the non-pin operand must be a voltage
literal; `x = high`-style level sugar is deferred, see the ledger);
condition pin's net feeds control; bridge request (A, B, control) →
three unions + a `conditional_bridges` record (the phase-3 complement
check and eventual per-region physics consume it) + proof provenance.
Path-side assignment canonical for symmetric parts. Emitter untouched —
the mechanism is ordinary copper. Conduction physics stays black-box
(D15): the compiler proves the wiring; the part's datasheet owns the
conduction.

**General rule now in force (D14 realization):** ambiguity diagnostics
REQUEST the strategy selection and name the candidates — the compiler
never picks silently, in mechanisms or anywhere else.

---

## Amendment 2026-09-21 — D17: model boundaries (the honesty ledger)

The schematic-level model is sound and honestly bounded. These are the
incomplete items we MUST account for later — each with its trigger and
its eventual home. None may be silently claimed as covered.

- **B1 — Parasitics.** Traces have R/L/C; every current proof assumes
  ideal wire. *Trigger:* first board that misbehaves in silicon, or the
  first budget check that needs trace-resistance terms. *Home:* layout
  domain (non-goal) with schematic-level budget hooks last.
- **B2 — Signal integrity / EMI.** A 20 V rail beside a 1.8 V sense line
  is a layout problem. *Trigger:* real-world failure or a standards
  requirement (EMC). *Home:* post-layout analysis; explicitly non-goal.
- **B3 — Thermal.** Dissipation is PROVEN per part (P = V × I vs rating);
  heat SPREADING (copper pours, vias, airflow) is not. *Trigger:* a part
  whose rating passes electrically but fails thermally. *Home:* layout +
  a future convention slice (thermal pad / pour requirements per part
  class, property-driven like E13).
- **B4 — Thresholds.** "High" is really "above VIH-min, worst case over
  temperature." The voltage-comparison vocabulary is the first honest
  cut. *Trigger:* first level-sensitive proof that must survive
  tolerance. *Home:* `tolerance` machinery + per-class threshold specs.
- **B5 — Test & safety provisions.** Test points, ESD structures,
  creepage/clearance on the 20 V input, fuse conventions. *Trigger:*
  first fab-worthy board (the usb_sensor or the IdeaPad-class gate).
  *Home:* convention slices — the E13 pattern (declared property,
  generic checker) extends directly.
- **B6 — Layout reality.** A real board is ~40% schematic, ~60% layout;
  most fabbed-board failures live in the deferred 60%. *Standing rule:*
  the compiler claims SCHEMATIC-LEVEL truth only — every verification
  output carries that scope. The ledger (this file + D16/D17
  amendments) is the record of what is and is not claimed.

D17 is a standing obligation: whenever a slice's proofs approach one of
these boundaries, the boundary is re-stated in that slice's output —
never silently crossed.
