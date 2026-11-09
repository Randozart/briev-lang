# Reclaim storage published into an incoming parameter

## Goal
Fix the definite memory leak where an allocation stored into a callee's incoming
object parameter was never reclaimed (BUGS.md 2026-11-09), without reintroducing
the WebSocket use-after-free the barrier was added to prevent.

## Investigation
- `record_incoming_store` (`src/analysis/reclamation/walk.rs:354`) marks the
  publishing callable a **barrier**. `emit_entry` for a barrier emits
  `__briev_region_suspend`, so the caller's region is suspended and the
  allocation falls back to plain `malloc` — never freed.
- The barrier exists for the 2026-11-08 WebSocket UAF: an INHERIT lowering would
  reclaim published storage at the caller's return while the receiver outlived it.
- **Experiment (Golden Rule 19):** neutralizing the barrier made the leak
  disappear, but exactly one unit test failed
  (`projection_argument_propagates_incoming_store_barrier`) — the barrier is
  load-bearing for receiver shapes that outlive the frame. So removal is unsound;
  the barrier must be made conditional on the receiver's lifetime.

## Fix
A publisher barriered **solely** for publish-into-incoming is dropped when every
call site passes a receiver argument that is:
- a bare local identifier of the caller (not a parameter, not a global), and
- non-escaping in that caller (not global-reachable, not part of the returned value).

Then the caller's region owns the storage and reclaims it at frame exit. If any
call site is unknown or unsafe, the barrier stays. Escape-derived barriers are
never overridden (`publish_only ∩ escape_barriers = ∅` guard in `analyze`).

Implementation: `ReclamationPlan.{escape_barriers,publish_only,publish_sites}`;
`Graph.{publish_receiver_param,publish_call_args}`; `Graph::plan` split,
`publish_safety`, `site_is_safe`; `analyze` global safe-set removal;
`record_publish_call` in the walk. Additive — the existing barrier classification
inside `plan()` is unchanged, so `classify` semantics are preserved.

## Verification
- `tests/publish_reclamation.rs` — repro Valgrind-clean.
- `reclamation_runtime::native_aliases_borrowed_payloads_and_nested_returns_are_memory_safe`
  — previously RED under Valgrind, now green.
- 39 `analysis::reclamation` unit tests green (incl. two new: local receiver
  reclaimed; parameter receiver stays a barrier).
- Non-recursive variants (with extra parameters) clean.

## Residual / follow-ups
- **Recursive producer** variant still leaks; the publisher barrier is correctly
  dropped there, so it is a separate reclamation defect. Logged in `BUGS.md`.
- `reclamation_runtime::region_records_follow_explicit_free_growth_and_suspension`
  fails on `realloc() with size 0` — pre-existing and independent of the plan
  (the test links `reclamation.ll` + a C harness directly).
