# Plan 2026-09-24: ASCII units, main sync, and law-proof closure

**Status:** approved / building
**Branch:** `feat/e14a-intent-synthesis`
**Supersedes the temporary “must spell `Ohm`” rule.** The real ergonomic rule
is ASCII-only, not full-word-only.

## Decision

Component physics avoids non-ASCII/Greek-like symbols. Compact ASCII unit
forms are first-class. Full-word forms are aliases, not a requirement.

Allowed:

- voltage: `V`, `mV`, `Volt`, `mVolt`
- current: `A`, `mA`, `Amp`, `mAmp`
- resistance: `R`, `kR`, `MR`, `Ohm`, `kOhm`
- E-series: `4k7`
- capacitance/inductance/power/etc. keep their existing compact ASCII bases
  (`F`, `pF`, `H`, `Hz`, `W`, `K`) and full-word aliases.

Banned in component physics:

- `Ω`
- other non-ASCII / Greek-like unit symbols
- `µ` (use ASCII `u`)

A bare number remains ambiguous for a component physics parameter: `330`
does not state a unit. `330R`, `4k7`, and `330Ohm` do.

## Layer split

- **Compiler-native substrate:** dimensional algebra, SI-prefix parsing,
  quantity normalization, law IR, guards, KCL, deterministic DC solve.
- **Fundamental identity:** `Ohm == Volt / Amp` dimensionally.
- **Component behavior:** constitutive equations live in component
  declarations (`Resistor`, `Diode`, user components), never compiler
  catalog match arms.
- **Instance values:** datasheet parameters via `spec`; `value` remains
  opaque BOM annotation.

## Phase 0 — sync local main

Merge local `main` (`7ed975a3`) into this branch. Local main carries the
metaprogramming/bad-dialect AST changes; `origin/main` is already contained.

Resolution policy:

- Keep main’s AST shape: remove `Statement::InlineTxn`, keep
  `Definition.variadic_param` and `BadFn.bootstrap`.
- Keep branch electronics behavior: fab, quantities, component laws, DC
  solver, fixture migration.
- Preserve main’s parser changes (`bootstrap bad`, raw bad, chain-capture
  receiver rule) and branch parser changes (`thru`, component specs, laws).

After conflict resolution, sweep for stale `InlineTxn`, missing
`variadic_param`, and missing `bootstrap` initializers. Build, run full lib
tests, emit electronics fixtures, run Praetor, commit/push the merge.

## Phase 1 — ASCII unit correction

Current branch briefly required spelled `Ohm` for resistance. Correct it:

1. `R`, `kR`, `MR`, `Ohm`, `kOhm` resolve to `QuantityDim::Ohm`.
2. `Ω` is removed from quantity parsing and diagnostics.
3. Component resistance accepts:
   - declaration: `spec Resistance: R;` or `spec Resistance: Ohm;`
   - value/default: `330R`, `4.7kR`, `4k7`, `330Ohm`, `4.7kOhm`
4. Instance specs accept the same forms.
5. Bare resistance (`330`) remains an error: no unit, no physics.
6. Diagnostics recommend `R` or `Ohm`; never `Ω`.

Tests cover accepted compact/full-word forms and rejection of `Ω`. Existing
fixtures remain green.

## Phase 2 — doctrine docs

Update SPEC, electronics architecture, component-laws plan, and ledger:

- fundamental dimensional algebra is compiler-native;
- component-specific equations are component vocabulary;
- compiler knows no component catalog names;
- ASCII unit forms are the ergonomic physics surface;
- `Ω` and non-ASCII unit symbols are rejected.

This is documentation plus the parser correction, not a behavior/catalog
expansion.

## Phase 3 — law-proof closure (Slice 6)

The DC solver currently feeds current bounds and voltage checks, but older
proof paths can still miss law-bearing parts. Close the gap:

1. **Law-derived power**
   - For each law-bearing instance, compute absorbed power from solved
     operating point.
   - Two-terminal resistor: `P = |Va − Vb| × |I|`.
   - General component: `P = Σ pin_voltage × pin_current`.
   - A proven-dissipating part without a rating is an undeclared decision.
   - `rating any` remains explicit permission.
2. **Law-derived budgets**
   - Budget checks must use solved law branch currents, not only the legacy
     series heuristic.
   - Deterministic summation and law-provenance diagnostics.
3. **Lower current bounds**
   - `pin.current >= bound` proves against signed law current.
   - Preserve upper-bound behavior.
   - Add explicit proof records.
4. **Unpopulated law-bearing parts**
   - Solve or explicitly reject present/absent law states.
   - No silent vacuous pass.

## Gates

Every commit:

- clean `cargo build`;
- `cargo test --lib` green;
- electronics fixtures emit;
- no new Praetor diagnostics in touched files;
- deterministic diagnostics and solve order;
- no contract weakened to make a proof pass.
