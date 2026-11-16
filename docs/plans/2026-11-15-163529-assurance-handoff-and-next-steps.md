---
schema: "codex-work-plan/v1"
created_at: "2026-11-15T16:35:29+01:00"
timezone: "Europe/Amsterdam"
status: "planned"
baseline_commit: "a9917e166e2eeffc19e714bcbb3fe68f79ae574b"
supersedes: []
superseded_by: []
related: ["docs/plans/2026-11-15-144601-interval-and-smt-boundaries.md", "docs/plans/2026-11-15-verification-next-steps.md"]
---

# assurance-handoff-and-next-steps

## Purpose

Close the merged assurance slice and order compiler implementation proofs and optional Briev-to-Lean certificate work

## Approach and alternatives

Confirm branch/main convergence, preserve private work, document the remaining trusted adapters and staged refinement obligations, and commit a linked roadmap

- Expand language/export coverage immediately: deferred until existing production adapters and AST-to-model correspondence have stronger evidence

## Scope and documentation

Documentation only: this dated handoff and a dated correction/link in the existing verification roadmap; no compiler, model, gates or language changes

## Verification

- Required build, quick and full library before commit; offline local links, whitespace and documentation review; freeze committed state and record fresh documentation evidence, with historical proof results still bound to a9917e166

## Baseline and merge decision

Local `main` and `feat/compiler-assurance-lean` both contain `a9917e166`.
The assurance worktree is clean; main has private untracked work that must be
preserved. There is no outstanding compiler merge at this baseline. This handoff
will be a separate documentation commit, fast-forwarded into local main.
No push, worktree deletion, branch deletion or hook installation is included.

The preceding [interval/SMT slice](2026-11-15-144601-interval-and-smt-boundaries.md)
repaired reversed-bound false proofs, strict-endpoint overflow, unsupported SMT
fallbacks, symbol collisions and rebound-field reads. Its exact-commit evidence
records 3,803 library tests, 55 integration tests, 71 prover tests in each profile,
eleven selected production Kani harnesses and four rejected mutants. The existing
Lean gate checks twelve model theorems and four false-theorem controls. Export
controls check eight contracts and reject twelve invalid/false artifacts; finite
execution checks compare 648 Lean/interpreter samples and 2,592 native results.
These counts describe the recorded baseline, not universal compiler coverage.

Standard evidence snapshots still cannot traverse `test-project-otto`; the
separate manifest records regular contents and literal links. Counsel has
refused/unproven/skipped files. The clang matrix lacks `u.data`, skips two rows,
and exercises recommender with its script's stand-in list. Keep these outcomes
explicit. Whole-parser and multiplication Kani attempts also remain incomplete.
Private logs, source manifests and machine paths stay outside the repository.

## Ordered next implementation slices

### 1. Strengthen the production adapters that can authorize a proof

Start with the trust boundaries exposed by the latest failures, rather than
adding new proven program forms. Inspect:

- [fact extraction, reachability and verdicts](../../briev-verify/src/verify.rs):
  require every inferred range to overapproximate feasible entry values, and
  every hard violation to have a feasible path. Retain advisory Unknown results.
- [saved-value conversion](../../briev-verify/src/symbolic/snapshot.rs) and
  [assignment/field state](../../briev-verify/src/symbolic.rs): specify entry atoms,
  current bindings, RHS-before-write evaluation, invalidation and depth limits.
  Keep source object-alias semantics outside the model until explicitly related.
- [sort validation](../../briev-verify/src/smt/encoding.rs) and
  [query/declaration adapter](../../briev-verify/src/smt.rs): check that supported
  formulas retain their meaning, free names are declared consistently, and
  unsupported/ill-sorted syntax cannot acquire a proof. The byte codec proof
  does not establish the complete UTF-8 String encoder or declaration collector.

For each adapter, first state its invariant and supported input domain, then add
properties against the actual production path. Use Kani where its representation
is tractable; retain resource limits and undischarged obligations explicitly.
Generated AST/state sequences and differential tests supplement proofs. Reduce
any find into an API regression and a source/CLI case where source admission
permits it. Include deliberate mirror, rebinding, missing-fact, rank and symbol
corruptions. A copied reference implementation alone is insufficient evidence.

Acceptance: existing genuine violations still fail; valid saved values and
reachable paths retain their proofs; impossible/unsupported paths do not invent
proofs or hard violations. At least one intended corruption must be rejected for
each new obligation. Do not reclassify existing incomplete proofs as discharged.
Document results in [verifier arithmetic](../architecture/verifier-arithmetic.md),
[solver response](../architecture/solver-response.md) and BUGS.md as appropriate.

### 2. Close the Briev-to-Lean serialization gap for the current subset

The [optional exporter](../../src/backend/lean.rs) currently consumes a
**typechecked AST**. Its [model serializer](../../src/backend/lean/model.rs)
and direct emitter produce two Lean views whose agreement is checked per
artifact. Both views can still share a source-mapping mistake. Begin with the
existing pure signed-64, straight-line subset; keep type/operator admission and
exceptional-domain restrictions intact.

Define the source-side evaluation and binding relation independently of emitted
Lean text. Specify and discharge expression evaluation preservation, then
assignment/return state correspondence, followed by contract transport. Cover
input/local slot separation, overwrites, signed comparisons, wrapping arithmetic
and all-result postconditions. Do not treat agreement at the actual return value
alone as agreement of the specification.

The first deliverable is a small reviewed representation and an executable
cross-check on actual serializer output. Follow with an inductive theorem and a
checked connection to the Rust translation, or a per-artifact translation
validator with an explicit trusted boundary. A theorem about a separate model
without that connection does not close the implementation gap. Record any
remaining parser, typechecker and metadata-resolution assumptions.

Acceptance: corrupt a local slot, literal/operator, precondition and postcondition
while preserving the rest of an artifact; each defect must be rejected. Extend
[export controls](../../verification/lean/export_gate.py) and
[execution controls](../../verification/lean/execution_gate.py) accordingly.
Retain the three named reports and approved axiom policy. No broader source
correctness label may appear until its correspondence obligation is checked.

### 3. Relate the accepted subset to typed HIR and native execution

Only after identifying the AST trust boundary, define how admitted bindings,
physical widths and operations correspond to typed HIR and native lowering.
Build on the existing AST/HIR and clang O0/O3 comparisons. State definedness
premises for traps, division and effects before admitting them; total SMT word
operations alone cannot certify native exceptional behavior. A wasm comparison
is a useful independent test track, not an all-target refinement theorem.

Acceptance: preserve the current eight-fixture boundary corpus and its negative
controls, add minimized lowering discrepancies, and document precisely which
all-input relations have been proved versus sampled. Hardware or tool absence
must remain unavailable, never a passing row.

### 4. Grow optional program guarantees after the bridge is credible

Keep certificates as optional backend/sidecar artifacts; no new language syntax
or Lean runtime dependency is required. Add control flow, then calls with checked
summaries and loops with explicit invariant/termination obligations in separate
slices. Treat memory, aliasing, effects and non-word types as new semantic models.
Each extension needs source admission, semantics, correspondence, proof checks,
unsupported-input diagnostics and a negative control before it is called checked.
The model/exporter consumer is the current contract gate; optimization consumers
need a separate explicit proof-consumption contract.

## Profiling and the older roadmap

The older [verification roadmap](2026-11-15-verification-next-steps.md) retains
useful profiling, wasm and fuzz-corpus proposals. Its ordering predates the latest
soundness repairs. Profiling is evidence for performance work, not a prerequisite
for closing a known false-proof route. Do not treat its proposed feature flags
or illustrative commands as implemented interfaces.

Do not adopt its multiplication sign rules for wrapping words: positive
`MAX * 2` wraps to `-2`. Even nonnegative multiplication requires overflow/domain
evidence. A future sign analysis must model modular arithmetic or prove the
required no-overflow premise. Profile solver calls and compile time before
claiming a performance benefit; reject an optimization hypothesis if independent
measurements show no benefit or its abstract facts exclude valid wrapped values.

## Handoff verification and continuation rules

This change edits this plan and adds a dated amendment to the older roadmap;
compiler source, proof models, fixtures, dependencies and gates remain identical
to `a9917e166`. Run the repository-required build, quick gate and full library
before committing. Check local links and both staged/unstaged whitespace. Commit
only these documents, then fast-forward main while comparing private-file hashes.

After committing, freeze the new HEAD/index/content and run fresh documentation
checks plus a tree comparison proving that only these Markdown files changed.
Keep the earlier implementation proof record bound to `a9917e166`; do not relabel
it as a new full proof run for the documentation commit. Final evidence and the
resumption checkpoint live outside the repository. Verify freshness immediately
before reporting. Subsequent implementation slices require new dated plans and
all applicable code/proof/native gates; this handoff does not authorize weaker
checks or a remote publication.

## Decision history

- Created before editing the existing roadmap; no new compiler implementation is
  included. The future work above is planned, not completed or accepted as proof.
