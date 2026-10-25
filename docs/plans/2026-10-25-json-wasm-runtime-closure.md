# JSON wasm closure and website runtime

**Date:** 2026-10-25
**Status:** ACTIVE
**Branch:** `feat/json-wasm-runtime`
**Predecessor:** `docs/plans/2026-10-25-otto-contribution-adoption.md`

## Goal

Close the website-critical runtime work left after the first Otto adoption
slice: strict JSON on wasm, browser heap/snapshot scale, and the compiler
correctness batch that those paths expose. Preserve Otto's attribution and
never weaken contracts or add parser-specific workarounds.

## Current evidence

- Strict JSON parser and stringify pass on native.
- wasm Object parse reports the correct pair count.
- wasm `field_of`-style key lookup does not recover the Bool payload
  (`done=false` on toggle; direct `json_parse("true")` returns true).
- The old parser path is green in Task Board on wasm.
- Reflection/liveness/boxed-enum ownership fixes landed in `0db07d3c` and
  `ab47f25b`.

## Phase A — strict JSON on wasm

1. Reapply strict parser `14f7f666` and stringify `892dcd6c` without switching
   the default parser.
2. Add isolated wasm probes for:
   - Object pair count;
   - `pair.0` key extraction and String equality;
   - `pair.1` JsonValue tag;
   - `field_of` return tag.
3. Compare wasm/native IR for:
   - `List<(String,JsonValue)>` element stride and load width;
   - boxed `Result<JsonValue>` payload placement;
   - tuple field offsets.
4. Fix the general boxed enum/tuple/list-width cause; no parser hack.
5. Switch `json_parse` to strict only after the grouped gate passes.
6. Add parse/stringify gates to the daily corpus.

Acceptance: malformed/incomplete/trailing/Unicode parse gates pass native and
wasm; Task Board toggle renders true; stringify round-trips controls; old
Task Board gate remains green.

## Phase B — browser heap and snapshots

Adopt the missing `2f5ad2bf` runtime pieces:

- `WebHeap` malloc/free/realloc/calloc/memcpy/memmove/memset with validated
  ranges and coalescing;
- dynamic collection snapshots replacing `[1024 x i64]`;
- helper-write DOM invalidation;
- lexical elaboration scope after heap/snapshot gates are green.

Keep the keyed `b-each` refresh/order fixes.

Acceptance: 1100-item Unicode snapshot, heap range/reuse/coalescing tests,
Task Board Chrome gate, no native regression.

## Phase C — correctness batch

1. Unused calls to possibly-trapping defns stay observable.
2. Logical short-circuit + mutable parameter slots (`5b9ac7f8`), with loop
   exits and dominance/cache tests at `-O0`/`-O2`.
3. Module metadata allocator + target-width constants (`a42faac5`), with a
   large-universe test.

## Phase D — stdlib breadth

- PostgreSQL client contract pass; no weakened network bounds; byte gate plus
  optional live SQLSTATE gate.
- Portable web cast/prelude, events, WebSocket, fetch, host JS.
- General CSR extraction; recommender-specific shapes stay examples.

## Deferred

- Recommender app/auth/cache/persistence/server/frontend.
- Direct import of stale integration branches.

## Gates per step

- `cargo test --lib`
- `cargo test --test daily_use_smoke`
- `cargo build --release --bin brievc`
- `bash benchmarks/rbv_gate.sh`
- `bash benchmarks/ll_determinism_check.sh --quick`
- Task Board structured Chromium gate
- full runtime suite before merge
- Praetor on changed directories

## Attribution

Strict JSON parser/stringify and browser runtime pieces originate with Otto
Westerlund (`@odecode`). Preserve original authorship where possible and add
source SHA/branch plus `Co-authored-by` on adaptations. Update
`docs/contributors/otto-westerlund.md` with every adopted result.
