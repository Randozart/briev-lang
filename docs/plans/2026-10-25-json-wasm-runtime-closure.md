# JSON wasm closure and website runtime

**Date:** 2026-10-25
**Status:** Phase C partial — unused-trap observability and lazy logical
semantics landed; metadata allocator remains
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
   exits and dominance/cache tests at `-O0`/`-O2`, plus an
   `nbody_newton_accel` non-hang gate before re-landing.
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

## Execution log

- `dc342cad` — strict parser + stringify port (Otto's `14f7f666` /
  `892dcd6c`), including target-width integer accumulation and `cast_lanes`
  removal.
- `107c3017` — boxed tuple list indexing uses the canonical 8-byte word
  stride; strict JSON object fields after the first pair now resolve.
- `019202f4` — logical short-circuit + mutable parameter entry slots
  (`5b9ac7f8`) was tested and reverted: strict JSON works without it, and it
  hangs `nbody_newton_accel`. It moves to the separate Phase C gate with a
  required nbody regression.
- `1317e8bf` — interpreter scalar `.^^Bytes` returns the value width and
  `Mod#` aliases remainder, restoring the reference JSON parse path.
- `0bd6ed1e` — strict parse/stringify gates added to the daily corpus.
- `66b33459` — restore the empty-object `find_pair` guard.
- `0d82ed9e` — collection `.^Length` uses the target-width handle and adapts
  the i64 len header; wasm stringify compiles and round-trips Unicode.
- Task Board structured Chromium CRUD passes with the strict parser.

### Validation checkpoint — Phase A complete

- `cargo test --lib`: 3322 passed.
- `cargo test --test daily_use_smoke`: 25 passed (strict parse/stringify +
  adopted Result/collection/matrix gates).
- Task Board structured Chromium CRUD with strict JSON: PASS.
- Task Board API: PASS.
- `rbv_gate`: OK.
- determinism quick: ALL DETERMINISTIC.
- full runtime suite: PASS, all correctness MATCH/PASS (including
  `nbody_newton_accel`; the logical patch was reverted before this run).
- wasm stringify probe: Unicode/control round-trip true.
- `nbody_newton_accel`: finishes on current build; 5b9 logical patch hangs it
  and remains a separate gated follow-up.

### Phase B execution log

- `de574ea0` — WebHeap allocator (malloc/free/realloc/calloc/memcpy/memmove/
  memset, validated ranges, coalescing reuse, host static strings).
- `a2038053` — dynamic collection view snapshots (`realloc`, no 1024-word
  global), helper-write commit closure in the flush batch/sizing, 1100-item
  Unicode snapshot gate.
- `87b9d607` — foreach/match lexical bindings survive nested operator
  elaboration.
- `node tests/web_heap.mjs`: PASS.
- `node tests/web_collection_snapshot.mjs`: PASS (Unicode, 1100 items,
  shared snapshots, reset).
- Task Board Chromium CRUD with heap + dynamic snapshots: PASS.

### Validation checkpoint — Phase B complete

- `cargo test --lib`: 3325 passed.
- daily corpus: 25 passed.
- Task Board structured Chromium CRUD: PASS.
- Task Board API: PASS.
- `rbv_gate`: OK.
- determinism quick: ALL DETERMINISTIC.
- full runtime suite: PASS, all MATCH/PASS.
- `node tests/web_heap.mjs`: PASS.
- `node tests/web_collection_snapshot.mjs`: PASS (Unicode, 1100 items,
  shared snapshots, reset).
- CI now runs both browser runtime gates after the release build.

### Phase C execution log

- `fba17b79` — adopted logical short-circuit + mutable parameter slots
  (`5b9ac7f8`), with the stale-`terminated` reset needed for pre functions,
  async bodies and the reactor dispatch loop. `nbody_newton_accel` no longer
  hangs; `benchmarks/accel_reactor_nonhang.sh` runs in CI.
- `c7c0b5d3` — may-trap defns and callers emit #15 without `willreturn`;
  unused `unwrap(bad);` SIGILLs and Result mode 3 pins it.
- Validation: 3330 lib tests, 25 daily, Task Board Chromium/API, rbv,
  determinism, accel non-hang and full runtime suite all pass.
- Remaining Phase C: module metadata allocator + target-width constants.
