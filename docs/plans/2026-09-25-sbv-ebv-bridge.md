# .sbv ↔ .ebv bridge — design record (2026-09-25)

**Refines:** `docs/plans/2026-09-21-cross-dialect-interop.md` Wave 2
headline (`.sbv`→`.ebv` projection). **Supersedes** the initial
"ports→pins projection + `spec Bus`" sketch from the same day's design
session — the trait/edge/inflection below replaces the annotation
vocabulary that sketch invented.
**Depends on:** `2026-09-25-interop-wave1.md` (typed imports, provenance,
per-module preludes — the substrate every layer here rides on).
**Doctrine anchors:** Rules 14/15 (knowledge in stdlib/config/data, never
compiler match arms), Rule 21 (one meaning per keyword/delimiter),
SPEC §8.6 (structural trait conformance is THE semantics; explicit
assertion = intent + diagnostics), SPEC §8.7 (semantic categories;
crossing = at most one explicitly declared edge).

## Syntax decision (2026-09-27): the import alias is the instance binding

User decision, amending the 2026-09-21 plan's `import board = "fpga.sbv"`
sketch (that section carries a dated note pointing here). The shipped `:`
alias grammar carries the entire load — no `=`, no qualified-access
operator, no dialect keywords, no namespace blocks:

> **Alias when the module has an interface instance (state-shaped). Bare
> import when it has only declarations.**

`:` keeps its one meaning — "local name : source name" — extended to name
the module's PROJECTED INTERFACE INSTANCE for instance-shaped dialects;
access through it is ordinary member access (`.`) on a compiler-synthesized
ordinary object. No new name-resolution machinery: the synthesis emits
`obj` items, and everything after the alias is member access Briev
already has. The synthesized object is DISCLOSED
(`synthesized: port→state region for 'board' (fpga.sbv)`), per the
2026-09-21 plan's artificial-driver constraints.

Consumer grammar (every token shipped today; only `board.*`'s MEANING is
new, and only for instance-shaped modules):

```briev
import board: "fpga.sbv";            // alias = the projected instance

let booted: Bool = false;

node boot [!booted][booted] {        // node form: SPEC §9.4
    board.core.leds = 0x5A;          // ordinary member write → MMIO volatile store
    booted = true;
    term;
};
```

What the compiler synthesizes from the declared port table (ordinary `obj`
grammar — this is the expressiveness-closure proof: the compiler's optimum
is expressible as ordinary objects):

```briev
// synthesized (disclosed) — ordinary obj grammar
obj BoardFpga {
    core: BoardCore;
    ram:  BoardRam;
};
obj BoardCore {
    leds: UInt8;                     // slot layout from the declared table; leds → @addr
};
obj BoardRam {
    clk:  Bool;
};
```

Scaling property (why not flat selective imports): two modules exporting
the same port name coexist under aliases with no C4 collision
(`a.clk` ≠ `b.clk`), while flat imports force a rename on every side:

```briev
import a: "core.sbv";
import b: "ram.sbv";                 // both export `clk` — fine: a.clk ≠ b.clk
```

Instance-shaped module with a BARE import — an honest gate (the 2026-09-21
plan's "imported interface points surface as ordinary host things" made
explicit):

```briev
import "fpga.sbv";
// error: 'fpga.sbv' projects an interface instance (2 ports). Bind it with an
// alias: `import board: "fpga.sbv";` — bare import grafts declarations only,
// and a port module has none to graft.
```

Never (the `Mmio#` mistake class — two forms for one fact):

```briev
import board = "fpga.sbv";           // ✗ `=` is assignment load — second grammar
board.leds@0x400000 = x;             // ✗ no per-use address syntax — the table owns addresses
import silicon board from "fpga.sbv";// ✗ no dialect keywords
module fpga { ... }                  // ✗ no namespace blocks — the alias IS the namespace
```

**Phasing.** The static pair (`.sbv`→`.ebv`, this wave) needs NO alias —
a die grafts as a component, declarations only, C4-gated like any graft.
The instance binding lands with the RUNTIME pairs (Wave 2b: `.bv`↔`.sbv`
ports→state, `.bv`↔`.abv` buffers): alias + instance-shaped module →
synthesize projected obj items from the declared table, bind the alias as
the instance name. Until then the alias records provenance only (SPEC
§7.2 note, same date). Tests at 2b: `test_alias_binds_projected_instance`,
`test_qualified_ports_coexist`, `test_unaliased_instance_module_is_error`.

## The domain split (why this edge is different)

- Computational family — `.bv` `.dbv` `.abv` `.rbv`: one device, one
  runtime. Interop = namespace + provenance + capability intersection
  (Wave 1 machinery; Wave 2 runtime pairs).
- Physical family — `.sbv` `.ebv`: a die pin meeting copper is not a
  namespace problem; it is electrical law + (optionally) protocol
  semantics. The bridge below handles it without inventing a protocol
  vocabulary.

## Layer 0 — File-as-die

A `.sbv` file IS the die. Every dialect file is itself the design unit,
imports splice design units:

| File | IS a | file-scope surface |
|------|------|--------------------|
| `.bv` | module | items |
| `.ebv` | board | component instances, nets, connectors |
| `.sbv` | die | **pins (boundary state fields)** |
| `.rbv` | page | view + bound Briev |
| `.dbv` | dataset | records |

- Boundary fields are **bare `let`s** typed by pin classes — no `port`
  keyword. Pin-ness lives in the type (classes are valueless markers,
  `lib/std/electronics.bv:24-34`); direction lives in the class
  (`Io`/`IoOd`/`In`/`Out`), never in a declaration-time keyword (a wire
  is bidirectional; the current driver decides).
- Encapsulation is structural: file-scope = package boundary; anything
  nested is internal hierarchy, invisible to the board.
- Projection: imported `.sbv` file → `.ebv` component whose pins are its
  file-scope fields, named by module path. One rule; no selection logic;
  pin names = field names.
- Internal hierarchy (inner types instantiated within the die) is DEFERRED
  — flat v1; the encapsulation argument does not depend on it.

## Layer 1 — Unification ("it just works")

After the import splice there is nothing to bridge at runtime: the die's
fields are shared reactive variables in one program — the reactor's wake
sets wake on their change (the `node @ address` machine-entry model is
literally pins waking computation), writes are drives, and the electrical
solver (`.ebv` laws) and computational nodes read/write the same fields.

### The graft pattern (2026-09-27, after review — resolves the Layer 0/1 reading)

> **A die grafts its file-scope declarations like any module — bare `let`s
> as shared program state, types/defns as declarations — plus the
> synthesized component typedef. The component's pins are the fields:
> identity, not a bridge.**

The two headline sentences ("grafts as a component, declarations only" /
"the die's fields are shared reactive variables in one program") cohere
only under this one reading, and every constraint lands on it:

- *Layer 0* ("projects an `.ebv` component whose pins are its
  file-scope fields") — the typedef, derived mechanically from the fields.
- *Layer 1* ("fields are shared reactive variables … nothing to bridge")
  — literally: the fields splice; `sensor.sda` ≡ `sda` by construction
  (the pin table is generated FROM the fields, so instance pin access and
  the field are the same state — C2 wires that identity into the netlist).
- *"file-scope = package boundary"* — all file-scope declarations reach
  the board; an interpretation that hides the die's file-scope defns/types
  contradicts the boundary rule.
- *"C4-gated like any graft"* — now meaningful: multiple items pass the
  cross-module gate, and the rename escape must work for state items too.
- Free consequence: a plain-`.bv` board importing a die receives
  `std/electronics.bv` transitively (the die's own prelude splice), so the
  projected pin classes resolve there — no board-side requirement.

A die is one physical thing, so it gets one set of state, module-level —
no per-instance pin storage. Nested `let` (internal hierarchy) requires
the parser to accept member `let` in declaration bodies (obj, cell — the
loops that already accept defn/txn members); nested items splice as
declarations but never project as pins.

Correctness at every edge — ALWAYS on, vocabulary-free, all existing e14a
machinery:

- class compatibility (`IoOd`↔`IoOd` wired-AND; two drive-capable pins on
  one net = contention);
- direction preservation (out→in ok; out→out contention);
- voltage domain (`spec NetVoltage` vs pin tolerance);
- arity (arrays unify element-wise; `gpio[8]` ↔ `j1[8]`; mismatch named at
  the array);
- class-level electrical law (IoOd floating/pull-up facts — the class
  machinery owns these; the usb_sensor fixture deleted its explicit
  pull-up wiring for exactly this reason).

## Layer 2 — Optional trait refinement (for what classes cannot see)

Two wrongs survive class checking: role swaps that are class-compatible
(board `sda` net wired to die `scl`, both class-compatible), and buses
whose roles are electrically legal to violate (multi-controller I2C on
`IoOd` scl is wire-legal; "exactly one controller" is semantic).

For these — and ONLY these — stdlib trait families:

```bv
trait I2c { spec SingleController: true; };        // family rule, read generically
trait I2cTarget: I2c     { sda: IoOd; scl: In; };
trait I2cController: I2c { sda: IoOd; scl: Out; };
```

- Conformance is **structural inference** (§8.6 — no annotation). A die
  whose fields prove `I2cTarget`'s requirements conforms; explicit
  assertion only documents intent and requests direct diagnostics.
- Family binding: once ANY member of a net conforms to a family, all
  members must conform to compatible roles; `SingleController` counts
  conformers per net. Opt-in per design, mandatory per net once claimed.
- Diagnostics may SUGGEST ("these members structurally conform to I2c —
  assert to enable role checks"). Missing-impl is never an error;
  conformance VIOLATION is. Silent structural binding beyond the existing
  conformance rule is refused — accidental cross-file agreement is the
  same trap class as positional shadowing.
- New compiler code here (the only substantial piece in the whole
  bridge): trait "logical field" requirements typechecking against pin
  class ascriptions.

## Layer 3 — Packaging: base-grammar pinout records

The board does not solder to `sda`; it solders to pin 14. The synthesiser
is the PRIMARY consumer (Verilog+XDC / VHDL+LPF are the real-world
shape: constraints travel with the netlist, never inside the HDL). The
record uses the EXISTING `.dbv` schema+record grammar — field names ARE
the die's field names, no keywords invented:

```bv
// fab/sensor_die_qfn32.dbv
Pinout (QFN32) {
    sda: 14;
    scl: 15;
    gpio: 16..23;
    vdd: 1, 9;      // multi-bond, explicit
    nc: 2, 3;       // unbonded, explicit
};
```

- Validator (pure mechanism): record fields ⊆ die file-scope fields;
  range arity = array arity; positions unique; every die field covered or
  explicit `nc`. Same validation path as other `.dbv`
  (`check_data_source`).
- Dual consumers: silicon backend emits netlist + pin constraints in the
  target flow's format (XDC/PCF/LPF — target-specific emission, the
  correct home for format knowledge); `.ebv` projection takes KiCad
  symbol pin numbers from the same record.
- Binding: `--fab <record>` on the build names it. Absent record:
  auto-sequential numbers + warning for standalone checking; REFUSED for
  real synthesis/board placement output.
- "Fab" is a directory name, never grammar. Electrical content (rails,
  decoupling, tolerance) stays in `.ebv` — the record is pure geometry:
  names ↔ positions.
- SiP / multi-die packages: later slice; the record shape does not
  preclude them.

## Refusals, named loudly

- Waveform/timing physics (setup/hold, clock domains): out of scope; the
  `.ebv` law engine is the eventual seed, but it does not sneak in
  through this edge. Protocol traits carry static obligations only.
- Implicit cross-file name matching without an import — the import IS
  the one declared §8.7 edge; coincidental name agreement between two
  unspliced files is never semantics.
- Protocol vocabulary in the compiler: the compiler learns only
  mechanisms (class laws, structural conformance, record validation);
  I2C/SPI/whatever remain stdlib trait data forever (Rules 14/15).

## Implementation inventory (what is actually new)

| Piece | Size | Wave |
|-------|------|------|
| `.sbv` candidate classification (Wave 1 C2) | tiny | 1 |
| Trait-field vs pin-class typecheck | small | 2 |
| Pinout record validator | small | 2 |
| Silicon constraints emission hook (per target format) | medium, per-flow | 2 |
| KiCad symbol pin-number hook | small | 2 |
| Trait families (I2c first) | stdlib data | 2 |

## Gates (extending the interop plan's Wave 2 gates)

- Direction preservation per pair (class law tests).
- Capability intersection per imported module (`capabilities.rs`).
- Obligation transport: trait conformance crossing the splice,
  conjunction-checked (family binding test: two controllers on one net =
  error naming both).
- Corpus example per behavior: die+board fixture (gpio array + I2c pair +
  pull-up), pinout record, synthesis-constraint snapshot test, KiCad
  symbol snapshot test.

## Documentation when landing

Interop plan per-pair status; SPEC §7 (imports) + §8.6/§8.7 examples;
`glue-ffi.md` only if the runtime pairs touch it; `backend-contracts.md`
if the electronics/circt charters gain obligations.
