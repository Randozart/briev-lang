# Eternal ABI model and temporal transport policy

**Date:** 2026-10-27
**Status:** ACTIVE
**Rule:** compiler keeps the eternal (physical ABI, layout, proofs, IR
invariants); app/stdlib keep the temporal (transport policy, timers, retries).

## Why

Recent recommender debugging fixed symptoms one wasm shape at a time: tuple
stride, call i32/i64 coercion, enum/Result state width, parsed struct-list
traps, view `.^Size` staleness, reconnect/save timing. Several were real
compiler gaps; one was external event ordering. The pattern is a temporal
smell when per-shape patches replace a single model.

## Phase A — stop symptom patching

No new per-type/per-app width arm until the model/invariants exist.

## Phase B — one physical ABI/layout query

Introduce one compiler query:

```text
abi(ty, position) -> { llvm_type, size, align, kind }
kind = Scalar | TargetHandle | BoxedEnum | Aggregate | TupleSlot | CollSlot
```

Consumers:
- call args/returns
- state slots/loads/stores
- list/Ptr/collection read/write/resize
- tuple fields
- allocation sizes

`TypedRegister` carries the actual LLVM type; delete side tables and ad-hoc
width branches (`reg_llvm_ty` migration target).

## Phase C — verifier + invariants (FIRST)

- Run LLVM verification (`opt -passes=verify -disable-output`) on every emitted
  module before clang/llc. Invalid IR is a compiler diagnostic, never a clang
  error or runtime trap.
- Assert allocation size ≥ max emitted field end.
- Reject unsupported wasm lowerings at compile time; no runtime `unreachable`
  stubs for compiler gaps.
- ABI mismatch is an internal hard error with callee/param context.

## Phase D — collection/parser contract

One collection slot model; generic fixtures across literal/parsed lists and
Int/String/Float/struct/tuple/enum elements, one and 1682 items, repeated scans.

## Phase E — temporal transport layer

- Explicit `Live | Recovering | Offline` state machine; no timer-vs-click
  outcome.
- General virtual clock in the web test runtime for `event_after`.
- Tests force offline and online paths, never sleeps.

## Phase F — diagnostics as proof consumers

- Unproven collection index → hard error or checked/Result access.
- Unsupported view projection → compile error, never stale DOM.
- Ledger: a proven fact with no consumer is a dev/CI warning.

## Phase G — resume app acceptance

Only after B–F: parsed-list/reconnect path, browser gate, PR readiness.

## Acceptance

- No per-case target-width branches outside `abi()`.
- Emitted IR verified before external tools.
- Generic type-matrix tests pass on wasm.
- Transport tests use virtual time; no sleeps.
- No app names in compiler.
- Browser gate deterministic.

## Progress log

- 2026-10-27: plan created. Phase C starts with LLVM verification and layout
  allocation invariant.
