# Tail-recursion lowering and TCO-safe sret

**Date:** 2026-10-21
**Status:** ACTIVE — analysis slice.
**Branch:** `feat/tail-recursion-lowering`
**Base:** safe B2 `d65fa78c` on `feat/nested-enum-layout`
**Rejected B3:** `experiment/nested-enum-b3` (`9a922d04`) — nested sret slots
stack-overflowed `fsm_run_loop` at BOUND=1M+ because per-frame allocas disabled
tail-call optimization.

## Problem

Nested enum inlining needs `sret` slots. A per-frame `%enum_out`/`%esrt`
alloca in a self-tail-recursive defn prevents LLVM from eliminating the
recursive call. `fsm_run_loop` then grows the stack until segfault.

Constraint:

> Any sret slot used by a recursive function must live outside the recursive
> frame, or the recursion must be gone before the slot is allocated.

## Recommended solution: direct self-tail-recursion lowering

Lower a **direct self-tail-recursive defn** to a loop:

```
defn f(n, state) {
    if done { term base; }
    let r = step(state);
    term match r {
        Ok(next) => f(n - 1, next),
        Err(_)   => f(n - 1, state),
    }
}
```

becomes:

```
header:
    phi n, state
    br done, base, body
body:
    r = step(state)
    (n, state) = match r { ... }
    br header
base:
    ret base
```

Properties:

- no recursive self-call remains;
- sret slots allocate once in the loop frame;
- parameter assignment is explicit;
- guard/FP order is preserved;
- non-tail and mutual recursion are untouched.

This is the recommended durable fix: no hidden ABI, benefits FSM and similar
drivers, composes with Option B layout.

## Fallback: hoisted sret slots

If tail lowering cannot be made general quickly:

- detect self-tail-recursive defns;
- add hidden `ptr` slot parameters for their sret call sites;
- allocate the slots in the non-recursive caller;
- recursive calls pass the same slots;
- no per-frame alloca, so TCO remains possible.

This is smaller but creates a special recursive ABI. Use only with an explicit
retirement gate.

## Tail-position definition

A direct self-call is tail when it is:

- the expression of a `Term`/`Tail` statement;
- recursively in a `Match` arm body whose result is tail;
- the final expression of a `Block` that is tail;
- a tail branch of an `if`/else expression (when supported).

Any self-call not in such a position makes the defn **not eligible** for
lowering; keep the existing call.

## Analysis slice (first)

Add `src/analysis/tail_recursion.rs`:

- scan every `Definition` for direct self-calls;
- count total self-calls and tail-position self-calls;
- classify:
  - `TailRecursive` — at least one self-call and all are tail;
  - `NonTailRecursive` — self-call exists outside tail position;
  - `NotRecursive`;
- expose the sret callees reached by eligible defns as a follow-up fact (derive
  from `defn_sret` in the backend once the plan is wired);
- no codegen change.

Tests:

- direct tail recursion → `TailRecursive`;
- call bound to a let then used in term → `NonTailRecursive`;
- mutual recursion → `NotRecursive` at this slice;
- non-recursive → `NotRecursive`.

Diagnostic: `BRIEV_DIAG_TAIL=1` prints the classification for each defn.

## Fallback result — TCO-safe hoisted sret slots (2026-10-21)

Implemented the fallback first because it directly removes the per-frame
allocation that blocked TCO:

- `TailRecursionPlan` classifies direct self-tail recursion.
- Tail-recursive defns with sret callees receive hidden caller-owned
  `%hsretN` slots.
- Recursive self-calls pass the same slots; non-recursive callers allocate
  them.
- Sret calls inside the recursive body use the hoisted slot.
- Hidden params intentionally do **not** use `noalias nocapture`: recursive
  calls keep the pointers live across the call, and LLVM-optimized miscompiled
  FSM output when those attributes were present.
- Sret candidate callers that are themselves sret are allowed to pass through.

Results:
- FSM BOUND=5/1M/50M correct (`2000008`, `571428714296`); no stack overflow.
- Harness: fsm 2.95x, spsc 1.87x, ws 1.61x; no correctness failures.
- Full lib 3,293 green; quick guard PASS.

The tail-lowering pass remains the long-term cleanup; this hoist is the
TCO-safe bridge that lets nested B3 be retried.

## Lowering slice (second)

- Implement `TailRecursionLowering` as a first-class frontend/backend pass:
  - one header block with phis for parameters;
  - body emits the original non-tail code;
  - each tail self-call assigns new parameter values and branches to header;
  - base case returns.
- Preserve guards/contracts and effect order.
- Reject lowering when:
  - any self-call is non-tail;
  - parameters have destructuring/unsupported forms;
  - multiple return arities or control escapes;
  - contracts/observable boundaries would change.
- Gate by `BRIEV_DIAG_TAIL_LOOP=1` first; compare against baseline IR/output.

### FSM acceptance

- `fsm_run_loop` IR contains a loop and no self-call in tail position.
- BOUND=5, 1M, 50M output identical to baseline.
- No stack growth/segfault.
- `tcp_step` sret slot allocated once per loop frame.
- `malloc(16)` count for FSM reaches 0 after B3 re-enable.

## B3 retry over hoisted sret — REJECTED (2026-10-21)

Applied the B3 nested-inline patch on top of hoisted sret slots.

- BOUND=0/1/2 matched C.
- From BOUND=3 onward FSM diverged; BOUND=5 Briev printed `4000001` vs C
  `2000008`.
- Cause: the bound `next` state is an interior pointer into the hoisted
  `tcp_step` sret slot. On the next recursive iteration, `tcp_step` reuses that
  same slot, destroying the incoming `state` before/while computing the new
  result.
- Zero `malloc(16)` is therefore unsafe: a mutable temporary cannot own a
  loop-carried state value.

Preserved on `experiment/nested-b3-hoist` (`2805db65`), not mergeable. Safe
state remains `366ad30a`.

A correct design needs loop-carried ownership: when an inline nested enum
becomes a state/phi value, copy it into a stable loop slot or lower the
tail-recursive defn to a loop with dedicated phi storage. Reboxing at the
binding would restore correctness but retain one allocation per transition, so
it is not a valid zero-box optimization.

## Re-enable nested layout

After tail lowering is accepted:

1. Rebase `feat/nested-enum-layout` B3 on top.
2. Enable nested inline payloads.
3. Verify:
   - FSM 0 boxes at BOUND=50M;
   - ws BOUND=5/50M;
   - SPSC boxes 0;
   - full suite MATCH.

## Gates

- `cargo test --lib`; tier-1 daily smoke.
- `bash benchmarks/effect_shape_guard.sh --quick` + `--timing`.
- Full runtime suite.
- CI LLVM 18.
- IR check: no self-call in lowered tail position; no per-iteration enum malloc.
- Output parity at BOUND=5/1M/50M.
- No benchmark regression beyond noise.

## Rollback

- Safe B2: `feat/nested-enum-layout` at `d65fa78c`.
- Failed nested attempt: `experiment/nested-enum-b3` (`9a922d04`).
- This branch: `feat/tail-recursion-lowering`.

## Commit log

| Commit | Change |
|---|---|
| `9aceb86e` | plan |
| `31efb33c` | tail-recursion analysis + tests |
| `e3b6d914` | TCO-safe hoisted sret fallback |
| _pending_ | FSM/CI verification |
| `experiment/nested-b3-hoist` | B3 retry over hoisted sret — rejected (slot aliasing) |
