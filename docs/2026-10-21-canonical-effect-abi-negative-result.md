# Canonical effect-context ABI — negative result

**Date:** 2026-10-21
**Status:** ABANDONED — branch retained, not merged.
**Branch:** `feat/canonical-effect-abi`
**Tag:** `abandoned/canonical-effect-abi-2026-10-21`
**Plan on branch:** `docs/plans/2026-10-21-canonical-effect-abi.md`
**Production policy:** interim carry/escaped selector on `main` (`98f7fc42` /
  `54a24cc4`).

## What this branch tried

The long-term plan was to replace the measured carry/escaped selector with one
canonical loop/effect model:

- loop-local fields stay in SSA;
- effect state lives in an explicit `%EffectCtx` stack aggregate;
- print/effect defns take `ptr noalias nocapture %effect`;
- no program-level plain demotion, no `%State` escape, no carry/escaped toggle;
- one stdout buffer, guard kept inside the hot loop for FP order.

The intermediate steps landed on the branch:

- `c88db8e0` — plan.
- `c1952841` — `EffectContextPlan` (plain-field ownership + per-defn reach).
- `94eb9648` — `%EffectCtx` allocation/ABI behind `BRIEV_DIAG_EFFECT_CTX=1`.
- `6999dfc6` — S2/S3 gate result (failed).
- `c12edfce` — context + effect-carry combination (failed).
- `777dec04` — abandonment note.

## Measured result

The explicit context removes the `%State` escape and is correctness-clean
(full suite MATCH), but it loses the carry-winning shape:

| Benchmark | `%EffectCtx` | `%EffectCtx`+carry | current selector |
|---|---:|---:|---:|
| float_math_nonzero | 1.25x | 1.25x | **0.97x** |
| kalman_filter_runtime | 1.03x | 1.17x | **0.86x** |
| sweep_sparse | 1.32x | 1.35x | **0.98x** |
| float_math | 0.91–0.99x | — | 0.89–0.96x |
| queue_drain | 0.51x | — | 0.50x |

## Why it is abandoned

- The carry win is not merely “effect values are loop-carried”. Moving the same
  values into a context alloca changes the LTO loop/alias shape enough that
  the carry phis no longer reproduce the register-resident accumulator.
- `%EffectCtx` alone restores the ordinary stateless SROA/vector shape; in the
  carry-winning loops that is slower.
- Combining the context with carry still fails; there is no measured path from
  this ABI to the carry result.
- Pursuing it further means threading the context through txn/async/task/export
  ABIs and solving per-loop mixed selection, with no evidence it can beat the
  selector.
- The current selector is already correct, guarded, target-configurable, and
  benchmark-validated.

## Decision

Keep the interim **carry / escaped / stateless** selector. Do not merge the
canonical branch. Keep the branch and tag as the negative-result archive.

Reopen only with new evidence:

- a confirmed selector regression on a supported target/workload;
- an LLVM/target change that breaks the carry/escaped shape;
- a real program needing carry and escaped loops in the same program;
- a correctness/portability requirement for an explicit effect ABI.

If reopened, use a timeboxed spike with kill gates (no open-ended rewrite).

## Recommended next work

1. **P1-4/P1-6 pointer provenance** — retire the narrow Ptr-parameter volatile
   fence by completing interprocedural canonical-pointer coverage and mixed
   access-path tests. This is the remaining correctness work created by the
   LLVM-18 CI fix.
2. **P2-1 small-sum unboxing** — highest-leverage performance item: removes
   per-transition/per-pop heap boxes in FSM, SPSC, and `Result` APIs.
3. Then **P2-2 inlining/scalar replacement**, followed by **P2-4 loop and
   vectorization gaps** using the updated effect-shape baseline.

## Branch recovery

```bash
git switch feat/canonical-effect-abi
# or:
git switch -c canonical-effect-abi-return abandoned/canonical-effect-abi-2026-10-21
```
