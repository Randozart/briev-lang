# wasm32 tuple / enum-return ABI

**Date:** 2026-10-23
**Status:** PARTIAL — tuple/enum ABI fixed; JSON builds but runtime-crashes in browser.
**Branch:** `fix/wasm-tuple-abi`

## Problem

`std/json.bv` reaches wasm codegen after the collection fixes, then fails in
prelude helpers because tuple/enum handles and scalar elements mix native i64
and wasm32 i32:

- `sscale` returns `(Int, Int)` as an i64 pointer handle from an i32-return
  function.
- Tuple destructuring loads i64 elements and passes them to i32 parameters.
- Enum constructors store/return i64 handles against i32 results.
- Match scrutinee/payload extraction inttoptr's i64 from i32 handles.
- Slice bounds and boxed Bool/Char call arguments use i64.

## Fixes landed

1. Tuple materialization returns `ptrtoint` at `ctx.int_bits`.
2. Tuple field/destructure reads use the element's target width for scalar
   elements (`i8`, `i16`, `i32`); String/Blob still load the boxed i64 pointer.
3. Enum construction returns and stores payload words at target width; scalar
   payloads widen directly to the i64 slot.
4. Match/union payload extraction converts handles at `ctx.int_bits`.
5. Same-width cast-graph `ZExt`/`Trunc` lanes emit an identity copy instead of
   invalid `zext i32 ... to i32`.
6. Dynamic String slice bounds adapt to the target-width
   `briev_str_substr` ABI.
7. Boxed Bool/Char parameter registers (`%acN`) unbox at call boundaries when
   the callee expects i8/i32.

## Regression pin

`tests/fixtures/tuple_return_wasm.rbv` covers tuple return, destructuring,
numeric field access, and `float_fmt` import for wasm32.

## Remaining blocker

`std/json.bv` now compiles to wasm. The browser runtime then crashes inside
`parse_tasks`/`parse_string` (target crash), after the tuple, char-length,
boxed-Bool, and host imports were fixed. This is a runtime parser issue, not a
compile failure. It needs a minimized fixture before any Task Board integration
is attempted.

## Gates

- `cargo test --lib`: 3,307 PASS
- wasm tuple/list regression: PASS
- full `rbv_gate.sh`: PASS
- Task Board API + Chromium CRUD gates: PASS
- full runtime suite: pending record
