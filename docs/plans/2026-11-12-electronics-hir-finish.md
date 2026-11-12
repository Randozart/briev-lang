# Finish the electronics HIR port (2026-11-12)

Status: ACTIVE. Work branch: `electronics-hir-port2` (WIP commit `a31c9d87`,
34/45 parity). Main keeps the green 14-test subset. Goal: full AST↔HIR parity,
consumer flip, and removal of the documented unsupported list.

## Where the branch stands

- The WIP is **not missing scaffolding**: `src/hir/electronics.rs` (~5.3k), new
  `src/hir/electronics_laws.rs`, electrical fidelity in
  `program/lower/expr/to_ast/walk/stats/needs_state/verify`, and functions for
  wildcards, net membership/labels, series synthesis, current/power proofs,
  budgets, modes, mechanisms, and DC-state consumption.
- Diagnostic formatting is normalized (continuation artifacts removed, unit
  suffixes restored). Parity is 34/45.

### The trap: several WIP tests assert behavior the AST does not produce

`parity()` compares AST vs HIR dumps, then the test asserts a feature fact. For
many failures the dumps match, but the AST itself lacks the fact because the
test source is a simplified board. **AST is the oracle**: replace these sources
with the AST reference tests, never weaken the HIR to satisfy an invented case.

| WIP test | Cause | AST reference tests |
|---|---|---|
| `series_part_forcing_and_e_series_synthesis_match` | no load laws/series path | `series_part_forced_by_current_obligation`, `e_series_window_synthesizes_resistance`, `understrength_series_candidates_refuse_with_window`, `synthesis_skips_when_load_bounds_absent` |
| `current_bound_proven_by_fixpoint_and_vacuity` | no constitutive law | `bound_without_derivable_current_is_not_vacuously_proven`, `law_power_requires_a_rating_and_checks_it` |
| `max_current_and_drive_capability_proofs_match` | no forward law/branch | `max_current_envelope_proves`, `max_current_violation_is_an_error`, `drive_capability_proves_within_rating`, `undriven_net_skips_drive_capability` |
| `budgets_pass_and_violate` | board does not draw current | `BUDGET_BOARD` + `budget_under_derived_draw_passes`, `budget_exceeded_is_an_error`, `budget_reversed_form_and_unit_literals`, `budget_on_unknown_pin_is_an_error` |
| `named_modes_solve_per_mode_and_select_nodes` | no pins/driven rails | `spst_mode_ebv_fixture_selects_operating_states`, `contradictory_mode_precondition_has_no_operating_state`, `guarded_diode_law_solves_forward_mode`/`reverse_mode` |
| `resistor_law_dc_solve_proves_bounds` | under-specified law board | `dc_law_solve_derives_resistor_branch_current`, `dc_law_solve_classes_divider_mid_net`, `underdetermined_dc_law_group_is_a_hard_error`, `contradictory_dc_law_boundaries_are_a_hard_error` |
| `mechanism_synthesis_matches`, `mechanism_without_candidate_is_a_hard_error` | condition pin path unresolved | `mechanism_synthesis_wires_bridge`, `ambiguous_mechanism_requests_strategy`, `relay_with_coil_pair_synthesizes_bridge`, `signal_level_when_demands_mechanism` |

### Real gaps to fix (after source replacement)

| Test | Symptom | Suspect |
|---|---|---|
| `wildcard_broadcast_and_instance_array_match` | `t[*].a.voltage` nets 0 vs 2 | `expand_wildcard` / instance-array naming |
| `voltage_obligation_pull_up_forcing_matches` | parity drift: no “pull-up forced” | `force_voltage_obligations`, free PullUp pick |
| `stdnet_membership_inferred_and_labeled` | membership proof absent | `infer_rail_membership_hir`, `resolve_net_labels`, annotations lowering |
| budgets/currents/series/laws | may shrink after Phase 0 | run the AST reference sources first |

## Phase 0 — Re-baseline the suite on AST truth (first)

1. Shared corpus `tests/fixtures/electronics/parity/*.ebv` (one per feature)
   copied **verbatim** from the AST reference tests above, plus
   `tests/electronics/*.ebv` and `examples/{electronics,silicon}/*.ebv`.
2. `tests/electronics_hir_parity.rs` (or unit test module):
   - each fixture: parse/lower → AST `derive_netlist` vs HIR `derive_netlist`;
     canonical dumps must be exactly equal;
   - run the feature predicate against the AST result first; if AST fails, the
     fixture is wrong (fail with that message).
3. Replace/delete the invalid WIP test sources; keep feature assertions.
4. Add a parity-audit ratchet listing known-diff fixtures (may only shrink).
5. Acceptance: no test asserts something AST does not do; baseline recorded.

## Phase 1 — Intermediate fact dumps

Test-only deterministic dumps on both sides: type pins/classes/units, instances
(+tables/annotations), pin unions, net bindings/labels, component laws, DC
states/modes, budgets, participation. Compare stage-by-stage to separate
lowering-fidelity gaps from solver logic bugs.

## Phase 2 — Fix in dependency order

1. **Wildcard instance arrays** — `expand_wildcard`, element naming,
   `resolve_element_pin`; reference `wildcard_*` tests.
2. **Net membership/labels** — annotations payload, `collect_net_names`,
   `infer_rail_membership_hir`, `resolve_net_labels`; reference `stdnet_*` +
   `ldo_output_law_births_the_rail`.
3. **Pull-up/voltage obligations** — `force_voltage_obligations`,
   `ObligationForcing`, free PullUp pick, proofs; reference
   `examples/electronics/usb_sensor.ebv`, `examples/silicon/die_board.ebv`.
4. **Budgets** — `check_budgets`, net draw, state currents, unit literals;
   reference `BUDGET_BOARD` tests.
5. **Current/power proofs** — divider/KCL, fixpoint/vacuity,
   `check_max_current`, `law_power`; reference `max_current_*`, `drive_*`.
6. **Series synthesis** — probe laws/rail births, `synthesize_series_values`,
   E-series helpers; reference the four series tests.
7. **Laws/DC/modes** — `hir/electronics_laws.rs`, `solve_law_states`,
   `merge_law_rail_boundaries`, mode predicates/validation,
   `derive_electrical_proofs`; `electronics_dc` stays shared (result types
   only).
8. **Mechanisms** — `run_intent_pass`, `FactSink`, `condition_control`,
   `synthesize_bridges`, `through` markers, conditional bridge strings;
   reference mechanism tests.
9. **Tables/buses/participation sweep** — table-indexed fields, bus error
   shapes, unpop/bistable interactions.

Each fix: AST oracle untouched; add the AST source to the corpus; run all
gates.

## Phase 3 — Consumer flip

- Deterministic `consumer_fingerprint(&ElectronicsNetlist)` (production-safe
  canonical dump) or `PartialEq` on the result structs.
- `src/backend/mod.rs` `AnalysisResults.electronics` uses HIR derive by
  default; AST derive is the debug oracle while `BRIEV_HIR_EMIT` is on.
  The field type stays `ElectronicsNetlist`, so `compile.rs`/KiCad consumers
  are unchanged.
- Gates: AST electronics 157, HIR parity 45, quick, sweep, all `.ebv`
  fixtures/examples, A/B byte-identical, determinism, Praetor.

## Phase 4 — Merge/cleanup

- Remove `src/bin/scratch_elec.rs` (already gone) and the WIP plan; update the
  completion plan + H5 inventory (electronics flipped, unsupported list
  deleted), architecture/electronics docs, BUGS.
- Merge by cherry-picking `a31c9d87` then fix commits (or squash), keeping the
  AST analyzer as oracle until the opt-out release closes.

## Validation

`cargo test --lib analysis::electronics` (157); `hir::electronics` (45 target);
electroncs corpus parity (`.ebv` fixtures/examples); `scripts/gate.sh quick`;
corpus sweep; `hir_emit_ab.sh --quick`; `ll_determinism_check.sh --quick`;
Praetor on `src/hir`/`src/analysis`.

## Open decisions

1. Corpus form: shared `.ebv` fixtures (recommended) vs inline sources.
2. `electronics_dc` stays shared (recommended) vs ported.
3. Merge style: cherry-pick history vs squash.
4. Flip policy: HIR default with AST oracle during the opt-out window
   (recommended, same as reclamation/region) vs AST default until the window
   closes.
