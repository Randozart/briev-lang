---
schema: "codex-work-plan/v1"
created_at: "2026-11-15T13:22:15+01:00"
timezone: "Europe/Amsterdam"
status: "completed"
baseline_commit: "7f3f05ccb396b431c0857ed6703affc89d2ecf2f"
supersedes: []
superseded_by: []
related: []
---

# symbolic-value-snapshots

## Purpose

Repair false body proofs caused by reinterpreting saved symbolic values through later mutable bindings

## Approach and alternatives

Treat symbolic values as eager immutable snapshots, substitute current bindings only at expression leaves, freeze guards when recorded, and verify production regressions plus implementation-linked proof controls

- No alternative selected yet; record any material tradeoff before implementation.

## Scope and documentation

Identify affected files, contracts and documentation before implementation.

## Verification

- Baseline false-proof regressions, interpreter/native witness, real Z3, helper and guard snapshots, bounded production Kani where tractable, Lean snapshot theorem/control, required build/quick/full tests and source-bound final gates

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-15T13:22:15+01:00 — Plan created before implementation.

## Scope and falsification

Baseline 7f3f05ccb is clean and local main contains it. The current CLI reports
`f` proven for `let y = x; x = 0; term y` with post `term == 0`; an input of 7
returns 7. Inspect the same mutation boundary in helper summaries and guards.
Related: [execution bridge](2026-11-15-123047-lean-execution-correspondence.md).

`eval_symbolic` already eagerly copies symbolic expressions at assignment. The
proposed repair preserves that invariant: identifiers inside copied values refer
to entry atoms, while AST identifiers being evaluated refer to current bindings.
Re-following saved identifiers through the final mutable map is rejected. Freeze
path conditions at recording time; omit a fact conservatively if its value cannot
be represented. Retain bounded conversion failure instead of guessing a free
variable. Update helper-summary consumers and existing rationale together.

Alternatives: refusing all mutation would avoid the bug but abandon supported
programs; a full SSA/arena rewrite would add unnecessary machinery if eager
snapshots suffice. This is a correctness repair with no performance claim.
Falsify it if a saved value changes after overwrite, a guard refers to its entry
source name instead of its current value, a false contract is proven, or valid
accumulation/helper proofs regress without a soundness explanation.

Add public-API regressions before implementation, verify they fail on baseline,
and preserve actual interpreter/native counterexamples. Add an implementation
Kani obligation for the repaired production conversion where tractable and
record any timeout/unavailable outcome without substituting a model proof. Add a
Lean saved-slot theorem and false stale-value control; this does not prove the
Rust HashMap adapter or whole symbolic executor. Update BUGS.md and the canonical
verifier-arithmetic / Lean-assurance docs with the exact trust boundary.

Required verification: focused real-Z3 package debug/release and compiler CLI
regressions; root build, quick and full library before commit; current Lean and
execution gates; existing four Kani kernels plus new obligation/control; Praetor
baseline comparison, Rust/Python/shell formatting, links and secrets; full IR
determinism and installed-Clang matrix with known missing-data outcome retained.
Use the authorized home build targets. Evidence lives outside the repository at
/home/randozart/verification-evidence/briev-lean-assurance/state-snapshots/.
Finish notes before commit/final freeze, reconcile local main preserving private
work, and collect exact committed-head evidence. Do not weaken existing gates.

- 2026-11-15T13:42:27+01:00 — Confirmed two false-proof mechanisms on baseline 7f3f05ccb: saved symbolic values were followed through final mutable assignments, and guard facts retained entry source names after writes. The interpreter and four native AST/HIR O0/O3 variants return 7 while baseline reports term == 0 proven. The final five-test package corpus fails four tests on an extracted baseline and passes after the repair; compiler CLI regressions also pass. Current-expression leaves alone consult bindings; converted values retain entry atoms, unknown guard facts are dropped conservatively, and guards decided by current-state reasoning keep only feasible branches. This also removes a baseline false rejection in the positive known-guard control. Excessive recursive saved values fail closed. Lean checks twelve model theorems and four false controls, including arbitrary-state copy preservation at distinct slots. Kani now proves the actual production leaf kernel for every i64 literal, the fixed input atom and Unknown; all 272 checks have zero failures and four covers are satisfied. Recursive conversion/HashMap state reasoning/destructors are excluded from that obligation. Broad recursive-AST attempts and initial cleanup-heavy leaf attempts did not finish; retained logs are incomplete, not proofs. The harness uses ManuallyDrop for its proof-owned input/output to avoid analyzing unrelated recursive enum destructors; production code is unchanged by that setup. A private mutant replacing the saved identifier by zero fails exactly the intended assertion. No claims of universal symbolic-executor, Rust-to-Lean or native refinement are made. Full compiler and combined export/execution gates are in progress, with external evidence under state-snapshots.

- 2026-11-15T13:52:14+01:00 — Implementation complete. Precommit compiler build and quick gate pass (3801 selected library tests and 54 integration tests); full library passes 3802/3802. Package debug/release passes 33 unit tests plus all integration suites including the five snapshot regressions. Five production Kani harnesses pass, including the leaf proof with four cover witnesses; the exact corruption mutant fails its intended assertion. Lean checks twelve model theorems and rejects four false controls under the unchanged axiom policy. Existing export contracts/negative controls and all 648 Lean/interpreter plus 2592 native execution comparisons pass. Praetor adds zero blockers; format, shell/Python/static, offline links and secrets checks pass. The optimized build/determinism/matrix sequence is still running and will be rerun on final committed source. Commit all twelve intended files, reconcile local main preserving private work, and freeze before final components/root/release validation. Final components also execute a fresh exact converter mutant; final summary.json and status.py outside the repository bind source/tool/artifact identities. Preserve known standard-runner, Counsel and missing-dataset coverage gaps and all earlier incomplete proof attempts. No further in-tree edits are planned after this commit.
