# Wave 2 — `.sbv`→`.ebv` static pair — execution plan (2026-09-27)

**Implements:** `docs/plans/2026-09-25-sbv-ebv-bridge.md` (design record,
including its 2026-09-27 §"Syntax decision"). **Substrate:** Wave 1
complete (C0–C6, commits `eb367341`…`845c47a5`): provenance, truthful
extension dispatch, per-module preludes, the cross-dialect collision gate,
the `.rbv`→`.bv` declared edge, docs.
**Companion queue:** Wave 2b (runtime pairs + the alias instance binding)
is staged separately — pointer at the tail.

## Ground truth (verified 2026-09-25/27)

- `.sbv` explicit-ext imports resolve and parse as Briev today (Wave 1 C2
  placeholder arm, dated comment at the parse arm). No per-kind semantics.
- Pin classes are valueless marker typedefs (`lib/std/electronics.bv`
  `Power`/`Ground`/`In`/`Out`/`Io`/`IoOd`/…); direction lives in the class.
- Electronics netlist derivation (`derive_netlist`) consumes typedefs with
  `pin` slots + instances; the KiCad emitter consumes that.
- `.dbv` schema+record grammar exists (`check_data_source`,
  `pipeline.rs:749`); `Pinout` records are Layer 3's declared shape.

## C0 — Substrate verification

**DONE 2026-09-27.** The die fixture (`examples/silicon/sensor_die.sbv`)
checks clean: bare file-scope `let`s typed by pin classes typecheck as
ordinary state declarations, and the `.sbv` prelude provides
`std/electronics.bv` without an explicit import.

**Defect found and fixed (the C0 reason this stage exists):** the `.sbv`
prelude (`plugins/parsed/prelude-hw.bv`, dated 2026-07-21) anchored
`Import$("std/hardware.bv")` — a file that has NEVER existed — so every
`.sbv` file with an import failed resolution. No `.sbv` corpus file
existed to catch it (this fixture is the first ever swept). Fix: the
anchor now inserts `std/electronics.bv` — the bridge design record
(Layer 0) builds dies on exactly those pin classes — with the E14a-gate
fallback mirrored for import-less dies (anchored on the first `state`
decl, `.beast` variant name per `beast/serialize.rs:134`, then typedef).
The old "provides Cell, Wire, Register, Bit" comment was a promise the
tree never kept; that vocabulary lands as stdlib data with the CIRCT
backend work, never as a missing-file reference. A no-import die with
both fields and typedefs double-splices `std/electronics.bv` — same
module id, identical dump: the C4 benign pair (noted at the prelude).

Test: `test_sbv_die_boundary_fields_check` (frontend_check on the
fixture — the C5 pattern). Suite **2696/0**; conformance sweep green
with the first `.sbv` in the corpus; Praetor gate on
`import_resolver.rs` clean (test addition only).

The die fixture (no explicit import — prelude provides, same parity as
`.ebv` files):

```briev
let sda: IoOd;
let scl: Out;
let gpio: Io[8];
let vdd: Power;
let gnd: Ground;
```

The die is a `.sbv` file whose file-scope bare `let`s ARE the boundary
pins, typed by pin classes:

```briev
// examples/silicon/sensor_die.sbv
import "std/electronics.bv";
let sda: IoOd;
let scl: Out;
let gpio: Io[8];
let vdd: Power;
let gnd: Ground;
```

Verify: parse (shared AST — expected green), check (pin-class typedefs
splice via the explicit import; bare lets typecheck as state decls — the
open question this stage answers), sweep pickup (`examples/**`, Silicon
kind), and what `brievc check` says about a `.sbv` root (profile/target
gates). Record findings honestly; the fixture stays as the corpus anchor.
Test: `test_sbv_die_boundary_fields_check` (frontend_check on the fixture,
the C5 pattern).

## C1 — Layer 0 projection: imported die → component

> **2026-09-27 revision (after review — user approved).** The C1 semantics
> below were written before the graft-pattern decision; the approved
> pattern (`2026-09-25-sbv-ebv-bridge.md` §"The graft pattern") is: the
> die's file-scope declarations splice LIKE ANY MODULE (fields as shared
> program state, types/defns as declarations) PLUS the synthesized
> component typedef — pins ≡ fields, identity not a bridge. Includes:
> nested `let` parses in obj/cell bodies (internal hierarchy needs it),
> and `rename_item` learns `Statement::Let` (renamed state imports no-op
> today — without the fix, a die field colliding with a board let has no
> C4 escape). The C1 tests below gain: fields present in the splice;
> internal items grafted but never projected.

One rule (design record Layer 0): an imported `.sbv` file projects an
`.ebv` component whose pins are its file-scope fields — pin names = field
names, direction/class from the field's pin class, arrays element-wise
(`gpio[8]` → 8 pins or a pin array per KiCad conventions), named by module
path. Synthesis happens at import splice time (frontend), producing an
ordinary component typedef the board instantiates — zero backend knowledge.

Tests: `test_sbv_import_projects_component` (import `sensor_die.sbv` into
an `.ebv` root; a component named for the die exists with pins
sda/scl/gpio/vdd/gnd, classes preserved); `test_die_internal_fields_stay_hidden`
(nested items do not project — encapsulation is structural).

## C2 — Layer 1 correctness edges on the splice

All existing e14a machinery, wired to the projected component (design
record Layer 1 list): class compatibility (IoOd↔IoOd wired-AND; two
drive-capable pins on one net = contention), direction preservation
(out→in ok; out→out contention), voltage domain (`spec NetVoltage` vs pin
tolerance), arity (element-wise; mismatch named at the array). The gates
are the design record's: per-pair class-law tests. Corpus: die+board
fixture (gpio array + pull-up) checked AND netlist-derived clean.

Tests: `test_die_board_direction_preserved`, `test_die_board_contention_named`,
`test_die_board_arity_mismatch_named`, gate-fixture derive (assert_clean).

## C3 — Layer 2 trait machinery (the one substantial compiler piece)

Trait logical-field requirements typecheck against pin-class ascriptions
(§8.6 structural conformance): `trait I2cTarget: I2c { sda: IoOd; scl: In; }`
conformance is proven by the die's field types. Family binding: once any
net member conforms, all must conform to compatible roles;
`SingleController` counts conformers per net. Suggestion diagnostics may
point at structural conformance; missing-impl is never an error,
violation is. The I2c family itself is stdlib data (`lib/std/traits.bv`
or beside `std/electronics.bv`) — Rules 14/15.

Tests: `test_trait_conformance_by_field_types`, `test_family_binding_counts_controllers`
(two controllers on one net = error naming both), `test_missing_impl_is_not_an_error`.

## C4 — Layer 3 pinout records

`Pinout (QFN32) { sda: 14; scl: 15; gpio: 16..23; vdd: 1, 9; nc: 2, 3; }`
in the EXISTING `.dbv` grammar (extend the record vocabulary only if the
grammar cannot express it — verify first, `check_data_source` path).
Validator (pure mechanism): record fields ⊆ die file-scope fields; range
arity = array arity; positions unique; every field covered or explicit
`nc`. Binding: `--fab <record>`; absent record = auto-sequential + warning
for standalone checks, REFUSED for synthesis/board placement output.

Tests: `test_pinout_record_validates`, `test_pinout_range_arity_matches_array`,
`test_pinout_missing_field_is_error`, `test_pinout_required_for_synthesis_output`.

## C5 — Dual consumers: constraints + KiCad pin numbers

Silicon constraints emission hook per target flow (XDC/PCF/LPF —
target-specific emission lives in the target's config/templates, never a
Rust format table); `.ebv` projection takes KiCad symbol pin numbers from
the same record. Snapshot tests: synthesis-constraint snapshot, KiCad
symbol pin-number snapshot (the mod.rs fixture-test pattern).

## C6 — Docs

SPEC §7 (die/component projection note), §8.6/§8.7 worked I2c example,
interop plan per-pair row (`.sbv`→`.ebv` DECLARED), `backend-contracts.md`
if electronics/circt charters gain obligations.

## Wave 2b pointer (staged after this wave)

Runtime pairs + the alias instance binding (bridge design record §"Syntax
decision", phasing): `.bv`↔`.sbv` (ports→MMIO-backed state under the
alias, unaliased-instance gate), `.bv`↔`.abv` (buffer surfaces),
synthesized-obj disclosure, then Wave 3 derivation (compose adjacent
edges; synthesized bridge nodes for distant pairs).

## Verification per commit

`cargo test --lib` green · the commit's new tests · Praetor on changed
dirs (`--target` = DIRECTORY) · conformance sweep green · no new warnings.
Baseline: suite 2695/0 (post-Wave-1).
