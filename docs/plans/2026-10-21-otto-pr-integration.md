# Otto PR integration + CI gate repair

**Date:** 2026-10-21
**Status:** ACTIVE — approved execution plan.
**Related:** `docs/2026-10-21-effect-shape-full-research-report.md`,
`docs/plans/2026-10-21-effect-shape-followups.md`.

## Findings

| PR | Verdict |
|---|---|
| #30 relative imports | Good; targeted fix + tests |
| #31 generic literal types | Good; root-cause frontend fix + regression tests |
| #32 terminating match arms | Good; real miscompile fix + IR-verified tests |
| #29 volatile raw Ptr | Superseded by main's canonical-pointer provenance fix; merge-dirty; close |

PRs #30/#31/#32 are based on old `main` and share documentation tails, so
they must be updated/rebased and merged sequentially.

Current `main` CI also fails before these PRs because the mixed effect-shape
fixture is FP-order sensitive across escaped/stateless shapes. Fix that first.

## Execution order

1. **Guard determinism fix**
   - Make `tests/tier1/mixed_effect_shape.bv` use exact dyadic Float
     arithmetic so escaped/stateless output is bit-identical.
   - Re-run quick guard; push to trigger CI.
2. **CI environment repair**
   - `mkdir -p /tmp/opencode` before library tests.
   - Install `spirv-tools` for `spirv-val`.
   - Make bad cross assemble/link/run tests skip unless the **full**
     link+QEMU toolchain is present (the current guard checks only the
     assembler).
3. **PR #30** — update branch onto `main`, run import tests + tier-1 + guard,
   merge.
4. **PR #31** — update branch onto `main`, run typechecker/list/generic-literal
   tests + full lib + guard, merge.
5. **PR #32** — update branch onto `main`, run match termination tests +
   determinism quick + guard, merge.
6. **PR #29** — close as superseded by canonical-pointer provenance; link the
   replacement BUGS entry and runtime pin.
7. **Post-merge** — full lib/bin/tier-1, quick+timing guard, update report and
   commit log.

## Acceptance

- Main CI no longer fails on the mixed fixture or missing `/tmp/opencode` /
  `spirv-val`.
- Cross-target tests skip only when the full run toolchain is unavailable,
  never on a silent partial toolchain.
- Otto PRs pass their focused tests after rebase and merge cleanly.
- No blanket-volatile raw pointer behavior enters main.
- Full effect-shape guard and runtime suite stay green.

## Commit log

| Commit | Change |
|---|---|
| `86771f0a` | dyadic mixed fixture (first attempt) |
| `8037758b` | integer mixed output (guard parity) |
| `a2a663b1` | CI test dirs, spirv-tools, full cross-run detection |
| `755de732` | skip optional SPIR-V/PTX probes when absent |
| _pending_ | PR #30 merge |
| _pending_ | PR #31 merge |
| _pending_ | PR #32 merge |
| _pending_ | PR #29 close record |
