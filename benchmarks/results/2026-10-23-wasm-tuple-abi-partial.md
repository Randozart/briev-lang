# wasm32 tuple/enum ABI — partial slice

**Date:** 2026-10-23

## Landed

- target-width tuple handles and element reads;
- target-width enum handles and payload slots;
- target-width match/union extraction;
- same-width cast identity handling;
- target-width String slice bounds;
- boxed Bool/Char call-argument unboxing.

## Verification

- `tests/rbv_list_each_wasm.rs` now also builds `tuple_return_wasm` through
  `llc` (tuple return, destructuring, numeric field access, float_fmt import).
- `cargo test --lib`: 3,307 PASS.
- full `rbv_gate.sh`: PASS.
- Task Board API + Chromium CRUD gates: PASS.
- full runtime suite: all MATCH.

## Remaining

JSON ingestion now reaches `field_of` and fails LLVM dominance verification:
a reassigned `JsonValue` local is alloca'd inside a match arm and read outside
it. This is the next isolated blocker.
