# Plan 2026-09-24: Electronics component laws — expressive constitutive physics

**Status:** approved / building
**Branch:** `feat/e14a-intent-synthesis`
**Supersedes, for component behavior:** the implicit “two-pin part with a numeric
`value` is a resistor” heuristic. It does **not** replace topology inference,
mechanisms, or the participation model.

## Why

The current electronics analyzer has three separate facts doing one job:

1. Explicit pin equalities infer topology.
2. A few special conventions (`Decouple`, `PullUp`, `Control`/`Path`) synthesize
   or check structure.
3. Any two-pin instance whose opaque `value` happens to parse as ohms becomes a
   series resistor.

That third rule is accidental behavior, not language semantics. It also cannot
express the parts we actually need: ideal links, asymmetric devices, regulated
supplies, or piecewise conduction. The language must declare behavior; the
compiler must prove consequences. It must not enumerate component catalog
names.

**Doctrine:** Golden Rules 14/15/23/24. The compiler learns equations, physical
dimensions, guards, KCL, and solution-status diagnostics. It never learns
`Resistor`, `Diode`, `Led`, or `Wire`. Those types and their behavior live in
stdlib. A user can define a new part without a compiler change.

## Decisions (locked)

1. **Constitutive equations live in type-body `when` laws.** No new `law`
   keyword. A type-body law already means “guard ⟹ facts”; electronics now
   accepts electrical equation facts under the same rule. `when true` is the
   always-active form.
2. **`spec` is the parameter channel.** A component declares named, dimensioned
   physics parameters and instances supply values. `value` remains a pure BOM
   annotation forever.
3. **Canonical resistance spelling is `Ohm`, with SI prefixes.** New physics
   uses `330Ohm`, `4.7kOhm`, `1mOhm`; it does **not** use `Ω` or `R`.
   Existing quoted `value: "330R"` remains a BOM label and is never read as
   physics after migration.
4. **First solver is piecewise-linear DC.** Linear equations plus guarded
   branches. Nonlinear equations are rejected with a clear diagnostic until the
   solver earns them.
5. **No implicit operating point is chosen.** No solution, multiple solutions,
   missing parameters, uncovered contradictions, and dimension mismatches are
   hard diagnostics. A bound is proved only against a solved mode.

## Surface

```briev
type Resistor {
    pin a; pin b;
    spec Resistance: Ohm;

    when true {
        a.voltage - b.voltage == Resistance * a.current;
        a.current + b.current == 0;
    }
}

type Wire {
    pin a; pin b;

    when true {
        a.voltage == b.voltage;
        a.current + b.current == 0;
    }
}

type Diode {
    pin a; pin k;
    spec ForwardVoltage: Volt;
    spec DynamicResistance: Ohm;

    when a.voltage - k.voltage >= ForwardVoltage {
        a.current ==
            (a.voltage - k.voltage - ForwardVoltage) / DynamicResistance;
    }
    when a.voltage - k.voltage < ForwardVoltage {
        a.current == 0;
    }
    when true {
        a.current + k.current == 0;
    }
}

let r1: Resistor = Resistor {
    value: "330R";              // opaque BOM annotation
    spec Resistance: 330Ohm;    // physics used by the solver
};
```

Directionality is not a category. Symmetric equations are bidirectional;
guarded equations are directional. The compiler sees equations and guards only.

## Physics semantics

- `pin.voltage` is net potential relative to the board's common reference.
- `pin.current` is branch current with **positive-into-pin** sign.
- Every net obeys KCL: the sum of connected branch currents is zero.
- Contract voltage drives are ideal boundary conditions.
- A law fact is `guard ⟹ equation`.
- Unpopulated parts contribute no equations in the absent state; the compiler
  verifies both participation states, as today.
- `open` remains the only author disconnection primitive.
- Mechanisms (`Control`/`Path`) remain conditional-topology machinery. They are
  separate from constitutive physics.

## Solver scope

Slice target is deterministic piecewise-linear DC:

1. Instantiate each type's laws per component.
2. Resolve bare pin names and `spec` parameter names.
3. Dimension-check every term. Derived dimensional algebra includes
   `Ohm == Volt / Amp`; mismatched dimensions are hard errors.
4. Convert supported equations to linear form over net voltages and pin
   currents.
5. Add KCL for every net.
6. Enumerate guarded branch modes within a bounded mode budget.
7. Solve each active-mode linear system.
8. Reject a candidate that violates any inactive or active guard.
9. Report zero solutions, multiple solutions, or missing parameters rather
   than silently choosing.
10. A generic `spec Bistable: true;` acknowledges a intentionally multi-state
    device; all states must still satisfy the contracts.

Multiplication of two variables, division by a variable, transcendental
functions, and time dependence are outside the first solver. They produce an
unsupported-law diagnostic, never an approximation presented as proof.

## Implementation slices

### Slice 1 — quantity and parameter foundation

- Centralize physical-unit parsing for spec values and expressions.
- Add canonical full-word units, starting with `Ohm`; reject `R`/`Ω` in new
  physics values.
- Parse `spec Resistance: Ohm;` as a dimensioned parameter declaration and
  `spec Resistance: 330Ohm;` as a value/default.
- Accept `spec <Name>: <quantity>;` in component instance literals and carry
  the value as structured SI + dimension, not as a display string.
- Keep `value` opaque.
- Tests: canonical units, dimension conflict, missing value, instance supply.

### Slice 2 — law elaboration

- Extract pin-bearing type-body `when` laws into an electronics law IR.
- Resolve instance pins and spec parameters.
- Accept linear equalities over voltages/currents and constant quantities.
- Reject nonlinear forms, unknown identifiers, duplicate parameters, and
  dimension conflicts with what/why/fix messages.
- No solver yet; laws are validated and reported.

### Slice 3 — unguarded DC solve

- Build net-voltage and law-bearing pin-current variables.
- Add law equations and KCL.
- Solve systems with only always-active laws.
- Feed solved quantities into current bounds, budgets, and verification proofs.
- Test resistor, series pair, divider, parallel pair, ideal wire, and current
  source.
- The old `collect_series_parts` path remains only for programs that have not
  migrated; law-bearing parts must not be double-counted.

### Slice 4 — guarded piecewise solve

- Enumerate branch modes deterministically.
- Check every solution against the full guard vector.
- Add no-solution, multiple-solution, and mode-budget diagnostics.
- Add generic `spec Bistable: true;` for acknowledged multi-state devices.
- Test forward/reverse diode behavior and an uncovered contradiction.

### Slice 5 — stdlib and fixture migration

- Move `Resistor` and `Wire` laws into `lib/std/electronics.bv`; add a stdlib
  `Diode`/LED model.
- Migrate `led_blinker.ebv` and `usb_sensor.ebv` to `spec` physics.
- `r_led: "TBD"` becomes a hard missing-parameter diagnostic once its bound
  depends on that parameter.
- Remove the numeric-`value` resistance heuristic when no fixture needs it.

### Slice 6 — documentation closure

- SPEC: component laws, spec parameters, unit canonicalization, sign
  convention, KCL, solver limits, and operating-point diagnostics.
- Architecture: law IR, elaboration, solve order, integration with the netlist
  derivation, and proof provenance.
- Ledger: close the “components as recognized shapes” direction and reference
  this plan.
- Update every touched example and fixture comment.

## Gates

Every slice:

- `cargo test --lib` green (the known environmental probe flake excepted only
  after a rerun).
- No new Praetor diagnostics in changed files.
- Fixture behavior checked when parser/analysis surfaces change.
- Deterministic diagnostics and deterministic solve order.
- No commit weakens an existing proof to make the new path pass.

## Non-goals

- No transient, AC, thermal, or full SPICE simulation.
- No compiler catalog of component names.
- No net-naming syntax.
- No automatic synthesis of arbitrary parts from laws in this plan.
- No nonlinear solve until the piecewise-linear proof path is honest and
  complete.
