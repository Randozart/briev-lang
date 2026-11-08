# Post-integration next leg (2026-11-08)

State: `main` at the post-#78 integration (all Otto PRs merged, zero open PRs,
all gates green). This leg closes the remaining debt and resumes the H5 path
in a fixed order.

## Leg A — finish Praetor leg 2 (`src/hir` 79 → 0)

Ordered by count, same method as before (verbatim body extraction, ctx
structs to keep params ≤6, gates per group):
1. `needs_state.rs` (12) — split the local/stateful walkers into per-variant
   helpers; the two walkers already mirror each other, so extract shared
   arms into `expr_locals_expr` / `stateful_expr` sub-functions.
2. `defn_liveness.rs` (10) — split the root-walker arms and the shape
   collector into helpers.
3. `transition_graph.rs` (7) — `is_pure_body` (21), `propagate_live_once`
   (21), `extract_bounded_pre` (17), plus the three lexical O(n²)
   loop-depth sites (per-arm helpers).
4. Nits: `spawn_pool` 6, `enum_repr` 5, `task_linear` 4,
   `slp_isomorphism` 4, `lower` 4, then 1–2-diagnostic files
   (`swan_song`, `specialize`, `density`, `web_commits`,
   `global_lifetime` walker, `dyn_concrete_sets`, `task_segments`,
   `coll_length`, `to_ast`, `modulo_partition`, `verify`).

Gate per group: `cargo test --lib`, full sweep (137/0), A/B, Praetor delta.
Exit: `src/hir` 0 diagnostics (or a documented exception list).

## Leg B — pipeline HIR + flip the port-only analyses

1. Build the `HirProgram` once in the pipeline (where `termination` and
   `watchdog` currently run on items in `compile.rs`/`pipeline.rs`).
2. Flip both consumers under `BRIEV_HIR_EMIT` with debug equality asserts
   (their ports already exist: `hir::termination`, `hir::watchdog`).
3. Gate: full battery + their diagnostics fixtures (the parity tests exist
   in-module).

## Leg C — H5 remaining ports (focused sessions, named consumers first)

- `accel` (2791 lines) — blocks `coalescing` + `image_storage`.
- `gpu_schedule` (1129), `electronics`, `reclamation` lineage.
- Every port: named consumer before code (inventory rule), parity tests,
  consumer flip under the flag.

## Leg D — `Cast.via` frontend typer (design-first)

Scope how the typer's inferred types/cast lanes can populate HIR value
facts (`HirExpr.ty`, `Cast.via`), then MethodCall/Reflect receiver typing
becomes HIR-native. Deliverable first: scoping notes like `emit_match`'s,
then implementation.

## Coordination

The Intel/OpenVINO workstream is owned by the separate agent
(`docs/plans/2026-11-05-intel-xpu-openvino-harvest.md`); its harvest branch
is already merged. Do not touch `benchmarks/intel_ptl/` or the accel W3
lines from this leg; rebase on `origin/main` before each push.

## House gates for every commit

`cargo test --lib` green; full-corpus debug sweep (AST/HIR `.ll` equality);
`benchmarks/hir_emit_ab.sh`; `ll_determinism_check` for emission-touching
changes; Praetor on changed dirs; docs updated in the same commit.
