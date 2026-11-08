# HIR Praetor remediation — leg 2 (2026-11-05)

Continues the owner-directed full remediation from
`2026-11-05-h4-completion-loop-convergence-swan-song-emitter.md` (whose
checklist this plan executes). `emit_hir_stmt.rs` is already
Praetor-clean; the remaining offenders are the HIR analysis ports.

## Scope (exact sites — `praetor validate --warn --target src/hir`)

Order is by severity/leverage:

1. **transition_graph.rs** — `build` (cognitive 72) → split into
   `build_node` + `resolve_bounded_pre` + `pin_live_modifier_fields` +
   `collect_unguarded_ffi`; then `simplify_expr` (49),
   `compute_live_fields` (28), `is_pure_body` (21),
   `detect_lexicographic_ranking`/`collect_or_vars` (20/28),
   `detect_all_increments` (20), `detect_popcount_decay` (21),
   `detect_increments` (21 + cyclomatic 16), `extract_bounded_pre` (17).
2. **watchdog.rs** — `analyze` (47) → per-txn check + no-conflict/var
   reporters; `check_falsifies_in_body` (33), `check_restores_in_body`
   (30), `check_on_fire_handlers` (17).
3. **needs_state.rs** (sites 118/214), **defn_liveness.rs** (29/283).
4. **global_lifetime.rs** — `analyze` (24), 102-line
   `collect_state_identifiers` split.
5. Nits (1–2 diagnostics each): spawn_pool, task_linear,
   slp_isomorphism, swan_song, specialize, enum_repr, density,
   web_commits.

## Method (every refactor)

- Bodies moved verbatim; helper extraction only. Params ≤6 (ctx structs
  where needed, like `MatchCtx`/`MatchArmView` in emit_hir_stmt).
- Gates after each: `cargo test --lib`, full debug sweep (AST/HIR `.ll`
  equality is the behavioral net), `benchmarks/hir_emit_ab.sh --quick`,
  and a `praetor validate --warn --target src/hir` delta check.
- Continuous commits; push per refactor group.
- Legacy `src/analysis` / `src/backend/llvm` baseline stays documented
  (house rule) — not part of this leg.

## Exit criteria

- `src/hir` diagnostics at 0 (or a documented exception list with
  reasons).
- All gates green; IR byte-identical (sweep + A/B).
- Praetor on changed dirs stays in the per-commit checklist.

## After this leg (resume roadmap)

1. `Cast.via` frontend-typer arc scoping (MethodCall/Reflect receiver
   typing) — investigation + scoping notes like `emit_match`.
2. Remaining H5 ports (`accel` 2791 blocks coalescing/image_storage;
   `gpu_schedule` 1129; `electronics`; `reclamation` lineage), each a
   focused session with named consumers.
3. Pipeline-HIR build (unblocks `termination` + `watchdog` consumer
   flips), then the H5 default flip per the retirement inventory's
   preconditions.

## Leg 2 remainder (written 2026-11-05, executed immediately after)

1. **transition_graph.rs** (16 diagnostics):
   - `detect_increments` (21/16) → per-statement `increment_of(name, expr)`
     helper + add/sub dual-site helper.
   - `detect_popcount_decay` (21) → per-statement `popcount_of(name, expr)`
     helper.
   - `detect_all_increments` (20) → reuse `increment_of`.
   - `is_pure_body` (21) → extract the increment-gated Assign arm into
     `pure_assign_check`; split statement walk from expr walk.
   - `extract_bounded_pre` (17) → extract the comparison-pair match into
     `bounded_from_pair`.
2. **needs_state.rs** (118/214) → line-length/complexity at those sites.
3. **defn_liveness.rs** (29/283) → same treatment.
4. **global_lifetime.rs** — `analyze` (24) → extract manual-free/keep scan
     and touch-set build into sub-functions; split the 102-line
     `collect_state_identifiers` into per-variant helpers (expr walk +
     stmt walk already separate; split expr variants into grouped
     helpers).
5. **Nits** (1–2 each): spawn_pool, task_linear, slp_isomorphism,
     swan_song, specialize, enum_repr, density, web_commits.

Method unchanged: bodies verbatim, helper extraction only, params ≤6 via
ctx structs where needed, gates after each group (lib + sweep + A/B +
praetor delta), continuous commits.

## Execution record (2026-11-08)

Closed. `praetor validate --warn --target src/hir` → 0 diagnostics
(initial 105). Commit chain: `a7a6e170` (defn_liveness), `e46453c5`
(transition_graph), `8bf3968b` (spawn_pool), `a6d9bd00` (enum_repr),
`ed64c876` (task_linear/slp/lower), `189b24ab` (swan_song/global_lifetime),
`5d8c1521` (density/specialize/loop_shape/dyn_concrete_sets/storage_class/
coll_length/web_commits/task_segments/to_ast).

Method notes (add to the standard recipe):
- Tuple destructuring still counts as 2 params in `datalog-rule-4`; use a
  named ctx struct (`SyncGroupCtx`, `LetSite`) rather than a tuple.
- Nested-loop flags (`O(n²)`) are cleared by extracting the per-item
  dispatch into a sibling fn (`written_of_stmt`, `collect_stmt_surface`,
  `walk_one_assign_target`) — the recursive call then sits at depth 1.
- `loop { for … }` fixpoints become `while round(..) {}` with one round fn.
- A Praetor-reported line number points at the fn's *start* line for
  loop-depth findings; grep the fn name, not the line.

Every group gated on: `cargo test --lib` (3553), corpus sweep
(137 ok / 4 skipped / 0 mismatch), `benchmarks/hir_emit_ab.sh --quick`
(byte-identical), plus the affected parity suites.

Next per the roadmap above: `Cast.via` scoping (#1).
