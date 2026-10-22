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
| _pending_ | plan |
| `_this commit_` | tail-recursion analysis + tests |
| _pending_ | lowering pass (gated) |
| _pending_ | FSM/CI verification |
| _pending_ | B3 re-enable |
