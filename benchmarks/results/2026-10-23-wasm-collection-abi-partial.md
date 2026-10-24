# wasm32 collection ABI — List<T> slice

**Date:** 2026-10-23

## Landed

- target-width `Ptr<T>` member loads/stores;
- target-width collection resize/ensure/trim calls;
- direct binding of collection-valued state initializers;
- `.^Length` narrowing for String/Blob;
- target-width unary negation, Int constants/plain globals, foreach counters;
- webstack `CharCount#`/`Count#`/`At#` allowlist entries.

## Result

`List<Int>` now:
- builds for wasm32;
- passes `llc -filetype=obj`;
- renders `1, 2` through `b-each` in Chromium.

## Multi-instantiation result

`List<Int>`, `List<TaskItem>`, and both in one unit:
- build for wasm32;
- pass `llc -filetype=obj`;
- render correctly in Chromium (`1, 2` and `one/false`, `two/true`).

The monomorphized member lookup and the `b-each` closing-tag parser were the
two blockers.

## Remaining

- web JSON ingestion (`std/json` prelude function-pointer ABI).

## Gates

- `cargo test --lib`: 3,305 PASS
- full `rbv_gate.sh`: PASS
- Task Board API gate: PASS
- Task Board Chromium CRUD gate: PASS
- new `tests/rbv_list_each_wasm.rs`: PASS (List<Int>, List<TaskItem>, and both in one unit)
- full runtime suite: all MATCH
- full runtime suite: all MATCH
