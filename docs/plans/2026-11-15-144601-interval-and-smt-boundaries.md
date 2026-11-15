---
schema: "codex-work-plan/v1"
created_at: "2026-11-15T14:46:01+01:00"
timezone: "Europe/Amsterdam"
status: "implemented"
baseline_commit: "f489612c0419dbd56344d123e24294a824453c0d"
supersedes: []
superseded_by: []
related: []
---

# interval-and-smt-boundaries

## Purpose

Make range inference sound at all signed-word boundaries and audit production SMT symbol/sort/declaration handling

## Approach and alternatives

Represent empty intervals explicitly, normalize reversed comparisons, prove actual interval kernels with Kani, then repair confirmed SMT boundary defects with production-linked controls

- No alternative selected yet; record any material tradeoff before implementation.

## Scope and documentation

Identify affected files, contracts and documentation before implementation.

## Verification

- Baseline panic and false-proof regressions; arbitrary-i64 production interval proofs and mutants; debug/release and real Z3 tests; source CLI/native controls; existing model/export/execution gates; required build/quick/full tests, baseline-aware static checks and exact-source final validation

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-15T14:46:01+01:00 — Plan created before implementation.

## Scope and sequence

Baseline f489612c0 is clean and reconciled with local main. The debug compiler
panics on a valid guard `x > MAX`; the current range extractor also certifies
`[0 < x][term < 0] { term x; }` by failing to mirror a literal-left comparison.
Keep native behavior and language syntax unchanged. Related:
[snapshot repair](2026-11-15-132215-symbolic-value-snapshots.md).

First introduce a small production signed-i64 interval kernel with explicit
emptiness, intersection, comparison restriction and conclusive classification.
Prove membership preservation and verdict soundness for arbitrary i64 endpoints,
literals and witnesses with Kani; verify mirrored comparisons. Use checked
strict-endpoint updates, not wrapping or saturation that invents valid values.
An interval contradiction licenses only unreachable-path reasoning, not arbitrary
statements about feasible executions. Preserve unknown facts conservatively and
handle infeasibility before reporting a concrete violation. Tests cover reversed
comparisons, MAX/MIN, conjunctions, contradictory ranges, both guard polarities,
valid point facts and surviving violations. Compare debug and release behavior.

Then audit SMT symbol encoding, sort checks and unsupported syntax. Preserve
intentional #Self/#self canonical aliases while avoiding collisions with ordinary
identifiers. Prefer a reserved, injective ASCII hex namespace with a production
byte-codec proof to ad hoc hashword prefixes. Quoted SMT symbols do not cover
arbitrary bar/backslash input; a byte encoding stays within the simple-symbol
alphabet. Primary grammar checked in SMT-LIB 2.7, section 3.1 (2025-07-07):
https://smt-lib.org/papers/smt-lib-reference-v2.7-r2025-07-07.pdf
Use explicit Boolean/Word sorting and fail closed on unsupported forms instead
of equality/true fallbacks. Keep public String query APIs compatible by emitting
a diagnostic-only script on failure; proof consumers must observe Unknown.
Update protocol proof consumers if symbol names change. Replace copied
sanitizer example claims with tests/proofs over production code; keep cache-cap
models explicitly separate. No persistent proof cache or new assumed axioms.

Alternatives: wrapping strict endpoints is unsound; saturating MAX+1 into MAX
would include a value satisfying no strict bound. Refusing all branches is
needlessly conservative. A full executor or typed-HIR rewrite is unnecessary
for these concrete primitive defects. No performance improvement is claimed.
Falsify the repair if any valid interval witness is excluded, a conclusive
verdict is false for a member, an empty branch produces a spurious violation,
unsupported/ill-sorted syntax acquires SAT/UNSAT, or distinct canonical names
collide. Preserve counterexamples and deliberate mutants as negative controls.

## Documentation and verification

Update BUGS.md, verifier-arithmetic.md and solver-response.md proportionately,
including proof bounds, caller invariants and still-open Rust/Lean/native gaps.
Preserve substantive rationale. Source unit/API/CLI regressions must fail on the
baseline. Run real Z3, debug/release prover suites, actual production Kani kernels
and mutants, current twelve-theorem Lean gate and export/execution controls;
required root build, quick and full library before each commit; baseline-aware
Praetor, formatting, links, security checks and full native determinism/matrix
where affected. Retain missing dataset, standard snapshot and Counsel coverage
outcomes. Use existing authorized home build targets and external evidence:
/home/randozart/verification-evidence/briev-lean-assurance/interval-smt/.
Commit/reconcile local main preserving private work, finish notes, then validate
exact frozen committed source with tool/artifact freshness. Do not widen the Lean
backend in this slice or claim whole-executor or whole-string refinement proofs.


## Implementation notes — 2026-11-15

Implemented the production interval kernel, normalized comparisons and feasible
hard violations; split sorted SMT encoding and name encoding from the solver
adapter. Migrated protocol query binders and bodies together. The seven interval
regressions, four real-Z3 SMT controls and three CLI report tests pass in focused
runs; the five interval and one byte-codec Kani obligations are discharged.
The baseline reproduces the reversed-bound false proof, endpoint panic and
unsupported-conditional false UNSAT. Naming collisions demonstrate proof loss,
not a demonstrated false proof. A malformed free-identifier `true` unit fixture
was corrected to a Boolean literal while retaining its violation assertion.
Updated BUGS.md, verifier arithmetic and solver response contracts; retained two
scalar cache-cap models and retired copied sanitizer examples.

Final gates use external run-root.py (build, quick, full library),
run-components.py (debug/release package including ignored controls, focused
protocol solver controls, Lean models, formatting/links/Counsel, baseline-aware
Praetor, eleven production Kani kernels, seven separate scalar models and
negative mutants), and run-release.py (export/execution controls, release build,
full native determinism and dataset-aware clang matrix). Complete logs and source,
tool and artifact manifests remain in the external evidence directory above.
Incoming local main has advanced with structural derivation work; reconcile its
source with this repair before final freeze, retaining its tests and behavior.


### Reconciliation amendment — 2026-11-15T15:39:00+01:00 (Europe/Amsterdam)

Main advanced through 052cf5e03, including HIR fidelity/division guards and
structural field derivation. Reproduced a new field false proof against incoming
052cf5e03: write y.n=1, bind x=y, write x.n=2, then x.n==1 is reported proven.
Repair lexical field reads and invalidate fields in the common assignment API,
including declarations. Keep field postconditions available to the standalone
word AST model via sort inference separate from SMT serialization; source typed
admission remains unchanged. Correct four incoming free-identifier true fixtures
to Boolean literals without weakening assertions. Preserve Unknown's structural
inequality invariant, including pointer-identical trees. Add stale/latest field,
assignment invalidation and mixed-sort controls. These adapters/HashMaps remain
unproved; their tests do not extend the primitive Kani guarantees.

The first quick gate passed 3802 library and 55 integration tests. Its subsequent
full library run was interrupted to reconcile before validating the combined
source. Only this worktree's explicitly listed edits were temporarily stashed;
the applied stash and file-hash record remain in external evidence. No private
main work was stashed. Required build/quick/full will run before the combined
commit; final gates run anew after committing. Include the incoming HIR-to-AST
integration suite and its source baseline in the combined checks.

### Combined implementation — 2026-11-15T15:40:00+01:00

The reconciled debug prover suite passes, including all four incoming structural
derivation tests, four new field snapshot/sort controls and the Unknown identity
regression. The directly tracked field rank checker shares binary signatures with
SMT validation while retaining no field SMT encoding. All common assignments
now invalidate field entries after RHS evaluation. Pending combined precommit and
frozen gates must establish the final results; focused passes are not substitutes.

### Pre-commit results — 2026-11-15T15:59:47+01:00

Required build/quick/full library pass on the reconciled source: 3802 quick
library, 55 integration and 3803 full library tests; the ignored protocol solver
test passes explicitly. All 71 debug/release prover tests, four HIR round-trip
tests, eleven production Kani kernels, four deliberate mutants (three new plus
the saved-identifier control), twelve Lean model theorems/four false controls,
formatting, links, Counsel and secret scan checks have recorded outcomes.
Praetor adds no blockers against incoming 052cf5e03; existing diagnostics and
standard snapshot/Counsel coverage limitations remain explicit. Release build
also passes. These are pre-commit results, not final freshness evidence.

The combined commit preserves incoming [structural derivation](2026-11-15-proof-structural-derivation-how-far.md)
and [native/HIR work](2026-11-15-native-correctness-remaining.md). After committing,
reconcile main without touching private files, freeze tools/dependency locks and
relevant environment, then execute run-final-components.py, run-final-root.py and
run-final-release.py. Final source/review, export/native controls, baseline-aware
static results and unavailable outcomes belong in external summary.json; run
status.py immediately before reporting. No project edits are planned after that
freeze. Whole compiler, field-map, String encoding and source/native refinement
remain open.
