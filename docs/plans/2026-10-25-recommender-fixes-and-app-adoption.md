# Recommender fixes and app adoption

**Date:** 2026-10-25
**Status:** A/C/D landed; B rejected; E deferred; app still external
**Branch:** `feat/recommender-fixes`
**Sources:**
- standalone app: `odecode/recommender-app` @ `79234665` (publish `32491a3c`)
- compiler recovery commit: `odecode/brief-lang` `894fcfa5` (parent `1c1d4028`)

## Goal

Adopt the useful compiler/runtime/stdlib work exposed by the standalone
recommender application without copying a non-buildable product tree into the
compiler, and without discarding the fixes already landed on current main.

The app remains an external acceptance target until its stdlib prerequisites
build. Each compiler change must be independently testable and bisectable.

## Source assessment

`recommender-app` is a two-commit standalone snapshot with no vendored compiler
or stdlib. It needs:

- `std/db/pg.bv`
- `std/web/websocket.bv`
- `std/web/events.bv`
- `std/web/random.bv`
- async `http_async_*` fetch handles
- `std/rating_population.bv`
- a port from `Matrix`/`Vector` to `FloatMatrix`/`FloatVector`/`IntVec`

Its real gates require PostgreSQL, MovieLens data, and Playwright/Edge. It is
therefore an optional integration lane, not default CI.

The compiler fix commit `894fcfa5` mixes compiler, web, app, docs and tests. Only
files whose pre-image still matches current main are safe to adopt whole.

## Bundle A — wholesale-safe compiler fixes

Adopt whole files:

- `src/backend/llvm/helpers.rs` — pre-image matches current main:
  - stateless String allocation/free
  - mixed integer operand widening
  - boxed-handle widening
  - enum ownership and enum-return retention
  - integer coercion
- new `src/backend/llvm/crash_regressions.rs`
- new fixtures:
  - `tests/fixtures/browser_json_values.rbv`
  - `tests/fixtures/enum_aggregate_lifetime.bv`
  - `tests/fixtures/enum_local_return.bv`

Wiring:

- `mod crash_regressions;`
- make `logical_tests::clang_ir` visible to the sibling test module

Attribution:

- record source SHA in `docs/contributors/otto-westerlund.md`;
- use original author/co-author on the commit.

Gate: `cargo test --lib`, fixture compilation/link/run, wasm JSON/enum probes.

## Bundle B — conditional casting boundary

`src/casting/graph.rs` has a matching pre-image, but changes `Data → String`
from `CastFromBitCallback` to `IntToPtr`.

Adopt only if all remain green:

- strict JSON and browser JSON values
- WebHeap `briev_bits_to_str`
- Task Board and collection snapshots

Otherwise keep the old lane and adapt the app later.

## Bundle C — web/view fixes

Targeted hunks, not whole-file replacement:

- trigger argument separator (`args.join(", ")`, no leading comma)
- item-trigger parameters and per-item evaluation
- multiple event attributes on one element
- unchanged-text suppression (`_setText` / equality check)

Preserve current WebHeap, UTF-8 b-each, keyed refresh and b-show/b-hide work.

Gates: browser trigger regression, star-rating syntax, catalogue selection,
repeated flush observers, Task Board Chrome.

## Bundle D — WASM lowering

- run `opt -S -passes=mem2reg,tailcallelim` before `llc`
- resolve versioned `wasm-ld` through `clang -print-prog-name=wasm-ld`
- retain the original export list and wasm-ld flags
- add a bounded-stack catalogue scan gate

This is a correctness gate, not a throughput claim.

## Bundle E — specialization and remaining backend hunks

- add `generic_txns` to compiler context
- extend the specialization worklist to callable transactions
- preserve contracts/convergence
- gate with concrete `iter_chain<MovieCard>` / multi-struct catalogue iteration
- audit and port only missing width/lifetime hunks:
  - Bool pattern literal width
  - stateless declared call widths
  - browser JSON integer/boolean widths
  - escaping aggregate/enum lifetime

Do not whole-file replace `emit_expr.rs`, `emit_stmt.rs`, `emit_toplevel.rs`,
`mod.rs`, `pipeline.rs`, `typechecker/mod.rs`, `compile.rs`,
`web_generator.rs` or `view_compiler.rs`: current main has diverged.

## Phase D — stdlib prerequisites

1. PostgreSQL client (`fix/pg-receive-length` plus contract proofs); no weakened
   network bounds; byte-level and optional live SQLSTATE gates.
2. Portable browser stdlib: async fetch request handles, WebSocket, events,
   random, host JS transport.
3. `std/rating_population.bv` and CSR; port app's `cf_engine` to
   `FloatMatrix`/`FloatVector`/`IntVec`.
4. Keep recommender-specific algorithms in the example, not compiler vocabulary.

## App adoption

Recommended: keep `recommender-app` external and pinned at `79234665` until
Phase D builds it.

If vendored later:

- copy as `examples/recommender` (whole directory is mechanically possible but
  not buildable yet);
- port imports and matrix names;
- add Otto to `AUTHORS`/`NOTICE` and preserve the app license;
- keep PostgreSQL/MovieLens/Playwright gates optional and explicit;
- never make external services a hidden `cargo test --lib` dependency.

## Other open work retained

- Remaining Phase C metadata work is complete; keep its regressions.
- `nbody_newton_accel` non-hang gate remains in CI.
- Strict JSON, browser heap/snapshots, logical semantics and target-width fixes
  remain authoritative; recommender fixes must not revert them.
- Stdlib breadth beyond recommender: PG, portable web, CSR, then deferred
  app/auth/cache/persistence.

## Gates per bundle

- `cargo test --lib`
- `cargo test --test daily_use_smoke`
- `cargo build --release --bin brievc`
- `bash benchmarks/rbv_gate.sh`
- `bash benchmarks/ll_determinism_check.sh --quick`
- `bash benchmarks/accel_reactor_nonhang.sh`
- Task Board structured Chromium + API
- `node tests/web_heap.mjs` + collection snapshot
- full runtime suite before merge

## Attribution

Original recovery work by Otto Westerlund (`@odecode`). Record source SHAs and
adaptations in `docs/contributors/otto-westerlund.md`; preserve authorship where
possible and add co-authorship on adapted commits.

## Acceptance

- Bundle A/B/C/D/E are independently merged with focused regressions.
- App fixtures compile and run once their prerequisites land.
- The app remains labelled external/non-buildable until Phase D is green.
- Otto is credited in the repository, commits, PRs and app NOTICE if vendored.

## Execution log

- Bundle A — adopted `helpers.rs`, `crash_regressions.rs` and the browser
  JSON/enum-lifetime fixtures. Ported typed pattern-literal equality and enum
  ownership call sites; fixed owned enum handles to target width. Commits:
  `16e6c8a7` (adoption), `0c5f9605` (target width).
- Bundle B — rejected: whole-file `casting/graph.rs` changes Data→String to
  IntToPtr and fails existing Bit→String encoding tests. Keep the current
  encoding lane; adapt the app later.
- Bundle C — landed `5cf8526b`: trigger separator, item trigger params,
  multiple event attributes, class pairs, unchanged-text updates. View and
  generator tests plus Task Board Chromium pass.
- Bundle D — landed `c693c196`: `mem2reg,tailcallelim` before `wasm-ld`;
  versioned linker resolution through clang. Task Board and JSON wasm builds
  pass.
- Bundle E — deferred: partial generic-transaction specialization changed
  list typing in a concrete `iter_chain` probe (`Count#`/`.^Length` saw Int).
  Keep the existing defn-only worklist until a full frontend/txn specialization
  change can be gated by the multi-struct catalogue fixture.

## Validation checkpoint — bundles A/C/D

- `cargo test --lib`: 3340 passed.
- `cargo test --test daily_use_smoke`: 27 passed (including enum aggregate,
  enum local return, Result, collection, matrix, JSON gates).
- Task Board structured Chromium CRUD: PASS.
- Task Board API: PASS.
- `rbv_gate`: OK.
- determinism quick: ALL DETERMINISTIC.
- accel reactor non-hang: PASS.
- browser heap + 1100-item Unicode snapshot: PASS.
- full runtime suite: PASS, all MATCH/PASS.

Bundles B and E are not merged. B is rejected on the Bit→String encoding
test; E is deferred on concrete list typing in `iter_chain`.
