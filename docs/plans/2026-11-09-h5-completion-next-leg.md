# H5 completion + verification next leg (2026-11-09)

Status: ACTIVE. Continues `2026-11-08-post-integration-next-leg.md` (Legs A/D
done) and the verification umbrella
`2026-11-09-compiler-verification-program.md`. Owner decisions recorded below.

Related: `2026-11-05-h5-retirement-inventory.md` (work list),
`2026-11-04-typed-hir-h3-h5-execution.md` (phase definitions),
`2026-11-09-cast-via-typing-scoping.md` (A1/A2).

## Owner decisions (2026-11-09)

- **Seam refactors: all three.** `Defer`, `ArrowAssign`, and `Lambda`/`Spawn`
  become HIR-native before the H5 default flip. No permanent conversion path.
- **Nightly verification workflow lands with P3** (full opt matrix, wasm,
  interpreter corpus); per-commit gates stay quick.
- **Leg B approach: semantics-preserving multi-lowering.** Build a fresh HIR
  at each analysis point now; a single shared build arrives with the H5
  emitter-entry switch.
- **P5 snapshots immediately after Leg B**, before the seam refactors.

## Leg B — pipeline-HIR flips (first)

Why multi-lowering: `termination` runs before the `Typed`/`Normalized` plugin
stages (`compile.rs:520`, `pipeline.rs:1284`) by design; `watchdog` runs after
`Normalized` (`compile.rs:775`, `pipeline.rs:1274`); the backend HIR is built
later still, after `narrow_slices`/allocation/provenance. One early build
cannot serve all three without changing diagnostics timing or dropping later
AST mutations.

### B1 — termination

- Add `analysis::termination::analyze_dual(items)`:
  AST result is the fallback; under `BRIEV_HIR_EMIT` lower a fresh
  `HirProgram`, call `hir::termination::analyze`, `debug_assert_eq!` HIR ==
  AST, return the HIR result.
- Change both call sites (`compile.rs`, `pipeline.rs`) to `analyze_dual`.
- Port docs/inventory: termination becomes "flipped under the flag".

### B2 — watchdog

- Add `analysis::watchdog::analyze_dual` and
  `check_on_fire_handlers_dual` with the same dual-oracle pattern
  (`hir::watchdog::{analyze, check_on_fire_handlers}`).
- Change all four call sites (two analyses × two pipeline surfaces).
- Coverage note stays: HIR mirrors bare reactive txns, trigger names, on-fire
  set with `is_impl_fn` exclusions.

### B3 — measure the extra lowering

- Record corpus compile-time delta (three lowerings transiently). If material,
  the remedy is the H5 entry switch, not moving diagnostics.

**Gates:** `cargo test --lib`; corpus sweep (AST/HIR `.ll` equality) with the
flag on; HIR-emit A/B; termination/watchdog module parity tests; Praetor on
`src/analysis`, `src/pipeline*`, `src/compile*` changed dirs.

## P5 — pass-level snapshots (second)

- `hir::render`: deterministic canonical HIR text (ids, kinds, types, markers).
- `tests/hir_snapshots.rs` + `tests/snapshots/hir/*.snap`, refresh via
  `UPDATE_SNAPSHOTS=1`; capture after lower, after specialize, and for the
  consumer-flip fixtures.
- Two-run byte determinism is the gate; snapshots complement (not replace)
  `ll_determinism_check`.

## Seam refactors (third)

1. **Defer**: `defer_bodies: Vec<Vec<HirStmt>>` on the backend;
   `flush_defer_cleanup` walks the statement seam. Delete the AST-body queue.
2. **ArrowAssign**: register-taking strategy helpers; update
   `find_insert_strategy`/`find_extract_strategy` consumers (wire fire,
   streams, provenance) so no AST-shape dispatch remains.
3. **Lambda/Spawn**: HIR-taking closure construction and task/pool/storage
   helpers; `PendingClosure` built from HIR. Remove AST dispatch wrappers.

Each refactor: A/B byte-identical, sweep, affected runtime fixtures, Praetor.

## A2 typing + `emit_match` bodies (fourth)

- Extend `hir::typing` for `Field`/`Index` from `hir::type_facts`, then
  `MethodCall`/`Reflect` from trait/protocol tables (one dyn-dispatch worked
  example first; protocol bindings vs trait functions must be decided there).
- Split `emit_match` arm bodies through the seam (queued H4 item; H5 flip
  precondition).

## H5 ports (fifth)

Order: `reclamation` HIR parity first (it just changed in #79/#81;
reference-verified, AST-only), then `electronics`, `dependency_graph` +
`destination_ownership` verification + small-consumer sweep (`casing`,
`dataflow`, `range`, `region`, `purity`, `allocation`), then `accel` (unblocks
`coalescing` + `image_storage`), then `gpu_schedule`.

Each: named consumer, parity tests, flip under the flag with a debug equality
assert, inventory row updated.

## H5 default flip (sixth, gated)

Preconditions: match bodies; three seam refactors; structural families native;
tripwire clean across consecutive landings; `daily_use_smoke` + benchmark
MATCH; owner sign-off. Then: `hir_emit_enabled()` defaults true,
`BRIEV_HIR_EMIT` becomes opt-out for one release, AST arms deleted
family-by-family (Rule 6 inverts for H5), `to_ast` shrinks to zero.

## Verification increments (interleaved, seventh)

- `verification-nightly.yml`: full tier1 opt matrix + skip inventory, wasm
  cross-backend gate, interpreter-over-corpus using #80's deterministic
  harness.
- P4: tier1 seeds, growth slices; P7 curated corpus; P6 cvise wrapper.

## House gates for every commit

`cargo test --lib`; corpus sweep 139/0; `hir_emit_ab.sh --quick`;
`ll_determinism_check` for emission-touching changes; Praetor on changed dirs;
docs updated in the same commit.

## Execution record

**Leg B (2026-11-09, DONE).** `analysis::termination::analyze_dual`,
`analysis::watchdog::{analyze_dual, check_on_fire_handlers_dual}` lower a
fresh HIR at each analysis point and consume it under `BRIEV_HIR_EMIT`, with
the AST result as fallback and `debug_assert_eq!` oracle. All six call sites
(`compile.rs` ×3, `pipeline.rs` ×3) switched. Extra-lowering cost measured on
the release compiler (`--llvm`, 2 runs each): matrix_pipeline 521–543 ms AST
vs 551–567 ms HIR; sweep_sparse 488–501 vs 498–508; linked_list 480–496 vs
529–530 (~5–9% on small programs, flag-only). Gates: `cargo test --all` 3620
pass; corpus sweep 139/0 with the flag on (debug oracles armed); HIR-emit A/B
byte-identical; termination 16 / watchdog 37 module tests.
