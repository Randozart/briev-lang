---
schema: "codex-work-plan/v1"
created_at: "2026-11-15T10:30:13+01:00"
timezone: "Europe/Amsterdam"
status: "completed"
baseline_commit: "d5f3ee1df9bde718c335548cc203266588968a75"
supersedes: []
superseded_by: []
related: ["docs/plans/2026-11-15-092856-boolean-operator-proof-boundary.md", "docs/plans/2026-11-15-080204-typed-verification-and-lean-export.md"]
---

# lean-model-correspondence

## Purpose

Check the restricted exporter against explicit assignment semantics and discharge contracts through verified weakest preconditions

## Approach and alternatives

Add finite assignment execution and a proved WP soundness theorem to IntegerVC. Independently serialize supported Briev expressions/statements to the model alongside the existing direct Lean emitter. Require model VC, direct/model equivalence and derived contract theorem reports; retain source/native and AST-serialization refinement gaps.

- Only add more simp examples to the direct emitter: rejected because it would leave the exported program disconnected from assignment semantics
- Claim full Rust/source/native refinement from agreement of two generated views: rejected; shared serialization bugs remain possible and the compiler boundary needs separate proof evidence

## Scope and documentation

Lean assignment semantics/proofs, restricted exporter, proof checker and rejection controls, assurance architecture documentation; reconcile existing main commits without touching user edits

## Verification

- Model positive/false controls; real exported contracts and deliberately divergent operator/slot controls; current typed/unsupported regressions; root build, quick and full library; package debug/release/Z3; four Kani kernels; Praetor baseline; rustfmt/Ruff/shellcheck/links/secrets; full IR determinism and installed-clang matrix with explicit incomplete results

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-15T10:30:13+01:00 — Plan created before implementation.

### 2026-11-15T11:23:00+01:00 — Implementation decisions and focused evidence

Reconciled main through 8b69cbe2c (parity golden correction and advisory counsel
census/contracts). The parent plan baseline was d5f3ee1df; current verification
uses the reconciled tip as baseline. Preserve any later main changes separately.

The model now supports literal Boolean propositions, negation/disjunction and
finite assignment execution. Lean checks eleven named model theorems with only
propext, including unbounded sequence WP correctness and VC soundness. Three
false model theorems are rejected, including stale sequential assignment.

Export schema v2 contains three obligations: model VC, direct/model program and
pre/post correspondence (post quantified over every result), and the contract
transported from sequence_vc_sound. The transport uses all correspondence fields;
model post-state projection retains immutable parameters and slot-zero result.
The direct emitter uses fresh names per assignment; independent serialization
reuses mutable local slots. Rust AST serialization and source/native refinement
remain explicitly unproved; both emitters could share a systematic error.

Initial unrestricted simplification introduced Quot.sound in the bridge. The
axiom gate rejected it. Program equivalence now uses definitional equality;
only named logical/order/index rules are used for remaining transport. The
propext-only policy is unchanged. A raw Lean template preserves bullet indentation.
Boolean truth evaluation and its generic proof handle both constructors explicitly.

Focused actual-export checks passed eight contracts (including copying, mutation,
negation, signed relations, Boolean connectives and multiplication), rejected two
false contracts and seven drift/evidence controls. Added explicit missing-VC,
missing-correspondence and legacy-schema controls before full validation. These
are proof-mechanism controls, not full Rust serializer refinement proofs. Existing
four Kani kernels remain regression gates; the new model semantics are proved
in Lean and each exported correspondence is checked without a program-length bound.

Affected contracts/docs: v1 bundles must be regenerated; v2 checked.json records
all three theorem names and a dependency map. The optional source API remains
restricted to the existing typechecked AST subset; no source syntax/native
backend extension or proof-based optimization consumer is added. The Lean checker
is the named consumer of the new proof families.

Final commands and evidence: root build/quick/full library, package debug/release
and real Z3, four Kani kernels, Lean models/export gate, baseline-aware Praetor,
Rustfmt/Ruff/Shellcheck/whitespace/links/secrets, full IR determinism and installed
clang matrix. Logs and direct file/literal-link source manifests live outside the
repository at /home/randozart/verification-evidence/briev-lean-assurance/model-correspondence/.
Preserve standard snapshot failure on test-project-otto and dataset-dependent
matrix gaps as incomplete outcomes. Complete notes before committing/final freeze.

Falsification: any independently corrupted program/specification/slot accepted,
a missing VC/correspondence report accepted, a disallowed axiom accepted, or
serialization depending on HashMap iteration defeats the intended boundary.

- 2026-11-15T11:40:24+01:00 — Reconciled main through 715927711 (in-process SMT result cache), following 6c390d8f4 (central memcpy emission). Focused exports passed eight contracts and rejected two false contracts plus ten evidence/correspondence controls; eleven model theorems and three false-model controls passed. Broad precommit gates rerun on the combined source; final evidence will be collected after the authorized commit on a frozen HEAD. Existing Praetor findings, counsel coverage gaps, test-project-otto snapshot failure and dataset-dependent matrix incompleteness remain explicit. No new compiler bug is claimed from proof-template development.

- 2026-11-15T11:53:22+01:00 — Implementation complete on reconciled baseline 715927711. Precommit build and quick gate passed (3801 library plus 53 integration tests); full library passed 3802/3802. Debug/release prover suites including real Z3, four Kani harnesses, eleven Lean model theorems/three false controls, eight exports/two false contracts/ten correspondence controls, static checks and full ten-case IR determinism passed. Praetor has zero new blockers against the reconciled baseline; existing blockers persist. Counsel reports zero violations but retains refused/unproven/skipped coverage. Standard runner is unavailable for the tracked external test-project-otto directory symlink; direct content/literal-link snapshots are separate evidence. Installed-clang matrix remains incomplete (missing u.data and empty results glob), with recommender OK using its scripted stand-in. Commit and local-main reconciliation follow; final-root/final-components/final-release and summary.json under the external evidence directory will record fresh committed-HEAD validation and any limitations. Do not interpret these model/correspondence proofs as AST serializer, Rust exporter or source/native refinement.
