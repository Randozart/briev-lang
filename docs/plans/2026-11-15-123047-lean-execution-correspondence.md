---
schema: "codex-work-plan/v1"
created_at: "2026-11-15T12:30:47+01:00"
timezone: "Europe/Amsterdam"
status: "completed"
baseline_commit: "665cbcc079fef7f88394cf11e8c845c381aa7005"
supersedes: []
superseded_by: []
related: []
---

# lean-execution-correspondence

## Purpose

Find semantic disagreements between certified Lean exports, the real interpreter and native AST/HIR lowering at signed-word boundaries

## Approach and alternatives

Add a reproducible execution corpus, independent Python modular arithmetic oracle, real interpreter sampling helper, kernel-checked concrete Lean evaluations and native O0/O3 comparisons. Keep finite differential evidence distinct from universal refinement.

- No alternative selected yet; record any material tradeoff before implementation.

## Scope and documentation

Identify affected files, contracts and documentation before implementation.

## Verification

- Eight pure functions with 64-bit boundary input vectors; Lean contract and sampled-result checks; interpreter and four native variants; intentionally wrong expected results and altered native program rejected; required root and backend verification, Praetor baseline, freshness and explicit unavailable gates

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-15T12:30:47+01:00 — Plan created before implementation.

## Scope, contracts and tradeoffs

Related: [model correspondence](2026-11-15-103013-lean-model-correspondence.md).
Reconcile local main through ad5e03330 (safe profile and portable syscall change)
before implementation. Keep the current v2 certificate schema and its explicit
AST/exporter/source-native refinement gaps. The new evidence is a finite
differential campaign, never a universal proof or a new guarantee flag.

Add a small example helper that runs the real interpreter on a typechecked
selected function and JSON argument vectors. Add a separate execution gate for
eight supported functions (arithmetic, negation, snapshot locals, mutation,
polynomial and an ordinary alias). Use independently evaluated Python modulo-2^64
results over a fixed boundary corpus, not compiler folding, as the oracle.
For each function certify its contract, kernel-check every sampled Lean result,
compare interpreter values and execute native AST/HIR outputs at O0 and O3.
Reuse existing exporter/checker conventions and keep all generated artifacts
outside the repository. Include deliberately wrong Lean results and a native
body mutation to ensure comparisons can detect disagreement. Record exact
source/tool/binary/artifact hashes and coverage counts.

Alternatives: immediately claiming native preservation from LLVM shape is rejected;
finite executions cannot establish it. A universal Rust extraction/refinement
project remains valuable but is a separate larger obligation. This slice adds a
concrete bug-discovery bridge without expanding the admitted source subset or
changing native generation. If a real discrepancy is found, minimize it, record
the root cause in BUGS.md, fix the responsible production path, and retain a
regression before reconciling the fix into main. No performance claim is made.

Affected docs: update docs/architecture/lean-assurance.md with the gate command,
its concrete consumers and finite coverage/trust limits. Preserve existing
comments and certificate claims. No language, hook or verifier configuration change.
Falsification: any native/interpreter/Lean value differs from the independent
oracle, or either deliberate disagreement is accepted. Tests must report missing
Lean/compiler/Clang as unavailable, not a pass.

Verification baseline: 665cbcc07 was checked with 3802 library and 53 integration
tests, eleven model theorems, four Kani harnesses and eight export contracts.
Re-establish Praetor baseline on reconciled ad5e03330. Required commands include
root build, scripts/gate.sh quick and cargo test --lib before commit, debug/release
prover tests, model/export/execution gates, four Kani harnesses, formatting/static
analysis/links/secrets, full IR determinism and installed-Clang matrix. Finish
implementation notes before freezing committed HEAD and run final source-bound
validation. Preserve known test-project-otto snapshot, counsel coverage and missing
dataset matrix limitations. Evidence directory:
/home/randozart/verification-evidence/briev-lean-assurance/execution-correspondence/.

- 2026-11-15T12:45:41+01:00 — Reconciled main through ad5e03330 before implementation. Focused execution campaign passes eight contracts, 648 axiom-free concrete Lean evaluations, 648 real-interpreter evaluations and 2592 native comparisons across AST/HIR and O0/O3. All eight false Lean result controls are logically refuted; the separately compiled subtraction control is rejected against the unchanged addition oracle at sample 1 (-1 != 1). Missing compiler control exits 2 with an unavailable diagnostic. No new compiler discrepancy was found. Initial fixture diagnostics required explicit argument/result casts for nominal types and the pinned Lean false-proposition wording; these are gate setup corrections, not compiler defects. New helper and execution gate add no guarantee status to the v2 certificates. Static checks and baseline-aware Praetor show zero new blockers. Broad precommit gates are in progress; the initial link-runner path error is fixed and corrected links pass. Final evidence will be frozen after the authorized commit, with unavailable standard snapshot/counsel/dataset coverage retained.

- 2026-11-15T12:48:05+01:00 — Main added only four plan/research documents after the initial reconciliation; fast-forwarded them through 72ade68d1 without compiler/source changes. The code baseline and diagnostic comparison remain unchanged; final validation will include the reconciled committed HEAD. User-owned private .officina/.pi work remains untouched.

- 2026-11-15T12:54:39+01:00 — Implementation complete. Required precommit build/quick/full-library gates passed: 3801 quick library, 53 integration and 3802 full-library tests. Debug/release prover suites, eleven model theorems, four existing Kani kernels, format/static/security checks and baseline-aware Praetor pass with zero new blockers. The corrected offline link check passes (the initial runner referenced a nonexistent old-plan filename). The repeated execution campaign again passed 648 Lean and interpreter samples, 2592 native comparisons, eight false-Lean controls and the alternate-native mismatch control. Existing eight export contracts and twelve false/invalid export controls also pass. The optimized build/determinism/matrix sequence is still running; final-release will record fresh results on the committed HEAD. No production compiler discrepancy or semantics change is claimed. Commit these four intended files and reconcile main without touching private user work. Final commands are the source-bound final-root/final-components/final-release runners already prepared in the external evidence directory; summary.json/status checks will preserve known unavailable outcomes and bind exact source/tool/artifact identities. No in-tree edits are planned after the commit/final freeze.
