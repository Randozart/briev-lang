# Plan 2026-09-24: SPST component modes and node-scoped state selection

**Status:** approved / building
**Branch:** `feat/e14a-intent-synthesis`
**Builds on:** component laws Slices 2–6, ASCII units, Slice 6 proof closure,
and multi-state law solving.

## Decision

Named component modes use `mode <name> { equations… }`. A node selects a mode
with Boolean member sugar:

```briev
type Spst {
    pin a;
    pin b;
    reference "SW";
    tolerance any;
    rating any;

    mode closed {
        a.voltage == b.voltage;
        a.current + b.current == 0;
    }

    mode open {
        a.current == 0Amp;
        b.current == 0Amp;
    }
}

async node button_pressed
    [… && sw1.closed]
    […]
{ }

async node button_released
    [… && !sw1.closed]
    […]
{ }
```

`sw1.closed` is the ergonomic surface. Semantically it is a constraint on the
instance’s named operating mode, not a software member or runtime field.

## Semantic rules

1. A pin-bearing component may declare one or more named modes.
2. Every named mode is a mutually exclusive operating state.
3. Mode bodies contain ordinary constitutive-law facts.
4. Mode equations use the existing elaboration path: pin/spec resolution,
   quantity normalization, dimensional checks, linear IR.
5. A component with modes must not also use ambiguous guarded `when` laws.
   Named modes and guarded branches are separate authority mechanisms. Always
   active laws may coexist with modes.
6. A component with N modes contributes N candidates to the global state
   product. Multiple modes are explicit intent, not solver ambiguity.
7. `spec Bistable` remains authority for ambiguous guarded laws. It is not
   required for explicit named modes.
8. The global state budget remains 64. Named modes participate in the same
   bounded Cartesian product as participation states and guarded modes.
9. Every mode assignment is recorded on the solution and appears in proof /
   violation provenance.

## Node-scoped selection

A node precondition may contain mode facts:

- `sw1.closed` requires instance `sw1` to be in mode `closed`.
- `!sw1.closed` requires every mode other than `closed`.
- Conjunctions combine normally.
- For the first slice, OR may appear in the ordinary expression tree only if
  the resulting predicate is decidable over the finite mode assignments;
  unsupported mode-expression shapes produce a hard diagnostic rather than a
  silent all-states pass.
- A mode fact naming an unknown instance or undeclared mode is a hard error.
- If a node’s mode predicate matches zero global states, that node is an
  error: its precondition is physically unsatisfiable.

Proof applicability:

- Tolerance checks run in every state.
- Law power and rating checks run in every state.
- Budget checks run in every state.
- Node current bounds run only in states satisfying the node precondition.
- Diagnostics and proofs carry deterministic labels such as
  `[sw1=closed]`.

## Solver integration

`DcSolution` gains instance→mode assignments. The solver combines:

1. participation states (`present` / `absent`);
2. explicit component modes;
3. ambiguous guarded-law branch modes.

The first deterministic matching state remains the representative emitter
view. Every matching state is independently proved.

## Stdlib component

Add a generic `Spst`:

- `closed`: pins are equipotential and branch current is conserved.
- `open`: both pin currents are zero.
- No compiler matching on `Spst`; it is ordinary stdlib vocabulary.

## Fixture migration

`usb_sensor.ebv` migrates its local `Switch` to stdlib `Spst`:

- `button_pressed` requires `sw1.closed` and proves the low path.
- `button_released` requires `!sw1.closed` and proves the pull-up high path.
- The contradictory high/low facts currently compressed into one node are
  split across their actual operating states.

## Tests

Checked-in `.ebv` fixtures plus Rust analysis tests cover:

- mode parsing;
- duplicate/empty/unknown mode rejection;
- mode equation elaboration and dimension checks;
- one solved state per named mode;
- deterministic state labels;
- `sw1.closed` filtering;
- `!sw1.closed` filtering;
- zero-state node diagnostics;
- board-wide rating/budget checks across all modes;
- existing behavior on mode-free boards.

## Documentation

- SPEC: named modes, mode exclusivity, node sugar, proof applicability.
- Architecture: mode IR, state enumeration, node filtering.
- Ledger amendment: generic finite-state components landed.
- This plan is authoritative for the implementation; timestamped predecessor
  plans are not retroactively edited.

## Gates

Each implementation commit:

- clean build;
- full `cargo test --lib`;
- electronics fixtures emit;
- deterministic diagnostics and solve order;
- no new Praetor diagnostics in touched files;
- no contract weakened to make a state pass.
