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
