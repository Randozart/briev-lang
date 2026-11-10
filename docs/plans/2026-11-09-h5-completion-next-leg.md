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

## P5 — pass-level snapshots (second, DONE 2026-11-09)

- `hir::render`: deterministic canonical HIR text (ids, kinds, types, markers).
- `tests/hir_snapshots.rs` + `tests/snapshots/hir/*.snap`, refresh via
  `UPDATE_SNAPSHOTS=1`; capture after lower, after specialize, and for the
  consumer-flip fixtures.
- Two-run byte determinism is the gate; snapshots complement (not replace)
  `ll_determinism_check`.

## Seam refactors (third, ALL DONE 2026-11-09)

1. **Defer**: `defer_bodies: Vec<Vec<HirStmt>>` on the backend;
   `flush_defer_cleanup` walks the statement seam. Delete the AST-body queue.
2. **ArrowAssign**: register-taking strategy helpers; update
   `find_insert_strategy`/`find_extract_strategy` consumers (wire fire,
   streams, provenance) so no AST-shape dispatch remains.
3. **Lambda/Spawn**: HIR-taking closure construction and task/pool/storage
   helpers; `PendingClosure` built from HIR. Remove AST dispatch wrappers.

Each refactor: A/B byte-identical, sweep, affected runtime fixtures, Praetor.

## A2 typing + `emit_match` bodies (fourth, DONE 2026-11-09)

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

## H5 default flip (sixth, DONE 2026-11-09 — owner sign-off)

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

**P5 (2026-11-09, DONE).** `hir::render` renders a deterministic canonical
HIR text (ids, best-effort types, law markers, cast paths, contracts,
specialized callables). `tests/hir_snapshots.rs` pins lower/typed snapshots
for five focused fixtures plus one specialization snapshot under
`tests/snapshots/hir/`; refresh with `UPDATE_SNAPSHOTS=1`. The renderer is
order-stable by construction (vec order only, no map iteration) and is
asserted stable within-process; snapshot comparison supplies the
cross-process check. `.gitignore` whitelists `tests/snapshots`.

Next: the three seam refactors.

**Seam refactor 1 — Defer (2026-11-09, DONE).** `defer_bodies` now carries
`Vec<HirStmt>`; the HIR `Defer` arm queues the HIR body directly, the AST arm
(and the countable-loop synthesized defer) lower at registration via
`hir::lower_body_statements`, and `flush_defer_cleanup` emits through the
statement seam (lossless fallback per child). Found and fixed en route: a
node firing that ends by **fallthrough** (no explicit `term`/`rollback` in the
emitted body) never flushed its defer stack; the SSA loop now flushes before
its per-firing exit checks. New coverage: `tests/directives/defer_cleanup.bv`
(stdout `1\n7\n`) and a `defer` HIR snapshot fixture.

**Seam refactor 2 — ArrowAssign (2026-11-09, DONE).** The arrow dispatch no
longer reads AST shapes: `ArrowOperand` exposes only `name()` (AddrOf-peeled
identifier), `declared_type()` (identifier binding or tuple element types) and
`emit()` (HIR operand first, lossless AST fallback for an uncovered leaf).
`find_insert_strategy_name`, `find_extract_strategy_for_arrow_name`,
`insert_element_type_name`, `emit_strategy_{member,fn}_call_name`,
`emit_arrow_store_name`, and `emit_destroy_consumed_name` are the name-based
equivalents (AST variants delegate). The function is split into
wire/stream/insert/extract/plain-copy helpers, each returning `Option` so the
main path is a flat dispatch. The only remaining AST construction is the
`Print#` call for `#StdOut` (reuses the print dispatcher). Gates: arrow suite
24; lib 3573; sweep 139/0; A/B byte-identical; all-tests 3620; Praetor clean
on the new code.

Next: Lambda/Spawn seam refactor.

**Seam refactor 3 — Lambda/Spawn (2026-11-09, DONE).** Closure bodies are HIR:
`PendingClosure.body` and `ClosureDef.body` carry `HirExpr`; AST lambdas lower
their body once via `hir::lower_standalone_expr`; the AST and HIR arms share
`emit_lambda_value`; `emit_one_closure` emits through `emit_hir_operand` with
the reconstructed AST as fallback. Free variables come from
`hir::free_vars::collect_free_vars`, a scoped walker mirroring the AST
collector (params/nested-lambda/let binding, first-appearance order, `#`
intrinsics excluded). Spawn arguments are index-addressed through `SpawnArgs`
(`Ast`/`Hir`; HIR prefers the operand path) with shape probes for the event-wire
rule; `emit_spawn_value` is the shared task/pool/storage dispatcher
(`SpawnCtx` keeps it within the parameter limit). Gate: lib 3573; spawn 31 /
closure 14; sweep 139/0; A/B byte-identical; all-tests 3620; Praetor clean on
the new code.

All three seam refactors are complete; the H5 flip precondition list now
depends only on `emit_match` bodies and the owner sign-off.

**A2 slice — Field/Index typing (2026-11-09, DONE).** `hir::typing` builds a
struct/object field table from `HirTypeDefKind` (generic parameter lists kept)
and fills `HirExpr.ty` for `Field` (with substitution at the use site) and
`Index` (`List<T>`/`Array<T>`/`Vector` element). Cast paths now resolve for
field- and index-sourced casts. **A2 completion (same day):** MethodCall results resolve via the owning
member/trait callable (arity incl. `self`; ambiguous arities stay untyped);
dyn receivers resolve through their trait owner; reflection types as `Int`
(every emitted form materializes an i64). A2 is now complete.

**`emit_match` bodies (verified 2026-11-09).** Expression bodies already
dual-fuel (`MatchArms::Hir`, `emit_match_arm_dual`), statement arm bodies emit
children HIR-first with per-child AST fallback, and HIR Match arms are passed
through. The remaining AST body is the collection-witness `List` arm (needs
the declared result type to construct through `Init`/`InsertAt`) — an accepted
constructor exception, same category as the `#StdOut` `Print#` call. The H5
flip's match precondition is therefore satisfied.

**H5 flip readiness probe (2026-11-09).** First full `BRIEV_HIR_EMIT=1
cargo test --all` found nine HIR-only failures, all real fidelity gaps now
fixed:

- `defn_liveness` indexed `impl` member bodies the AST never sees (the
  pipeline erases/merges impls; dyn thunks and member dispatch inline them),
  so `mark("greet")` over-rooted a body. Impl callables (`is_impl_fn`) are
  now skipped, mirroring the AST index.
- ISR handlers were dropped by HIR lowering, so liveness lost their callee
  edges (`{"tick"}` vs `{}`). They now lower as
  `HirCallableKind::IsrHandler` with `is_isr`, rooted by liveness and excluded
  from bare-transaction consumers (swan-song, needs-state) exactly like the
  AST's separate `TopLevel::IsrHandler` arm.

With both fixes: HIR-flag `cargo test --all` 3624/0 and normal `cargo test
--all` 3624/0; sweep 139/0; release A/B byte-identical. Remaining before the
default flip: benchmarks under the flag, then owner sign-off (the flip keeps
`BRIEV_HIR_EMIT` as the opt-out).

**Benchmarks under the flag (2026-11-09).** `BRIEV_HIR_EMIT=1
benchmarks/build_and_bench.sh --runtime --correctness`: every runtime
benchmark MATCH (one PASS for bridge_multi, whose reference also PASSes).
Combined with the byte-identical `.ll` gates, timing parity follows from IR
identity. Every flip precondition is met except owner sign-off.

**H5 default flip (2026-11-09, DONE).** Owner sign-off received.
`hir_emit_enabled()` now defaults to **true**; `BRIEV_HIR_EMIT=0|false|off|no`
opts out for one release (all other values keep HIR on). The AST arms remain
until they are deleted family-by-family; the opt-out is the escape hatch for
any regression the tripwire misses.

Validation at the flip:
- default (HIR) `cargo test --all`: 3624/0; opt-out (AST) `cargo test --all`:
  3624/0.
- Corpus sweep (AST vs HIR `.ll`, 139 programs): 139/0 mismatch.
- `benchmarks/hir_emit_ab.sh --quick`: byte-identical.
- `BRIEV_HIR_EMIT=1 benchmarks/build_and_bench.sh --runtime --correctness`:
  every benchmark MATCH.
- Test harnesses that deliberately compared modes now set `BRIEV_HIR_EMIT=0`
  for the AST arm instead of unsetting it (field_receiver_once,
  mixed_integer_operands, nested_object_storage, deterministic_transitions,
  `hir_emit_ab.sh`).

Next: H5 ports (reclamation parity first) and family-by-family AST-arm
deletion; keep the tripwire and opt-out until a clean release cycle passes.
