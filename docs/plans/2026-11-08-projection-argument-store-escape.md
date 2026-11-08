# Projection-argument store escape and region reclamation

Date: 2026-11-08. Correctness fix; no performance claim.

## Symptom and reproduction

The scheduled multi-user journey harness crashed. After fixing an unrelated
object-layout overrun, Valgrind on the driver (`target/journey-uaf2/`) showed:

- `ws_accept_key` read a 33-byte block, allocated by `b64_loop` and freed by
  `journey_load_start_upgrade`.
- `journey_transport_send` read a 24-byte `GrowBuf`, allocated by
  `journey_transport_live_send` and freed by `journey_load_poll`.

The emitted IR for `journey_load_start_upgrade` opens and closes a reclamation
region; its teardown frees the WebSocket key that `journey_transport_upgrade`
published into `user.transport`.

## Root cause

Reclamation classifies callables as region / barrier / inherit. A callable that
publishes allocating storage into an incoming parameter's field must be a
barrier. Two gaps let `journey_load_start_upgrade` stay a reclaimable region:

1. **Syntactic allocation check.** `t.key = key` was not seen as publishing
   because `expression_allocates` inspects only the RHS syntax and `key` is an
   identifier bound to a call result.
2. **Projection arguments.** A callee's publish-into-incoming-parameter effect
   was propagated only when the caller passed a *bare* parameter, not a
   projection (`upgrade(w.t, …)`). `journey_load_start_upgrade` forwards
   `user.transport`.

## Fix

- Track allocation provenance across binds and copies (`alloc_bindings`); a
  field store whose RHS is an identifier naming an allocation now publishes.
- In `call`, after the callee is built, propagate its `incoming_store_owners`
  membership to this callable when any argument is rooted at one of its
  parameters, at any projection depth. Over-approximation is safe: a barrier
  only disables region reclamation.

Files: `src/analysis/reclamation/walk.rs`. Regression test:
`analysis::reclamation::projection_argument_propagates_incoming_store_barrier`
(asserts `upgrade`, `start_upgrade`, `poll` are barriers and not regions).

## Verification

- Reclamation unit tests: 31 passed.
- Rebuilt driver on the integration compiler; the upgrade path ran with zero
  Valgrind invalid reads/writes (previously six or more). Evidence:
  `recommender-app/target/journey-uaf3/` (`valgrind.log`, `results.csv`).
- A separate `journey_load_progress_user` trap remains and is not addressed here.

## Upstreaming

`incoming_store_owners` and projection-aware nested-field store recognition are
integration-only commits (`db514236`, `d4dfb3ce`). They must be upstreamed
together with this propagation fix; main alone does not region-ize the caller,
so it does not expose this bug but also lacks the escape machinery. Praetor
changed-file check to run on the fix; determinism/backend gates apply because
`reclamation` feeds codegen regions.

## Documentation

BUGS.md entry added. Reclamation rationale comments carry the 2026-11-08 date and
the trigger. No architecture doc names reclamation internals beyond the code
comments; update `docs/architecture/backend-architecture.md` if the region model
is documented there when upstreamed.
