# wasm32 generic collection ABI

**Date:** 2026-10-23
**Status:** COMPLETE for List<T>; JSON ingestion remains a separate blocker
**Branch:** `fix/wasm-generic-collections`
**Base:** `feat/fullstack-task-board` at `7bff81bf`.

## Problem

`List<T>` is generic at the source/type/op level:

```briev
struct ListBuffer<T> { data: Ptr<T>; }
coll obj List<T> { inner: ListBuffer<T>; }
```

The compiler scaffolds `Init`, `InitEmpty`, `InsertAt`, `At`, `Count`, and
friends from the element type. However, wasm32 codegen still assumes the
native 64-bit layout in several collection paths. Minimal `List<Int>` on
wasm32 fails before `b-each` runs:

```llvm
%t27 = load i64, ptr %t26
%t29 = inttoptr i32 %t27 to ptr  ; invalid: i64 value into i32 conversion
```

This blocks the struct-collection `b-each` work preserved on
`experiment/struct-collection-each` (`948838f5`).

## ABI rule

One target-consistent representation:

- `Ptr<T>` is `ptr` when stored in a struct field; its boxed integer handle is
  `i{ctx.int_bits}`.
- Platform `Int` is `i{ctx.int_bits}`.
- `String`/`Blob` are `ptr` values; boxed handles follow `ctx.int_bits`.
- Element stride is the target-correct size of `T`.
- Explicit `Int64`/`Bits<64>`/length-header fields stay 64-bit.
- Native x86_64 behavior is unchanged.

Hardcoded `i64` is only valid where the format itself is 64-bit.

## Work slices

### G1 — reproduce and pin

- Add wasm32 `List<Int>` construction/iteration regression.
- Pin invalid mixed-width pointer conversions before fixing.
- Include `List<String>` and `List<struct>` follow-ups.

### G2 — pointer-width collection access

- Audit boxed-instance member reads (`emit_instance_column_row`): pointer
  fields load as `ptr`, handles convert at `ctx.int_bits`.
- Audit collection `At`/`InsertAt` reads and writes.
- Audit `construct_local_collection` and `construct_local_collection_seed`.
- Remove `i64` where it encodes a pointer or platform Int.

### G3 — element stride

- Replace scaffold's fixed `elem_size = 8` with target-aware element size.
- Keep capacity allocation conservative but consistent with the actual stride.
- Keep `[len][bytes]` String headers explicitly 64-bit.

### G4 — generic collection tests

- wasm32 `List<Int>`: literal, Count, At, insert, `b-each`.
- wasm32 `List<String>`: pointer elements and shim decode.
- wasm32 `List<Task>`: struct elements and field projection.
- native x86_64 regression suite remains green.

### G5 — resume b-each work

- Rebase `experiment/struct-collection-each`.
- Validate JSON → `List<Task>` ingestion on the web target.
- Then structured Task Board rendering.

## Gates

- focused wasm IR tests;
- `llc -filetype=obj`;
- `cargo test --lib`;
- full `rbv_gate.sh`;
- Task Board API and browser gates;
- LLVM determinism quick gate;
- full runtime suite when native codegen is touched.

## Execution log

- G1: `List<Int>` wasm build reproduced the mixed `i64`→`i32` conversion.
- G2 landed:
  - boxed-instance `Ptr<T>` members load/store as real `ptr`, with
    target-width handle normalization;
  - boxed-instance reads and generic field access use the receiver's actual
    handle width;
  - collection `Resize#`/`EnsureCap#`/`TrimCap#` use `ctx.int_bits`;
  - collection-valued state initializers bind the constructed handle directly
    instead of wrapping it in another `Init`;
  - `.^Length` on String/Blob narrows its stored i64 header to target `Int`;
  - unary negation uses the operand width;
  - plain Int globals and Int constants use target width;
  - foreach counter slots use target Int width (String/Data byte cursors stay
    i64);
  - webstack allows the already-implemented `CharCount#`/`Count#`/`At#` ops.
- G3: not needed for correctness yet; the scaffold still over-allocates
  capacity with `elem_size = 8`, but indexed access uses the real element type.
- G4/G5 landed:
  - `List<Int>`, `List<TaskItem>`, and both in ONE unit build for wasm32,
    pass `llc`, and render correctly in Chromium;
  - collection construction resolves member bodies from the concrete
    monomorph key (`List<T>`), not the generic base;
  - multiple `b-each` containers now receive stable IDs (closing tags were
    never parsed by `inject_ids`, so the first each swallowed the rest of the
    view).

## Resolved — List<T> monomorphized op resolution

`ensure_mono` already created `List<T>`-substituted member bodies, but
`construct_local_collection` still looked members up by the base `"List"` key.
Construction now resolves the concrete monomorph first, falling back to the
base only when no mono entry exists. Regression coverage builds
`List<Int>`, `List<TaskItem>`, and both in one unit.

## Separate blocker — web JSON ingestion

`std/json.bv` now reaches web codegen after enabling `CharCount#`/`Count#`, but
the prelude still contains native-only helper ABI paths (function-pointer /
await helpers) that emit mixed i64/i32 conversions. JSON ingestion on wasm
must wait for that separate prelude ABI cleanup.

No app-level rendering workaround has been added.
