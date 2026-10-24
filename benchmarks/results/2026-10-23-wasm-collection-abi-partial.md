# wasm32 collection ABI — partial slice

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

## Remaining

- `List<T>` monomorphized op registry (base name `"List"` keys collide);
- struct-element list store with concrete element types;
- web JSON ingestion (`std/json` prelude ABI).

## Gates

- `cargo test --lib`: 3,305 PASS
- full `rbv_gate.sh`: PASS
- Task Board API gate: PASS
- Task Board Chromium CRUD gate: PASS
- new `tests/rbv_list_each_wasm.rs`: PASS
- full runtime suite: all MATCH
