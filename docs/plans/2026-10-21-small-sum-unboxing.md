# P2-1 small-sum unboxing — implementation plan

**Date:** 2026-10-21
**Status:** ACTIVE — first slice: representation/escape analysis.
**Branch:** `feat/small-sum-unboxing`
**Evidence:** FSM 2.86x C, SPSC 1.99x C; `emit_enum_construct` always
`malloc(16)` for `{tag, payload}`; match reloads tag/payload.
**Related:** `docs/plans/2026-10-21-post-effect-shape-next-work.md`.

## Goal

Remove per-construction heap boxes for small enums/`Result` values without
changing source, adding a keyword, or special-casing a type name. Escaping
values keep the existing boxed ABI.

## Current representation

- Enum value = `i64` handle to a 16-byte heap image: `{i64 tag, i64 payload}`.
- Construction: `malloc 16`, store tag, `adapt_to_i64(payload)`, return
  `ptrtoint`.
- Match: `inttoptr`, load tag, compare; payload binding `inttoptr` + load.
- Function returns are `i64` handle; caller matches it.
- Multi-payload variants stage a boxed tuple in the payload slot.

## Target representation classes

| Class | Representation | Rule |
|---|---|---|
| `InlineLocal` | Stack slot or SSA `{tag,payload}` | constructor result is consumed only inside the defining function, no escape |
| `ReturnedSmall` | hidden caller `sret` 16-byte slot | internal enum-returning defn; result is consumed by the immediate caller |
| `Boxed` | current `malloc` image | state/global/collection/export/dynamic/unknown escape |

The representation is derived from **use/escape facts**, never from enum names.

## Escape analysis facts

A value escapes if it is:

- stored in `%State`, a global, a collection, an aggregate, or an object field;
- returned from an exported/dynamic/foreign surface;
- passed to a call whose callee is not proven to consume it locally;
- captured by a closure/task/async surface;
- live across a loop/function boundary whose slot lifetime is insufficient.

`InlineLocal` is the first slice; `ReturnedSmall` needs an ABI extension and is
a separate commit.

## Phase A — enum representation plan (first slice)

Add `src/analysis/enum_repr.rs`:

- collect declared enum names and variant payload types;
- collect enum-returning defns;
- classify each enum-returning defn's return form:
  - direct constructor;
  - `match` with constructor/call arms;
  - other (boxed fallback);
- classify local `let` bindings of enum constructors when every use is a
  local match/`is_ok`/`is_err`/`unwrap`;
- default to `Boxed` when any fact is unknown.

Add tests:

- direct constructor return → `ReturnedSmallCandidate`;
- match-of-constructors return → candidate;
- constructor stored in a collection/state → `Boxed`;
- local constructor consumed by match → `InlineLocalCandidate`;
- unknown call return → `Boxed`.

No codegen change in Phase A.

## Phase B — InlineLocal lowering

- For a candidate local constructor, allocate a 16-byte stack slot in the
  defining function (or emit SSA tag/payload) instead of `malloc`.
- Match reads the same slot; no heap tag/payload loads.
- Any bail-out (use/escape discovered late) falls back to `malloc` for that
  construction; never emit a dangling stack handle.

Acceptance: local constructor/match IR has no `malloc`; suite MATCH.

### Phase B result (2026-10-21)

Landed `InlineLocal` for defn-local enum constructors:

- a local `let x = EnumCtor(...)` whose sole use is a local `match` or
  `is_ok`/`is_err`/`unwrap`/`unwrap_err` gets a 16-byte entry alloca;
- the constructor writes tag/payload into that slot, no `malloc(16)`;
- match reads the same slot; other uses/escapes fall back to the boxed path;
- `Find_inline_enum_lets` is conservative (exactly one use, same function).

Fixture `tests/tier1/local_enum_smoke.bv` golden-pins output `42\n`.
IR for `f` shows `%einl0 = alloca i64, i64 2` and no `malloc(i64 16)`.

Full lib 3,286 green; effect-shape quick guard PASS. Full runtime suite after
the change: all MATCH (fsm 3.13x, spsc 2.00x unchanged; no benchmark regression).

## Phase C — ReturnedSmall (`sret`) ABI

For internal defns whose return classification is `ReturnedSmallCandidate`:

- Add hidden `ptr %enum_out` (after `%state`/context parameters).
- Constructors in return position write tag/payload into `%enum_out`.
- A `term match` propagates the return slot to its arm expressions.
- Caller allocates a 16-byte slot, passes it, and matches locally.
- If the callee result escapes the caller, fall back to `Boxed` at the call
  site (or pass the caller's own return slot).
- Exported/foreign/dynamic ABI stays boxed.

### ABI decision

Use a hidden `sret` pointer, not a two-word return struct:

- no change to the i64 value ABI outside enum-returning internal defns;
- caller controls slot lifetime;
- LLVM can SROA the slot when the value is local;
- external/exported ABI remains unchanged.

### Return-context propagation

- `term <expr>` and `return` pass `enum_out` into the expression context.
- A `match` in return context passes the slot to each arm's expression.
- A constructor writes to the slot and returns a null/opaque handle; the
  caller's match reads the slot directly.
- A nested call in return context passes the caller's slot to the callee.
- Any non-return use ignores the slot and boxes.

## Phase D — match/destructuring

Support:

- `Ok`/`Err`, `is_ok`/`is_err`, `unwrap`/`unwrap_err`;
- unit variants;
- single-payload variants;
- multi-payload tuple payloads (existing boxed tuple semantics preserved);
- generic `Result<T,E>` payload substitution.

## Acceptance

- FSM transition loop: no per-step `malloc` for `Result<TcpState,FsmError>`.
- SPSC pop: no per-pop `malloc` for `Result<Int,Int>`.
- IR: allocation count in the hot loop drops; tag/payload read from local slot.
- Full runtime suite all MATCH; goldens/parity; no regression on current
  winners.
- `cargo test --lib`; tier-1; effect-shape guards; determinism gate.

## Risks

- Slot lifetime across recursion/async/inlining.
- Return-context propagation through nested matches/guards.
- Generic `Result` payload substitution.
- External/exported ABI must remain boxed.
- Conservative escape analysis must never produce a dangling stack handle.

## Rollback

Each slice is a commit on the feature branch. If InlineLocal/ReturnedSmall does
not improve FSM/SPSC, record the refutation and fall back to `Boxed`; do not
merge a partial representation.

## Commit log

| Commit | Change |
|---|---|
| `b3ef0b4a` | plan |
| `8a27a77f` | enum representation/escape analysis + tests |
| `29d9e0c4` | InlineLocal lowering + local_enum_smoke |
| _pending_ | ReturnedSmall sret ABI |
| _pending_ | match/destructuring coverage |
