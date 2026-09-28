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

## Annotation vs physics (2026-09-23 doctrine)

**The one rule:**

> If the compiler must READ it to prove the board works → physics →
> PascalCase spec key. If a human/manufacturer reads it to build the board
> → annotation → lowercase, opaque.

The compiler carries annotations; it never interprets them. Examples:

- `value: "4k7"` — read by the person placing the part → **annotation**.
- `reference "U"` / designator `U8` — read by the person at the PCB →
  **annotation** (means something only to that specific board).
- `package: "0603"` — **annotation**.
- Resistance, tolerance, rating, min/max current — read by the compiler
  to derive and prove current/voltage → **physics** → PascalCase specs
  (`Resistance`, `Tolerance`, `Rating`, `MinCurrent`, `MaxCurrent`, …).

Corollaries:

1. **PascalCase** for every compiler-intelligible spec key (`Supply`,
   `CanDrive`, `WiredAnd`, `Decouple`, `Decoupler`, `Tolerance`, `Rating`,
   `Resistance`, `MinCurrent`, `MaxCurrent`, `MinVoltage`, `MaxVoltage`).
   Grammar keywords are not spec keys and stay lowercase.
2. **Quantities are bare, never quoted.** `spec MinCurrent: 2mA;`, not
   `"2mA"` — quoted implies arbitrary. Quantity notation is core `.ebv`;
   spec values use the same unit grammar as expressions
   (`2mA`, `3.3V`, `100n`, `4k7`; scaling prefixes `p n u m k M G`,
   key-dimension resolution, dimension-conflict hard error). Canonical
   full-word units (`330Ohm`, `20mAmp`) are the spelling for new component
   physics.
3. **Resistance is physics, not a BOM label** — it moves to a spec; the
   compiler must never read an annotation to derive current.
   (Consequence: E14b-5's "values are immaterial for obligation
   satisfaction" is structurally true.)

Full record: `docs/plans/2026-09-23-quantities-and-annotation-doctrine.md`.

## Component laws (2026-09-24 direction)

Components are not compiler-recognized catalog entries. A component type
declares **constitutive equations** in type-body `when` laws; the compiler
consumes equations, dimensions, guards, KCL, and solution status generically.
`spec` supplies the law's constants (`spec Resistance: Ohm;`, instance
`spec Resistance: 330Ohm;`). Direction is emergent: symmetric equations are
bidirectional, guarded equations are directional. The first solver is
piecewise-linear DC and refuses zero/multiple operating points rather than
choosing silently. Full decisions and slice gates:
`docs/plans/2026-09-24-electronics-component-laws.md`.

Landed in Slice 1: centralized quantity parsing, canonical full-word units,
`ComponentInstance.specs` as structured SI + dimension, and a type-level
resistance default. Structured `spec Resistance` is now the ONLY physics
source — the legacy numeric-`value` heuristic was deleted 2026-09-24 (plan
`2026-09-24-retire-legacy-value-physics.md`); `value` is carried to KiCad
and never parsed.

Landed in Slice 2: `analysis/electronics_laws.rs` elaborates pin-bearing
type-body laws per instance into linear guard/equation IR. It resolves bare
pins and spec constants, dimension-checks expressions (`Ohm = Volt / Amp`),
treats literal zero as polymorphic notation, and rejects nonlinear terms,
unknown/missing parameters, dimension conflicts, empty laws, and duplicate
equations. The netlist carries `component_laws` and hard `law_errors`; the
KiCad gate refuses invalid laws.

Landed in Slice 3: `analysis/electronics_dc.rs` solves unguarded law groups.
Components are grouped by connected nets, fixed contract drives become
boundary constants, unknown net voltages and branch currents become matrix
variables, and KCL is added on non-boundary nets. Deterministic Gaussian
elimination proves one operating point or names the group as contradictory /
underdetermined with its free variables. Solved pin currents feed current
bounds; solved net voltages feed tolerance and downstream checks.
Law-bearing parts are solved from their law IR and never enter the series
graph, so there is no legacy value path left to double-count.

Landed in Slice 4: guarded laws use deterministic branch-mode enumeration
(bounded at 12 guarded laws per group). A mode supplies its active
equations; the candidate is accepted only when every guarded law has the
truth value selected for that mode. Zero valid modes is a hard error;
multiple distinct valid modes are reported, never silently collapsed.
Negative quantity drives (`-3.3V`) are boundary conditions.

Landed in Slice 5: component bodies accept generic dimensioned law
parameters (`spec ForwardVoltage: Volt;`), including quantity defaults.
Later component declarations shadow prelude declarations, so a user's
same-named local type cannot inherit stdlib laws accidentally.
`lib/std/electronics.bv` now declares resistor, wire, diode, and LED laws.
`led_blinker.ebv` uses the stdlib laws with structured datasheet physics
and proves its LED current. `usb_sensor.ebv` uses structured resistance
values; its local non-law Resistor remains intentional until the SPST
button has conditional-topology state.

## Deferred (recorded, not built)

Electrical pin roles (`[power_in]`), footprint validation against KiCad
libraries, serializer support for `PinDecl` (deserialize defaults to empty
— beast round-trip of pins is a follow-on), per-pin tolerances, LLVM/GPU
representation (awaits simulation), PinDecl in cell bodies' beast
serialization, series-resistor placement synthesis (choosing r_led so the
LED current lands in range — the residue of §3.2's `derive on:`, which
was superseded by the `spec MinCurrent`/`MaxCurrent` obligation form;
recorded as gap **E15** in the hardware-dialect ledger).

Landed since the list was written: unit suffixes (`3.3V`/`20mA`/`330R`,
now alongside canonical `330Ohm`/`20mAmp`), instance arrays
(`let r[i:16][j:8]` — 2026-09-23, E1, Slice 1), pin-level drive intents
(`inst.pin = true;` — 2026-09-23, E14a gate), the E13 decoupling
convention, the E14a intent-completion machinery, and per-prefix
reference designators. The §3.2 USB-sensor gate fixture
(`examples/electronics/usb_sensor.ebv`) compiles to a `.kicad_sch`.
(Named-net syntax was considered and then **retracted** on 2026-09-22 —
nets are named by derived physics, not author labels.)

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
- **Clauses**: `pin` / `reference` parse via shared helpers in
  `parse_type_body`, `parse_obj_like`, and `parse_cell`. `reference` is
  parse-mandatory when pins exist. The `!> Reference` metadata path is
  DELETED — analysis reads `td.body.reference` and `CellDef.pins/reference`.
  (2026-09-24: the `tolerance`/`rating` clauses joined that deletion — the
  envelope is `spec Tolerance`/`spec Rating` body metadata, see the dated
  entry below.)
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
instance `value` stays the raw label string and is never parsed).
Names resolve against FINAL union-find roots (stale-root safe); two
DIFFERENT names on one node are a hard error refusing emission
(`net_conflicts`, checked like dangling pins); the same name twice is
redundant, not a conflict.

**2026-09-12 (power ratings):** `spec Rating: 0.25W;` / `spec Rating: any;`
(clause form `rating 0.25;` retired 2026-09-24 — see the dated entry
below) on type/obj/cell bodies. After the current fixpoint, `derive_power`
computes P = ΔV²/R for every valued two-pin part with both endpoints
classed: exceeding the declared rating is a violation; proven dissipation
with no rating spec is an undeclared decision; within-rating records a
proof fact. One-sided parts (no proven ΔV) and zero-drop straps force
nothing.

Deferred: per-pin tolerances, LLVM/GPU representation (awaits
simulation), PinDecl in cell bodies' beast serialization, pin roles,
rail inference, series-resistor placement synthesis, board auto-routing
(E14b/fab plan follow-ons).

**2026-09-23 (fab layer, plan `2026-09-23-ebv-fab-layer.md`):** the
physical-layout section. `fab { board 40mm x 20mm; place u1 @ (20mm,
10mm) rot 90; }` declares the outline + pinned positions; the compiler
auto-places the rest deterministically, proves containment (error) and
clearance (warning), and emits a `.kicad_pcb` alongside the schematic
(physics-derived net labels, footprints from `config/footprints.dbvl`).
New surface: `Length` dimension + `mm`/`cm` units.

**2026-09-24 (SPST component modes, plan
`2026-09-24-spst-component-modes.md`):** named finite-state components.
A type declares `mode closed { … }` / `mode open { … }`; mode bodies share
the law elaborator and solver. `sw1.closed` / `!sw1.closed` in a node
precondition filters the states in which that node's postconditions are
proved. Board-wide tolerance/rating/budget checks still cover all modes.
`usb_sensor.ebv` now uses stdlib `Spst` for pressed/released behavior and
declares its pull-up resistor as a law-bearing component.

**2026-09-24 (tolerance/rating → spec keys, plan
`2026-09-24-tolerance-rating-spec-migration.md`):** the `tolerance …;` /
`rating …;` clauses are retired. The envelope is type-level `spec
Tolerance: 3.6V;` / `spec Rating: 0.25W;` (explicit ASCII unit required —
the unit IS the physics; `any` declares unrated, `Volt`/`Watt` alone
declare the dimension). Values live in `TypeDefBody.metadata`; the
`ast::top::Tolerance`/`Rating` enums and the `TypeDefBody`/`CellDef`
fields are deleted, and an instance-literal `spec Tolerance:` is a
parse-time error (envelopes are type-level; per-instance is Phase 4).
Old clauses parse-error at the clause site naming the replacement;
`check_tolerance`/`derive_power` consume `TypeInfo.tolerance`/`rating`
built from the metadata (`any` → INFINITY).

**2026-09-24 (multi-state law solving, plan
`2026-09-24-multi-state-law-solving.md`):** the DC solver now returns
labeled global operating states. Guarded group candidates are combined
deterministically (64-state board budget). `spec Bistable: true` is
authority for a component's guarded laws; an ambiguous group retains its
states only when every guarded-law contributor declares it. Tolerance,
current, power, and budget proofs run per state and name their state.
`unpop` law parts solve present and absent participation states; the
absent state omits their laws and records the omission explicitly. The
first deterministic state remains the representative emitter view.

**2026-09-24 (Slice 6, plan `2026-09-24-ascii-units-and-law-proof-closure.md`):**
law-proof closure. Current/power checks run after the DC solve, so solved
pin quantities participate in every proof. Law-bearing parts get
`P = Σ pin_voltage × pin_current` rating proofs; budgets consume solved
branch-current magnitudes; lower current bounds are checked against the
signed pin current; proofs name `component-law DC` provenance. A law
component marked `unpop` is an explicit dual-state unsupported diagnostic
rather than an absent-state vacuous pass.

**2026-09-24 (ASCII units and physics layer split, plan
`2026-09-24-ascii-units-and-law-proof-closure.md`):** unit ergonomics is
ASCII, not verbosity. Compact `V`, `mA`, `R`, `kR`, and E-series `4k7`
forms are first-class alongside full-word aliases (`Volt`, `mAmp`,
`Ohm`, `kOhm`). `Ω` and other non-ASCII/Greek-like symbols are rejected.
The compiler-native substrate is dimensional algebra — including
`Ohm == Volt / Amp` — quantity normalization, law IR, guards, KCL, and
DC solving. Component-specific constitutive equations live in component
declarations; the compiler has no component catalog.

**2026-09-24 (component laws, Slice 1, plan
`2026-09-24-electronics-component-laws.md`):** the physics parameter
foundation. Unit parsing is centralized for spec values and expression
literals, with both compact and full-word ASCII quantities (`4.7kR`,
`4.7kOhm`, `20mA`, `20mAmp`). `spec Resistance: R;` (or `Ohm;`) declares a
dimensioned component parameter and `spec Resistance: 330R;` states its
value; instance literals carry that value as structured SI + dimension
(`ComponentInstance.specs`), separate from the opaque `value` annotation.
Structured resistance is the sole physics source: the legacy numeric-`value`
heuristic (`parse_ohms` on the label) was deleted 2026-09-24 (plan
`2026-09-24-retire-legacy-value-physics.md`) — a value-only instance derives
no current, no divider, and no dissipation. The approved destination is
type-body `when` laws as constitutive equations plus a piecewise-linear DC
solver; later slices elaborate and solve them.

**2026-09-25 (E14b slice 8, plan `2026-09-25-ebv-e14b-ldo-output-law.md`):**
law-derived rail birth via `spec Output`. The LDO output law is an
ordinary law parameter — dimension-only at the type (`spec Output:
Volt;`), value at the instance (`spec Output: 3.3V`) — with an
unconditional `when true { vout.voltage == Output; }`. Birth detection
is generic: an always-guarded equation pinning a single supply-class
pin voltage to a constant drives its net as a contract fact would; the
births merge into the driven-rail map ahead of the membership ladder
and the obligation forcing, proving `rail born: u1.vout drives the 3.3
rail (component law)`. No parser change (the law-parameter path
carried it); a drive contradicting the law is the existing hard
"no DC operating point" error. Joining a law group demands a unique
operating point — the type declares `en.current == 0Amp` and
`gnd.current == 0Amp` beside the output equation, or the branch
currents stay free and the closed switch state fails to solve. The
usb_sensor fixture's six rail-birth equalities are deleted; only
`j1.vbus == 5.0V` (external reality) and E15's signal wiring remain.

**2026-09-25 (quantities Phase 4, plan `2026-09-25-quantities-phase4-envelopes.md`):**
envelope specs + the anti-vacuity rule. `spec MaxCurrent` — the
absolute-maximum current envelope, unconditional, every state — joins
`Tolerance`/`Rating`, with pin-qualified rows (`spec Tolerance: in:
24V, vdd: 3.6V;`) for asymmetric parts and INSTANCE literals overriding
type envelopes (derating; resolution instance > pin row > uniform).
Min bounds are deliberately NOT spec keys: they live in node
postconditions (`[d1.a.current > 0]`), state-scoped by the guard — a
min-current requirement does not hold in every state. And the vacuous-
proof hole is closed: a stated current bound the solver cannot attempt
is a hard error naming the missing law physics (absent-participation
states exempt). led_blinker migrated: envelope on the instance, minimum
in the node postcondition.

**2026-09-25 (E14b slice 7, plan `2026-09-25-ebv-e14b-rail-membership.md`):**
supply-rail membership via the `net<>`/`stdnet<>` strategy keywords
(order-free modifier family, `sync<g>` shape) on `let`. `stdnet<Name>`
looks up a declared registry row — an ordinary type with `spec
NetVoltage` (+ `spec KicadLabel`); stdlib seeds VBUS, V5V, V3V3, V12V.
The ladder: expectation filter (candidates = rails driven at the
declared volts; expectations constrain, they never drive) → tolerance
refutation (unique survivor infers silently) → propagation toward a
single bound-named candidate rail → D13 enumerated error naming the
keywords. Names bind to rails only through physics-forced pins; a name
refusing to span two rails is an error; return-class and non-supply
pins take no name; the bare form requires exactly one supply pin
(`<in: VBUS, vout: V3V3>` qualifies per pin). `net<ident>` is the
board-local opaque form (never consults the registry, Rule 15). Author
labels flow through `Net.author_label` into the KiCad emitter. The
usb_sensor guard is now §3.2's exact form — rail births plus the
signal wiring E15 owns. Distinct from the label-only `net <name>:`
retired 2026-09-22: this is membership semantics the engine acts on.

**2026-09-25 (E14b slice 6, plan `2026-09-25-ebv-e14b-return-net.md`):**
return-net inference + decoupler auto-bridging. Every pin whose class
declares `spec Return: true` on a populated instance unions into the
board's single return net (D6 class semantics — choice-free, no D13
surface). The E13 decoupling convention became a forcing rule: an
un-bridged supply net consumes the next free two-pin `spec Decoupler`
part (return side → return net, supply side → the supply pin's net);
the bridge test stays net-level, ≠2-pin decouplers are never auto-wired,
and impossible obligations keep the E13 diagnostic. `wire_low` gained the
p1-pre-wired switch completion (one path pin on the obligation net, the
free pin joins the return net), and stdlib `Spst` pins ascribe `: Path`
again. The usb_sensor fixture's return side is solver-inferred: 17 guard
equalities deleted, caps sized to the bridge convention (`c[i:2]`), the
button low bound moved into the node body (postcondition bounds verify —
they do not force). Remaining E14b: supply-rail membership (declared-
intent design fork) and the LDO output law.

**2026-09-23 (E14b slice 5, plan `2026-09-23-ebv-e14b-value-aware-pullup.md`):**
value-aware pull-up matching. A MIN obligation is satisfied by any
pull-up resistance, so distinct-value free parts assign deterministically
instead of erroring (switches keep the ambiguity — multi-pole capacity).
The button node of `usb_sensor.ebv` is now fully pure-intent.

**2026-09-23 (E14b slice 3, plan `2026-09-23-ebv-e14b-bus-assembly.md`):**
open-drain bus assembly. MIN obligations on same-name WiredAnd-class pins
at the same voltage union into one net before pull-up forcing; voltage
obligations realize before drive completions so unassembled open-drain
pins never appear as drive candidates. The i2c node of `usb_sensor.ebv`
is now obligations only.

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
