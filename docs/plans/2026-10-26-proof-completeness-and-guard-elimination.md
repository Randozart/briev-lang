# Proof completeness and guard elimination

**Date:** 2026-10-26
**Status:** ACTIVE
**Rule:** an obligation that follows from contracts, loop bounds, constructor
postconditions, or path facts is a compiler completeness bug. A guard exists
only for facts the compiler cannot prove (external/runtime input).

## Goal

Make the proof engine discharge every precondition it can derive, then delete
the corresponding `when !(...) { trap; };` guards from the recommender. Keep
guards only for genuinely dynamic boundaries (database, sockets, user input).

## Baseline

- Recommender carries 612 contract guards; hotspots: `cf_engine` 158,
  `server_model` 91, `active_profile` 88, `cf_prepared` 70.
- `cargo test --all` green; all local recommender gates green except the
  browser catalogue path.

## Phase 0 — guard audit

Add an env-gated diagnostic (`BRIEV_AUDIT_GUARDS=1`) at every guarded
statement: prove the guard's required condition (the inner condition of a
`when !(cond) { trap; }`) against `known_facts` before the guard.

- `Proven` → redundant guard.
- `Refuted` → guard is dead / contradictory.
- `Unproven` → classify by missing fact family.

Deliverable: per-function counts and a categorized list. Do not remove any
guard until its category has a sound proof rule.

## Phase 1 — high-yield proof fixes

1. **Conjunct-normalize fall-through facts.** `fallthrough_facts` must return
   the same conjunction split as `facts_of`; `!(A && B)` produces `A` and `B`,
   not one opaque expression. Add regression tests.
2. **Record call postconditions in elaboration.** `elaborate_stmt` must call
   `record_call_post_facts` for single-name lets, matching `infer_statement`.
   This makes `let v = vec_new(n)` establish `v.len == n` in the proof pass.
3. **Loop range facts for real endpoints.** Extend beyond identifiers/fields:
   pure arithmetic and calls, or lower endpoints into stable synthetic locals.
   Preserve soundness; do not use an endpoint that can change in the body.
4. **Equality/inequality closure.** Add equality substitution and transitive
   bounds so length/offset facts across different atoms compose.
5. **Constructor/field invariants.** Propagate postconditions into struct
   fields (`vec_new`, `ivec_new`, `mat_new`, `pop_build`, CSR offsets).

## Phase 2 — general loop/counter reasoning

Add sound interval analysis for integer locals: constants, `+1`, and bounded
loop ranges, with conservative widening at unknown writes. This replaces
manual guards around accumulating counters (`active_build`,
`active_copy_row`, prediction scans).

## Phase 3 — eliminate guards

For every audit `Proven` guard: delete it, rebuild, rerun all gates. For every
remaining `Unproven` guard: either add the missing general proof rule or keep
the guard with a comment stating the dynamic source. Public dynamic boundaries
prefer `Result` over `trap`.

Acceptance:
- Audit reports zero redundant guards.
- `cargo test --all` green.
- All PG/server/worker/active gates green.
- Browser catalogue gate green (debug on its own lane).
- No runtime behavior/output change.

## Phase 4 — compiler invariants for silent miscompiles

Separate from proof completeness:
- post-layout size ≥ every emitted field end;
- verify emitted IR before invoking clang;
- regression tests for nested `JsonValue` copy and event-name suffixes.

## Explicit non-goals

- Do not weaken contracts.
- Do not turn unprovable dynamic facts into assumptions.
- Do not encode algorithm-specific or type-specific proof tricks.
- Do not treat terminal-sentinel guard ordering as a prover bug; it is app
  logic, though a refuted-on-path warning is acceptable.

## Validation log

- 2026-10-26: plan created.
- 2026-10-26: Phase 0 audit added (`BRIEV_AUDIT_GUARDS=1`). Baseline after
  Phase 1: 612 → 358 trap guards; 254 removed as proven; 354 remain unproven
  (contract/runtime/loop families). Zero redundant.
- 2026-10-26: proof fixes landed: conjunct split on negated guard fall-through;
  callee postconditions in elaboration; constant folding in goals AND facts
  with parameter/local shadow tracking; match-arm fact scoping; transitive
  difference-bound closure.
- 2026-10-26: soundness catch: the first difference-bound implementation had a
  constant-sign inversion and briefly proved `n < 0` from `n == 5`; fixed and
  covered by `difference_bounds_do_not_overprove` / `check_refuted_is_error`.
- 2026-10-26: arithmetic range endpoints added; 612 → 348 guards (264 proven),
  344 unproven remain. App gates: active-scratch, active-profile,
  worker, server, run/accounts/cache/operation-store/prediction-workers PASS
  (prediction recovery needs `PGSCHEMA` in the harness environment).
  Compiler `cargo test --lib` 3351 passed.
- 2026-10-26: remaining 354 unproven guards are contract boundaries of
  internal functions, dynamic DB/result checks, and loop/counter facts not yet
  covered by interval/induction reasoning. Next: loop endpoint capture and
  counter intervals, then contract inference for private defns.
