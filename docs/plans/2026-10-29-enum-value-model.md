# Hybrid enum value model

**Date:** 2026-10-29
**Status:** RECONCILED — implementation strategy superseded by
`docs/plans/2026-10-29-destination-ownership-reconciliation.md`; experiments
and evidence remain historical
**Scope:** concrete enum representation, ownership, ABI, state/list/struct
layouts, and performance.
**Parents:** `docs/plans/2026-10-29-enum-by-value-abi.md`,
`docs/plans/2026-10-28-v0.1-preview-readiness.md`.

## Objective

Replace today’s handle/sret enum ABI with a **semantic enum value model** and
select the fastest safe physical layout per concrete enum type and context.

The semantic rule is:

- an enum is a discriminated value;
- ownership is explicit at memory/ABI boundaries;
- no internal enum may expose a pointer into a dead frame.

The physical rule is:

- use SSA/register values when that is the fastest safe shape;
- use compact inline tagged unions for memory-resident values;
- box only when the layout planner proves inline has poor shape or the ABI
  requires a stable external representation.

## Current failure being fixed

Every enum return is a handle to a `[tag, payload…]` image. `sret` returns
require `retain_enum_return`, copying the image into the caller slot. Removing
that copy produces dangling stack pointers and crashes server gates. Correct
copying makes `ws_frame` 1.58x C. Existing alternatives all failed:

- boundary by-value: still ~0.04s;
- heap boxing: ~0.045s;
- forwarding to `%enum_out`: no speed win;
- unsafe no-copy: dangling callee stack.

## Execution log

### 2026-10-29 — Stage 0 groundwork

- Added `EnumValueRepr::{ScalarValue, PackedMemory, OwnedBox}` and the initial
  four-word packed threshold to `EnumLayoutPlanner`.
- Added deterministic classification tests: two-word scalar, nested inline
  packed, oversized owned box, and depth-capped nested payload.
- Added function-local register representation metadata and a no-IR-change
  backend query/regression test.
- Added a passing state-enum lifetime fixture.
- Found and recorded a real red defect: a four-element
  `List<Result<Int,String>>` sequence makes index 2 become `Err`; the ignored
  regression pin is `tests/enum_list_lifetime_regression.rs`, reproducer is
  `tests/tier1/enum_list_lifetime.bv.known-bug`, and BUGS.md records the alias.

Stage 1 remains open until the list-alias defect has a non-weakening fix or is
subsumed by the first aggregate slice.

### 2026-10-29 — Stage 2 ScalarValue prototype (not committed)

An environment-gated aggregate prototype was built for the `ws_frame` chain:
`{ i64, i64 }` constructors, locals, calls, returns, and matches. It produced
`0016384`, emitted zero `%enum_out`/`ret_copy`, and passed `cargo test --all`
with the gate enabled. Prototype patch: `/tmp/opencode/enum-value2-stage2.patch`.

The performance gate failed:

- current copying path: ~0.045s;
- ScalarValue prototype: ~0.044s;
- unsafe no-copy control at HEAD: ~0.012–0.016s.

After LLVM inlining, the prototype performs **more** `WsFrame` allocations than
the no-copy control (6 vs 4 inside optimized `ws_decode`). Thus the two-word
enum aggregate alone does not remove the dominant cost: the enum payload is a
boxed `WsFrame` object. Correct fast execution requires object payload values
to participate in the same value model (stack/inline or proven allocation
reuse), not merely enum boundaries.

Stage 2 is therefore **not merged**.

### 2026-10-29 — small-object destination reuse prototype

A second hand-transform extended `ws_partial` and `ws_decode_mask` with hidden
object-result destinations. The constructor wrote directly into a caller-owned
34-byte slot instead of `malloc`; `ws_decode` propagated a frame destination.

Performance gate: **PASS** at old-baseline range:

- ScalarValue only: ~0.044s;
- object destination reuse: ~0.012–0.017s;
- unsafe no-copy control: ~0.012–0.016s;
- current copying ABI: ~0.045s.

The hand prototype was not output-correct (`acc=10920`, expected `0`): the
manual IR transform did not implement every object-result path (notably the
unmasked `Ok(f)` copy into the destination) with full ownership/lifetime
rules. That is expected for a hand transform and does not refute the model; it
identifies the implementation work: compiler-managed object destinations,
destination select/copy for every constructor path, and ownership proofs.

Gate conclusion: allocation reuse/destination construction is the missing
performance mechanism. The next compiler slice must implement it generally,
not enable the partial hand IR.

### 2026-10-29 — production ObjectDestination slice (not merged)

The first compiler implementation was environment-gated
(`BRIEV_ENUM_VALUE2`) and combined:

- ScalarValue aggregate ABI;
- hidden object-result destinations for small obj helpers;
- destination forwarding through aggregate call chains;
- alias-aware fresh scratch slots;
- escaping-result copies back to the caller destination.

It reached correct `ws_frame` output in one configuration, but later
alias/lifetime refinements still crashed. A mis-scheduled object destination
caused LLVM to select the extended-length path on a masked raw byte. The
default compiler was never changed; the complete experiment is preserved at
`/tmp/opencode/object-destination-slice.patch` (ephemeral).

**Gate: FAIL.** The slice is not merged. The required next design is a formal
destination/liveness pass, not additional ad-hoc register metadata:
destination SSA values, escape/overlap proofs, and explicit copies at ABI
edges. This likely needs its own plan and backend rewrite before retry.

## Physical classes

### 1. `ScalarValue`

Small, non-recursive enum whose concrete layout is register/aggregate friendly
(first slice: exactly two i64 image words).

Use for:

- locals;
- parameters;
- returns;
- match results;
- hot scalar fields in expressions.

LLVM prototype:

```llvm
{ i64 tag, i64 payload }
```

Constructor: `insertvalue`.
Match: `extractvalue`.
No malloc, no `%enum_out`, no `%esrt`, no `retain_enum_return`.

### 2. `PackedMemoryValue`

Inline tagged union used in state, object fields, collection elements, tuple
elements, and closure environments.

Physical target:

```llvm
{ iN tag, payload-overlay bytes with max alignment }
```

First stable slice may keep two i64 words for compatibility; later tightening
must shrink the tag and payload overlay from `EnumLayoutPlanner`.

The memory cell stores the value by bytes, not a pointer to a temporary image.
Each container gains a concrete element/field stride.

### 3. `OwnedBox`

Explicit heap-owned image.

Required for:

- recursive enums beyond the inline depth cap;
- very large/awkward payloads where the cost model proves boxing wins;
- stable C/JS/FFI ABI;
- exported symbol compatibility.

An owned box always has a documented owner. It is never a borrowed stack slot.

### Boundary ABI

Exported and FFI enum symbols keep today’s public ABI. Internal aggregate
implementations are wrapped:

```text
public old handle/sret ABI
  -> unbox old input handles
  -> call aggregate implementation
  -> box aggregate result
  -> write old output
```

No aggregate layout is exposed to C/JS.

## Semantic/ownership invariants

1. No internal value may be a borrowed stack image.
2. Every representation conversion is explicit and table-driven.
3. Memory stores own a complete value or an owned box.
4. Payload ownership follows existing Briev move/copy rules; aggregate copying
   must not silently create two owners of a mutable resource.
5. Aggregate representation is keyed by concrete type/layout, never enum name.
6. Recursive/over-depth values are boxed by layout, not backend name match.
7. Interpreter `Value::Sum` remains the semantic reference.
8. The old ABI exists only at exports/FFI wrappers.

## Staged implementation

### Stage 0 — representation groundwork

Add, without changing emitted code:

- `EnumValueRepr::{ScalarValue, PackedMemory, OwnedBox}`;
- concrete classification from `EnumLayoutPlanner`;
- register representation metadata;
- deterministic tables in `AnalysisResults`;
- tests pinning two-word, nested, depth-capped, and boxed classes.

Gate: all existing tests green; no IR changes.

### Stage 1 — red lifetime tests

Create fixtures that fail under today’s unsafe-path assumptions where possible:

- state enum read after helper return;
- collection enum stored across another call;
- `List<Result<Int,String>>` built in a loop;
- two live enum results across a third call;
- nested payload binding escaping its match;
- recursive/depth-capped enum;
- state/list wasm32 width checks.

Gate: behaviorally correct fixtures pass under current ABI; IR tests expose
old handle/copy assumptions where relevant.

### Stage 2 — ScalarValue slice

Migrate eligible two-word enums in expression/function flow:

- aggregate constructors;
- aggregate locals;
- aggregate calls/returns;
- aggregate match/tag/payload;
- match phis preserve aggregate type;
- remove `%enum_out`, `%esrt`, `retain_enum_return` on this slice.

Fallbacks still boxed initially:

- generic-specialized defns;
- exported/FFI defns;
- tail-carrier recursive defns;
- reassigned enum locals if aggregate allocas are not ready;
- enums crossing a memory boundary.

Gate:

- `ws_frame` output parity;
- `compare_baseline.sh ws_frame` ≤1.10x legitimate target or corrected
  baseline;
- no hot-chain malloc/ret_copy;
- `cargo test --all`.

### Stage 3 — memory representation

Migrate state/struct/list/tuple/closure fields to `PackedMemoryValue`:

- concrete field/element stride;
- explicit store/load;
- no pointer to temporaries;
- update reflection/view metadata and wasm state layout;
- preserve persistence/versioning where applicable.

Gate: no raw internal handle remains in memory storage for scalar enums.
Add list/state/fsm performance benchmarks.

### Stage 4 — ABI wrappers

- exported enum symbols keep old ABI;
- FFI calls box/unbox only at boundary;
- generic specialization classifies concrete instantiations;
- closures choose aggregate or owned-box ABI explicitly.

Gate: real C/JS export round-trip and IR wrapper tests.

### Stage 5 — remove legacy paths

After all value classes are covered:

- delete sret/retain for eligible enums;
- retire dead `InlineLocalCandidate`;
- retire dead escape-analysis scaffolding;
- split old ABI tests into aggregate and boxed cases.

Gate: invariant test proves no internal stack-backed enum handle remains.

## Tests

Analysis/layout:

- two-word scalar payload;
- nested inline enum;
- depth-capped recursion;
- oversized/boxed payload;
- deterministic classification.

Backend IR:

- aggregate constructor has no malloc/ptrtoint;
- aggregate match uses extractvalue;
- aggregate return has no `%enum_out`/`ret_copy`;
- boxed ABI still works for recursion/export;
- export wrapper shape;
- state/list/struct boundary box/load;
- wasm32 no i32/i64 mismatch;
- tail-carrier fallback.

Behavioral tier-1:

- two live results across another call;
- `List<Result<Int,String>>` built in loop;
- state/obj enum survives helper return;
- nested `Result<Result<...>>`;
- recursive enum;
- exported `Result` function;
- wasm state/list enum fixture.

Interpreter parity:

- add enum cases to parity corpus;
- keep `ws_frame_codec_parity` unchanged as reference.

Determinism:

- add `ws_frame`, `fsm_dispatch`, `spsc_ring`, and an enum fixture to the
  determinism corpus.

## Performance gates

- Actual `-O3 -flto` linked binaries, never `llc` inspection alone.
- Interleave reference/experiment/C timings.
- `ws_frame` target: eliminate explicit copies and hot allocation; expected
  legitimate target is old-baseline range, not the dangling-pointer time.
- Full runtime suite all MATCH.
- Add list/state/fsm-heavy rows before tuning inline thresholds.
- Record instruction-count/IR evidence, not only wall time.

## Risks

1. Representation ambiguity: `Type::Custom` alone cannot say whether a
   register is aggregate or handle. Add explicit metadata.
2. Boundary audit: state/list stores currently do not copy enum ownership.
3. Match canonicalization: current code flattens enum result types to
   `Type::int()`.
4. Payload ownership: mutable resources in payloads need explicit rules.
5. State layout expansion: cost model must decide inline vs box.
6. Generic specialization: classification must be per instantiation.
7. ABI stability: exported symbols must remain old ABI through wrappers.
8. IR determinism: all classification maps sorted.

## Go/no-go

Proceed from Stage 0 only after layout classification and metadata are green.
Proceed into Stage 2 only after red tests and a real full-value IR prototype
prove the speed expectation. Do not tag preview until a safe enum model passes
performance, correctness, browser, and server gates.
