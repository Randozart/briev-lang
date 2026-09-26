# E8 — driven-node drive capability and fan-in (op-amp output columns)

Date: 2026-09-26
Gap: `2026-09-21-hardware-dialect-gaps.md` E8 (op-amp drive / fan-in model)
Status: LANDED — slices 1+2 and the gate fixture landed as one analysis
commit (`488494ea`); slice 3's docs are this commit.

## Problem

An active part that drives a loaded node (op-amp output column, LDO rail,
GPIO pin) has a datasheet capability: a maximum sourced current and a
maximum number of loads it can drive (fan-in). The analysis proves per-net
physics — KCL branch sums (`contribution_pass`), law operating points,
`budget` roll-ups (E7), per-pin `MaxCurrent` envelopes (Phase 4) — but
never attributes a net's draw to the part that drives it, and never bounds
the load count. A 17-input summing node wired to an op-amp rated for 16
compiles today.

The gap's original evidence is partly stale: `parse_rating_clause` is
retired (`spec Rating` envelopes landed), and the black-box "rating any"
concern shrank when law-bearing parts stopped being opaque. What remains
is exactly the driven-node check plus the fan-in bound.

## Prior art this builds on (nothing new invented)

- **Roll-up core (E7)**: `law_net_draw` (component-law DC) falling back to
  `net_draw` (B4 fixpoint) — the net's derived draw with provenance.
  No derived draw → vacuous skip (nothing provable flows), same rule
  budgets use.
- **Envelope ladder (Phase 4)**: instance override → pin-qualified type
  key (`drive_current:out`) → type key (`drive_current`), read from
  `spec_defaults` — the exact ladder `max_current_for` walks.
- **Driver identification**: a net's driver is its lone `can_drive` pin.
  Zero drive-capable pins → nothing sources, no check. Two or more →
  ERC contention already refuses (2026-09-22). Rails birthed by a law
  (E14b-8) have exactly one drive-class pin, so LDO outputs check too.
- **Diagnostic family**: datasheet physics belongs in
  `check.violations` (the B4 "electrical contracts are violated"
  refusal), not in a new error family — `budget` is an author statement
  (own family, E7); `DriveCurrent`/`FanIn` are component physics.

## Design decisions

- **D1 — `spec DriveCurrent: <current>`** is the part's maximum sourced
  current, pin-qualified like every envelope (`spec DriveCurrent: out:
  20mAmp;` or type-level `spec DriveCurrent: 20mAmp;`), instance
  overridable. Checked per operating state: a net whose lone `can_drive`
  pin's type declares it must have its derived draw within the limit.
- **D2 — `spec FanIn: <count>`** is the maximum number of load branches
  the driver may see: `net.pins − 1` (the driver's own pin), counted per
  driven net. Dimensionless count, same ladder (pin-qualified, type
  level, instance override). Opt-in: absent → no bound (rail nets with
  big membership stay untouched unless the part declares a fan-in).
- **D3 — unpop drivers are exempt** (an unpopulated part carries no
  copper — the same exemption `check_current_bounds` and `check_budgets`
  use).
- **D4 — proofs, not just refusals**: a passing check emits a proof line
  (`draw <= DriveCurrent (sourced)` / `fan-in n <= N`) so the gate
  fixture shows the summing node PROVEN, not merely unfailed.
- **D5 — no name vocabulary**: the check reads class properties
  (`can_drive`) and spec envelopes generically. "Op-amp" appears nowhere
  in the compiler (Rule 15).

## Slices

1. **DriveCurrent**: envelope key + `check_drive_capability` beside
   `check_max_current` + tests (overdriven node refuses with a
   proof-carrying diagnostic; within-capability passes with a proof
   line; no-derived-draw nets skip).
2. **FanIn**: count bound in the same pass + tests (16 loads pass, 17
   refuse).
3. **Gate fixture + docs**: a summing-node fixture proving 16 inputs
   within rating (and its overdriven twin refusing), gap entry E8 →
   CLOSED, design record.

## Gate (from the gap entry)

A 16-input summing node proves within rating; an overdriven node is a
compile error. Both as analysis-level contract tests, matching the E7
gate style.
