---
schema: "codex-work-plan/v1"
created_at: "2026-11-19T12:02:52+01:00"
timezone: "Europe/Amsterdam"
status: "planned"
baseline_commit: "a0bb555f1579c9fa63272d218a4f9e15580e7069"
supersedes: []
superseded_by: []
related: ["docs/plans/2026-11-15-163529-assurance-handoff-and-next-steps.md"]
---

# call-contract-fact-soundness

## Purpose

Reject the dead-call and mutated-argument false proofs by keeping contracted-call facts at their evaluation point and on their execution path

## Approach and alternatives

First reproduce both failures with API and CLI regressions, then replace entry-hoisted facts with evaluation-scoped lowering and snapshot arguments at the call; retain modular contract premises and fail closed on unsupported evaluation shapes

- Disable all callee-contract facts: safely conservative but loses useful supported proofs; retain only as an explicit fallback for forms whose evaluation order cannot be modeled safely

## Scope and documentation

Prover-only call lowering, focused source/API regression tests, required gate selection, and proportional verifier/bug documentation; no Lean backend or native lowering changes

## Verification

- Baseline failures and positive controls; debug/release prover suites including ignored solver tests; source CLI reports; production-linked Kani obligations and deliberate mutants where tractable; required build/all-targets/quick/full library, baseline-aware Praetor, formatting/links/security; commit and reconcile local main before exact-source final validation

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-19T12:02:52+01:00 — Plan created before implementation.


## Baseline, contracts and acceptance

Baseline `a0bb555f1` has both reviewed false proofs in the production API and CLI:
an unreachable positive(x) call poisons the entry path; a live call after x=1
still contributes facts about the old x=0. Both callers claim term==999 while
returning 0 and 1 respectively. Callee preconditions hold for every executed call.
Main's existing fuzz/Cargo.lock edit belongs to other work and stays untouched.
Use the previously authorized home build trees and external evidence storage.

Replace global fact collection with statement-local prefixes. A guard's condition
facts execute before that guard; its body's facts remain inside it. A logical
RHS contributes facts only under its short-circuit execution predicate. Existing
Gate snapshot conversion freezes argument values before the containing statement
can overwrite them. Keep per-occurrence atoms and tuple result arity. Use fresh
names against source bindings and reject unrelated callee identifiers explicitly.
Unsupported evaluation forms stay outside the model. This retains the existing
modular assumption that callees meet their declared contracts and source call
preconditions; it does not establish a closed call-graph or native refinement.

Add API regressions and CLI exit/report checks for both false proofs. Include
correct return contracts, nested/conditional guards, short-circuit AND/OR,
self-assignment of call results, later argument overwrites, nested calls and
separate call occurrences. The new API suite must be selected by both quick and
verifier gates. Demonstrate baseline failures before repairing production code.
A repair is falsified by either false caller remaining Proven, by facts leaking
from an unexecuted logical RHS, or by losing the corresponding supported valid
proofs. Preserve existing tuple/handle/Bool regressions; inspect new advisories.

Kani will check the actual lazy-RHS guard constructor for concrete Boolean leaves
across arbitrary Boolean conditions/operators; a swapped guard mutant must fail.
This is a narrow production-linked obligation, not a proof of recursive lowering,
HashMap state, snapshots, callee contracts or whole compiler semantics. Preserve
resource-limited/unavailable proof outcomes rather than replacing them with a
copied model. Tests and source-bound review cover the adapters outside that proof.

Update BUGS.md, verifier-arithmetic.md and the existing contract-fact comments
with path/evaluation timing and proof bounds. Keep historical plan rationale.
Required checks: build, all-target check, recorded quick gate (including the new
suite), full cargo test --lib, debug/release prover with ignored Z3 controls,
CLI regressions, script selection tests, three-case determinism, formatting,
local links, secret scan and baseline-aware Praetor. Native/Lean implementations
are unchanged; bind that comparison and keep their prior proof gaps explicit.
Commit logical progress with ordinary git commit, reconcile main while hashing
its private edits, then freeze the final HEAD/index/content/tools for fresh final
gates and immediate freshness status. Do not publish remotely.


### Baseline and focused results — 2026-11-19

The baseline API suite fails seven of eight tests with false Proven verdicts,
including both requested cases and lazy-RHS/argument-overwrite variants. The
first local-prefix repair rejects both original false proofs and preserves their
correct contracts. Nested/arithmetic counterexamples follow the existing SMT-SAT
advisory policy, so those additional tests require an examined unproven/violated
result; the two original concrete regressions still require hard refutations.
This distinction retains the public API contract rather than widening hard errors.
The new call-fact suite is required by both quick and verifier gate selections.


### Implementation evidence — 2026-11-19

Statement-local prefixes, guarded lazy RHS facts and reserved result atoms are
implemented in a private calls module. The original two API counterexamples are
hard-refuted; their true contracts still prove. All focused prover tests pass.
The production lazy-guard Kani harness passes: zero of 4,072 checks failed (155
unreachable), all three covers satisfied. This is the narrow constructor proof
specified above, not a whole-lowering proof. Praetor has no added blocking
metrics; the existing entry function retains cognitive complexity 34 with a
small advisory line-count change. Final counts, mutations, full compiler checks
and exact-source validation remain to be recorded externally.


### Main reconciliation — 2026-11-19

The core repair passed build, all-targets, quick (including nine API regressions
and 59 integration tests), and all 3,943 library tests. Both debug/release prover
runs pass 96 tests. Three production-lowering mutants (entry hoist, guard leak,
after-write facts) fail the intended behavioral regressions; the inverted lazy
RHS guard fails its Kani assertion. The two standard-library probes retain their
baseline verdict counts and introduce no violations.

Main advanced during the merge from the inspected documentation update to
504f4fceb, which also brings PR #143's glue-boundary fix. The merge is clean and
keeps those changes intact. The first core-only evidence is historical; rerun
build/all-targets/quick/full on the combined source, plus glue_gen_parity and the
new protocols_only_boundary integration, before committing the pending merge.
Freeze and validate the final committed state afterward. Native/Lean changes
remain outside our own repair; inherited glue code is explicitly included in
combined validation. No private main edits were stashed or overwritten.
