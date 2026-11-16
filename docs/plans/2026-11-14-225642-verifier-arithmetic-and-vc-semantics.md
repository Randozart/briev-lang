---
schema: "codex-work-plan/v1"
created_at: "2026-11-14T22:56:42+01:00"
timezone: "Europe/Amsterdam"
status: "active"
baseline_commit: "7bcd1e87bb1a25f72d37b2e723dc7d8f8f88c082"
supersedes: []
superseded_by: []
related: ["docs/plans/2026-11-14-201223-compiler-assurance-and-lean-export.md"]
---

# verifier-arithmetic-and-vc-semantics

## Purpose

Align verifier arithmetic with execution and establish a checked foundation for expression/VC export

## Approach and alternatives

Reproduce overflow crashes; use explicitly wrapping add/sub/mul consistent with interpreter and QF_BV; treat exceptional literal division conservatively; check production helpers and model assignment substitution over BitVec 64

- Mathematical unbounded integers rejected because interpreter and SMT already wrap; checked arithmetic would change language behavior

## Scope and documentation

Symbolic integer arithmetic, boundary regressions, production Kani arithmetic harnesses, restricted Lean expression/substitution model; no claim of whole-compiler correctness

## Verification

- Boundary regressions before/after; package debug/release and explicit Z3 tests; production Kani harnesses and negative control; pinned Lean gate with axiom reports and false theorem control; root build/quick/library; Praetor baseline comparison

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-14T22:56:42+01:00 — Plan created before implementation.

### 2026-11-14T23:04:05+01:00 — Implementation and verification boundary

Baseline is feature 7bcd1e87b / main 6347e287f. Both new public-API boundary
regressions failed on the old source with overflow panics. The production literal
fold now uses wrapping add/sub/mul and checked division. Kani add/sub checks
passed with both overflow covers; exceptional-division checks passed. The combined
wide-multiplication proof timed out at 150 s; the isolated Z3 multiplication
proof timed out at 90 s. Keep the separate multiplication harness explicitly
unverified; do not treat solver budget exhaustion as success. The initial
Z3 attempt also exposed an unavailable PATH binary; the retry used the managed
Z3 directory. No check or assumption was weakened.

The hypothesis that the verifier matches native arithmetic was falsified by the
general LLVM emitter's no-wrap flags. An isolated matching IR pattern optimizes
wrapping-false monotonicity to constant true. This is recorded in BUGS.md and
the new arithmetic contract; backend repair/full Briev reproduction is the next
production work item. Native division and target-width correspondence also remain
open. Current fixes concern the symbolic fold, not these additional obligations.

The restricted Lean BitVec64 expression/assertion model checks three new named
theorems: expression substitution, assertion substitution and assignment-VC
soundness. All nine model theorems report only propext. Both false theorem
controls are rejected. There is no exporter or Rust/native refinement yet.

Affected contracts/docs: BUGS.md, verifier-arithmetic.md, lean-assurance.md.
Final gates: root build, quick gate, full library suite; package debug/release;
explicit real-Z3 regressions; Kani add/sub, exceptional division and restored
unchecked-add negative control; pinned Lean gate and axiom-control mutations;
format/shell/link/secrets checks and Praetor baseline comparison. The standard
evidence runner's tracked external symlink limitation remains explicit.
External logs and source fingerprints: /home/randozart/verification-evidence/
briev-lean-assurance/arithmetic/. Final results belong there, not in this note.

Reconciliation: once checked, commit this phase and fast-forward local main
(including the previously checked Lean model) if ancestry and unrelated dirty
file preservation allow it. No remote push. Preserve the separate worktree.
