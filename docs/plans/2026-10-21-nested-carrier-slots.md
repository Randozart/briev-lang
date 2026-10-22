# Nested enum carrier slots

**Date:** 2026-10-21
**Status:** ACTIVE — ownership experiment.
**Branch:** `feat/nested-carrier-slots`
**Base:** safe TCO hoist, `feat/tail-recursion-lowering` at `19ac9996`.
**Failed predecessor:** `experiment/nested-b3-hoist` (`2805db65`) — inline
`next` pointed into the hoisted `tcp_step` sret slot; the next recursive
iteration destroyed incoming `state`.

## Rule learned

An sret temporary owns its value only until it is consumed. If an inline
nested enum becomes a loop-carried recursive argument, it must be copied into
storage whose lifetime spans recursive iterations. A mutable temporary cannot
simultaneously be the old state, the new state, and the recursion's incoming
state.

## Design

Target **direct self-tail-recursive defns** whose recursive arguments include
inline nested enum handles.

For each such loop-carried argument:

- add one hidden `ptr %carrierN` parameter;
- the initial caller allocates the carrier using the concrete nested layout;
- recursive self-calls pass the same carrier;
- at the self-call boundary, if the argument is a local bound to an inline
  nested result, copy the nested image from the temporary into the carrier and
  pass the carrier;
- if the argument is already the carrier (e.g. unchanged state on an Err arm),
  pass the carrier unchanged;
- all other arguments keep existing semantics.

The temporary sret result and the carrier never alias. Thus `tcp_step` may
write its result while `state` still points at the carrier.

## FSM shape

Before:

```
r    -> hoisted tcp_step temp
next -> interior of r
fsm_run_loop(..., next, ...)  // next dies when tcp_step temp reuses slot
```

After:

```
r        -> hoisted tcp_step temp
carrier  -> stable state image owned across recursion
next     -> interior of r
copy next image -> carrier
fsm_run_loop(..., carrier, ...)
```

Err path:

```
fsm_run_loop(..., carrier, ...)  // unchanged state
```

## Ownership analysis

Classify each tail self-call argument:

| Class | Meaning |
|---|---|
| `InlineNestedCarrier` | local bound directly from an sret call whose result layout has an inline nested payload |
| `ExistingCarrier` | already the corresponding carrier param |
| `Opaque` | boxed/unknown; no carrier |

First implementation accepts the FSM shape:

- direct self-call in tail position;
- argument is an identifier bound to a local `let r = sret_call(...)`;
- the callee result layout marks that variant payload inline;
- corresponding function parameter is enum-typed.

Everything else remains boxed/no carrier.

## ABI

Tail-recursive defn signature:

```
... existing params ..., ptr %carrier0[, ptr %carrier1 ...]
```

Carrier params come after existing hoisted-sret params and before ordinary
user parameters in the hidden-prefix ordering.

- Recursive self-calls pass their own carrier params.
- Initial callers allocate carrier slots and pass them.
- Carrier size is `EnumLayout.image_words * 8` for the concrete param type.
- Hidden carrier params use plain `ptr align 8`; do not mark `noalias
  nocapture` because recursion keeps them live across the self-call.

## Ownership copy

At a self-call whose argument local is `next`:

1. `next_handle` already points to the temporary result image.
2. Emit `temp = inttoptr next_handle`.
3. For each word in the nested image, load temp word and store carrier word.
4. Pass `ptrtoint carrier` as the recursive argument.

For an unchanged-state Err argument, pass the existing carrier handle
directly.

## Slices

### C1 — carrier analysis

Add `src/analysis/tail_carrier.rs`:

- identify direct self-tail-recursive defns;
- inspect tail self-call arguments;
- classify inline-nested carrier candidates;
- report carrier count/order;
- tests for FSM-like and non-tail/mutual/boxed shapes.

### C2 — carrier ABI

- Store carrier metadata in backend context/function context.
- Add hidden carrier params to eligible tail-recursive defns.
- Initial callers allocate/pass carriers.
- Recursive self-calls reuse carrier params.
- No codegen for non-eligible defns.

### C3 — ownership copy

- At eligible self-call arguments, emit the image copy into the carrier.
- Pass `ptrtoint carrier` as the recursive argument.
- Err/existing-carrier arguments pass the carrier without copy.

## Acceptance

- FSM output matches C for BOUND `0..8`, `1_000_000`, and `50_000_000`.
- No stack growth/segfault.
- FSM enum `malloc(16)` count remains 0.
- SPSC remains 0 boxes and ~1.9x.
- ws remains correct.
- Full runtime suite MATCH; `cargo test --lib`; guards; CI LLVM 18.
- No benchmark regression beyond noise.

## Risks

1. Carrier parameter ABI changes recursive and initial callers; both must be
   updated together.
2. Multiple carrier args require stable ordering.
3. A wrong layout size or missing copy reintroduces state corruption.
4. Hidden params cannot use `noalias nocapture` because recursive calls keep
   them live.
5. Non-FSM recursive patterns may be rejected conservatively; that is safe.

## Rollback

- Safe base: `feat/tail-recursion-lowering` at `19ac9996`.
- Failed predecessor: `experiment/nested-b3-hoist` (`2805db65`).
- This branch can be abandoned without touching the safe state.

## Result (2026-10-21)

Carrier slots landed and validated:

- FSM output matches C at BOUND `0..8`, `1_000_000`, and `50_000_000`.
- FSM enum `malloc(16)` count: **0**.
- FSM ratio: **1.48–1.49x** (down from 3.08x).
- SPSC: **1.89x**, 0 enum boxes.
- WS: **0.27–0.29x**, Briev wins.
- Full runtime suite: all MATCH.
- `cargo test --lib`: **3,296 passed**.
- Effect-shape quick guard: PASS.

The stable carrier removes the aliasing failure without per-transition malloc.
The implementation is intentionally conservative: only direct self-tail
recursion with a local sret result and a destructured nested payload is
eligible.

## Commit log

| Commit | Change |
|---|---|
| `92508278` | plan |
| `cbbd4d5e` | C1 carrier analysis + C2 ABI |
| `2805db65` | predecessor nested layout (rebased here) |
| `04dd1681` | C3 ownership copy + nested layout |
| _next_ | results pin |
