# wasm32 obj-init width fix — record

**Date:** 2026-10-23
**Branch:** `feat/fullstack-task-board`

## Fix

Object value handles now follow the target pointer width. Struct-literal
members load at their declared LLVM types rather than through native-i64
boxed words. Boxed-instance member reads use the same target-width and native
member rules.

Old wasm32 IR:

```llvm
%t9  = ptrtoint ptr %t5 to i32
%t10 = inttoptr i64 %t9 to ptr
%t12 = load i64, ptr %t11
```

New wasm32 IR:

```llvm
%t9  = ptrtoint ptr %t5 to i32
%t10 = inttoptr i32 %t9 to ptr
%t12 = load ptr, ptr %t11
```

## Gates

- focused regression: `tests/rbv_obj_init_wasm32.rs` PASS
- full `rbv_gate.sh`: PASS
- `cargo test --lib`: 3,303 PASS
- LLVM determinism quick gate: PASS
- Task Board API gate: PASS
- Task Board frontend browser gate: PASS
- full runtime suite: all MATCH

No performance claim is made from the runtime table; this is a correctness and
target-ABI slice.
