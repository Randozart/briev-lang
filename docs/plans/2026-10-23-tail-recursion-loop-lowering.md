# Direct self-tail-recursion loop lowering

**Date:** 2026-10-23
**Status:** DONE — carrier-gated first slice landed.
**Base:** `main` after `ad989e62` (nested carrier baseline).
**Umbrella:** `docs/plans/2026-10-21-tail-recursion-lowering.md`.
**Related:** `docs/plans/2026-10-21-nested-carrier-slots.md`.

## Goal

Lower eligible direct self-tail-recursive defns to an explicit loop in the
LLVM backend. The recursive call disappears; parameters become loop-carried
values; the loop body is emitted exactly once.

This is the durable successor to the TCO-safe hoisted-sret fallback and the
nested carrier-slot ABI. Correctness and ownership must be identical, but the
frame, recursive call, and special lifetime dependence go away.

## Scope

Eligible:

- direct self-recursion only (no mutual recursion);
- first slice is gated to defns with a loop-carried nested-enum carrier:
  plain tail recursion already gets cheap LLVM TCO, and lowering it broadly
  regressed `ws_frame` by ~1.45x in controlled A/B;
- every self-call is in tail position (`Term`/`Tail`, match arm, tail block);
- call arity matches the defn parameters;
- first slice: every parameter has the existing i64 internal binding
  (`Int`, enum handle, boxed non-float, Ptr handle, ptr-ABI struct handle);
- native `float`/`double`, scalar `iN`, and aggregate slots are deferred
  until the header reload has their typed cache;
- no `defer`, `rollback`, or `endprogram` in the body;
- no generic type parameters on the defn (specializations are separate).

Everything else keeps the existing hoisted-sret/carrier/recursive path.

## Design

### Analysis

Add `TailLoopPlan` in `src/analysis/tail_recursion.rs`:

- reuse `build_tail_recursion_plan`;
- require `TailRecursionClass::TailRecursive`;
- collect every self-call and verify arity;
- reject unsupported statements and generic defns;
- expose `is_lowerable(name)`.

The plan is structural. The backend applies the final parameter-representation
check before emitting a loop, because LLVM widths depend on the type universe.

### Function context

Add:

- `tail_loop_header: Option<String>` — current lowered loop header;
- `tail_param_slots: Vec<(String, String)>` — per-parameter alloca + type;
- `tail_loop_backedge: bool` — the last expression ended at the backedge, so
  an enclosing `term` does not emit a second terminator.

`tail_loop_fn` remains the current defn name for existing self-call detection.

### Entry emission

For a lowered defn, after the shared body prologue and other entry allocas:

1. allocate one slot per parameter at its internal LLVM type;
2. store the current parameter binding into its slot;
3. for each nested carrier, seed the stable carrier image from the incoming
   handle;
4. emit `br label %tail_header`;
5. emit `tail_header:`;
6. load every slot, install the loads as the parameter bindings, then emit the
   original body once after the header.

The header must come after every entry-block alloca so LLVM sees no dynamic
stack growth in the loop.

### Backedge emission

In `finish_user_call`, before ordinary call marshaling:

- if the call is the current lowered defn's self-call, store each argument into
  its parameter slot using the slot's internal type;
- for a carrier parameter:
  - temporary nested result: copy the image into the stable carrier and store
    the carrier handle into the parameter slot;
  - unchanged state: store the existing carrier handle;
- emit `br label %tail_header`;
- mark the block terminated;
- return a typed dummy register.

No `call @self` is emitted.

### Exit

Base-case `term`s keep their existing return path. `defer`/`rollback`/
`endprogram` defns are not lowered, so their cleanup semantics are unchanged.

## Ownership rule

The loop-carried nested enum cannot point at a reusable sret temporary. The
loop uses the same stable carrier image already proven by the carrier-slot
slice:

```
tcp_step temp -> copy -> carrier -> parameter slot -> next iteration
```

No malloc, no aliasing, no per-iteration frame.

The hidden carrier ABI remains in this slice. Retiring it is a separate
follow-up after loop lowering is proven.

## Implementation slices

### L1 — analysis + tests

- Add `TailLoopPlan` and eligibility tests.
- No codegen change.

### L2 — loop emission

- Add function-context state.
- Emit parameter slots/header for lowered defns.
- Intercept self-calls and emit backedges.
- Keep the old path for non-lowered defns.

### L3 — ownership integration

- Seed nested carriers at entry.
- Route carrier self-call copies through the parameter slots.
- Verify FSM at BOUND `0..8`, `1_000_000`, `50_000_000`.

## Gates

- `cargo test --lib`
- FSM output parity at BOUND `0..8`, `1M`, `50M`
- FSM IR contains no self-call in the lowered defn
- FSM enum `malloc(16)` remains 0
- `ws_frame`, `spsc_ring` correctness
- full runtime suite all MATCH
- `bash benchmarks/effect_shape_guard.sh --quick`
- `bash benchmarks/ll_determinism_check.sh --quick`
- CI LLVM 18
- no benchmark regression beyond noise

## Rollback

- Branch-local.
- Base `main` at `ad989e62`.
- Keep the hoisted-sret + carrier fallback until loop lowering passes every
  gate.

## Result (2026-10-23)

The carrier-gated slice passes:

- FSM output matches at BOUND `0..8`, `1_000_000`, and `50_000_000`.
- FSM IR has no self-call in `fsm_run_loop`; zero `malloc(16)`.
- Controlled `compare_baseline.sh`: FSM **0.40x** baseline (Briev ~2.5x
  faster), carrier baseline `5a1a0532`.
- The broad first implementation regressed `ws_frame` (~1.45x) by lowering
  plain recursive stdlib helpers. The backend gate now requires a carrier.
  After the gate, `ws_frame.ll` is byte-identical to the carrier baseline and
  interleaved direct runs are within noise.
- `cargo test --lib`: 3,301 green; effect-shape quick guard and LLVM
  determinism quick gate PASS.
- Full runtime suite: all MATCH. Record:
  `benchmarks/results/2026-10-23-tail-loop-lowering.md`.
- Exhaustive same-env corpus IR check: **45/46 byte-identical**; the only diff
  is the intended `fsm_dispatch.ll` loop lowering.

Plain recursion and non-carrier shapes stay on the existing recursive path.
Retiring the hidden carrier/hsret ABI remains a follow-up.
