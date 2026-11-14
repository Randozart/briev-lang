---
schema: "codex-work-plan/v1"
created_at: "2026-11-14T20:12:23+01:00"
timezone: "Europe/Amsterdam"
status: "active"
baseline_commit: "8da7e61d2ecda2efa371a7ac8a7652c699cfc17e"
supersedes: []
superseded_by: []
related: ["docs/plans/2026-11-09-compiler-verification-program.md"]
---

# compiler-assurance-and-lean-export

## Purpose

Establish implementation-linked compiler proofs and discover semantic bugs, then explore Briev-to-Lean exports with independently checked guarantees.

## Approach and alternatives

Create an isolated worktree; audit a small production proof component against explicit machine semantics; reproduce and fix discovered bugs; build checked Lean foundations with negative controls before extending to typed-HIR proof export.

- Aeneas extraction connects Rust more directly but requires a compatibility experiment; handwritten Lean models require explicit refinement evidence.

## Scope and documentation

Compiler assurance first; staged Lean export second. No language syntax changes or whole-compiler correctness claim.

## Verification

- Production regressions, independent semantic boundary checks, kernel-checked Lean proofs when tooling is available, negative controls, required Rust gates, Praetor and source-bound evidence.

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-14T20:12:23+01:00 — Plan created before implementation.

## Worktree and baseline

- Source checkout: `/home/randozart/Desktop/Projects/briev-lang`.
- Isolated checkout: `/home/randozart/Desktop/Projects/briev-lean-assurance`;
  branch `feat/compiler-assurance-lean`, base `8da7e61d2`.
- Original dirty files are excluded, not stashed, reset or copied into the branch.
- Existing assurance: Rust regressions, symbolic/SMT discharge in `briev-verify`,
  bounded Kani storage proofs, HIR checks and native/interpreter differential tests.
- No installed Lean/Lake/Elan found on PATH; GitHub DNS resolution currently fails.
  A proof file is not a checked proof until the pinned Lean invocation succeeds.
- `/scratch` is read-only; the required build location needs user clarification.
  Evidence: `/home/randozart/verification-evidence/briev-lean-assurance/`.
- This is a correctness workstream; no performance improvement is claimed and
  no runtime optimization is planned, so a performance baseline is not applicable.

## Acceptance and proof boundaries

The objective is twofold: strengthen the actual compiler and find missed bugs;
subsequently export Briev programs with explicitly scoped, checked guarantees.
Universal compiler correctness is a long-term program, not a milestone label.
Each result must name source revision, theorem, assumptions, unsupported features,
connection to production code, trusted components, and executed checks.

A well-typed Lean output alone is not a correctness certificate. Source contracts
are obligations, not automatically true axioms. Native-output guarantees require
an explicit preservation theorem or checked translation evidence connecting the
program model to that output. Tests are valuable refinement evidence but are not
universal implementation proofs. No `sorry`, `admit`, or unchecked project axioms
may discharge accepted obligations. Check theorem axiom dependencies explicitly.

## Ordered milestones

1. **Assurance boundary and executable counterexamples (start now).** Audit the
   production solver trust boundary and reproduce misclassified answers using the
   real solver plus deterministic parser cases. Fix confirmed classification bugs
   with focused regressions. Separate observed failures from suspected issues.
2. **First Lean component proof.** Define solver response recognition over explicit
   tokens and prove that only an exact leading `unsat` response can authorize an
   unsatisfiable verdict. Include SAT models containing status words, error text,
   unknown answers and malformed output as negative controls. Connect to Rust
   with the same fixture corpus; evaluate Aeneas extraction of the small pure
   component before treating a handwritten model as an implementation proof.
3. **Semantic core and bug discovery.** Define supported machine arithmetic and
   small-step execution; verify symbolic substitution, path fact snapshots and
   VC generation. Compare interpreter, native execution and symbolic reasoning
   at overflow, signed division/remainder, aliases and branching boundaries.
   Pin every discovered counterexample as a regression and record it in BUGS.md.
   Extend Kani over actual production primitives where feasible; mark bounds.
4. **Compiler preservation.** Inventory and prioritize passes licensing unsafe
   behavior, memory reclamation and typed lowering. Prove one concrete transform
   preserves behavior; add intentional invalid-transform controls. Expand only
   after identifying the actual implementation/refinement link and assumptions.
5. **Briev-to-Lean feasibility.** Select a typed-HIR subset with explicit coverage
   errors; design export of execution definitions, contracts and proof obligations.
   Integer widths, exceptional outcomes, termination and effects must be explicit.
   Export before optimizing so optimization bugs cannot rewrite the specification.
   No language syntax changes; no fake proofs for unsupported constructs.
6. **Checked export slice.** Add an optional Lean target through existing backend
   dispatch/capability conventions. Parse/check a real `.bv` fixture, export a
   complete Lean package, prove a true contract and reject a false one. Retain
   artifact/source hashes and a coverage/assumption manifest. Add loops, calls,
   effects, memory and backend preservation only with separate reviewed evidence.

## Alternatives, tradeoffs and falsification

- Handwritten Lean semantics are tractable but can drift from Rust. Prefer a
  small extracted component if Aeneas supports it; otherwise keep the missing
  refinement theorem explicit and use shared fixtures plus differential checks.
- Direct HIR Lean export avoids depending on generated LLVM, but does not prove
  LLVM/native behavior. Translation validation can connect individual outputs;
  a general pass proof has higher initial cost and reusable scope.
- Whole-compiler extraction immediately is rejected for initial scope: large
  dependency/unsafe/I/O surfaces obscure the first useful trusted component.
- Falsify the first slice if the suspected misclassification cannot be reproduced,
  corrected parsing breaks valid responses, or the Lean model accepts any error,
  unknown or SAT response as a proof. Falsify export feasibility if supported HIR
  semantics cannot be preserved without silent approximation.

## Contracts and documentation affected

Preserve public `SmtResult` and prover APIs. Internal solver classification may
be made conservative on malformed answers. Keep existing substantive comments.
Add architecture documentation for proof boundaries and exact reproduction/check
commands; append confirmed bug entries to BUGS.md. Keep this plan append-only.
No hooks, Praetor configuration, language surface or native backend change is
part of the initial slice.

## Concrete verification

Before/following the first fix: run focused `briev-verify` tests, deterministic
solver-output negative controls and a real Z3 counterexample. Run both development
and release tests for the affected crate. Run required library tests, quick gate,
build and changed-directory Praetor; establish baseline diagnostics before edits.
Use the verified-development evidence runner with external source-bound reviews;
record unavailable gates rather than reporting green. Run Lean with a pinned
version when available and check negative controls fail for the intended reason.
Record pending extraction/Kani/refinement obligations explicitly. No commit until
the repository-required per-commit gates pass. No push or publication requested.

## 2026-11-14 — Initial implementation and verification setup

- Confirmed with actual Z3 4.13.0: SAT model variable `unsat` is misclassified by
  the existing substring logic. Confirmed a rejected assertion followed by UNSAT
  is also accepted. External `counterexamples.json` contains exact queries,
  outputs and exit codes; this is solver reproduction, not a compiled Rust run.
- Replaced both substring classifiers with a shared exact-leading-status parser;
  added four unit tests and an explicit real-Z3 public-API regression. Empty SAT
  model payload behavior is preserved; model extraction was already unimplemented.
- Added a pinned initial Lean model with exact-status theorem statements and
  model/error/unknown controls. It remains unexecuted; no implementation proof
  or whole-compiler proof is claimed. Added architecture limits and BUGS entry.
- First-use doctor identified the absent worktree compiler binary. No Lean toolchain
  found; GitHub DNS lookup failed. `/scratch` writes denied; user was asked about
  a dedicated home build directory, with compilation dependent on that answer.
- Evidence runner planning refuses an existing tracked directory symlink,
  `test-project-otto`, before selecting gates. Preserve that link; do not weaken
  runner integrity checks. Collect direct gate logs and label runner unavailable.
- Next: run the package/real-solver regressions and required compiler gates when
  an authorized build path is available; install/check Lean when accessible;
  add a false-theorem rejection, then extraction/refinement evidence. Commit only
  after required gates pass. Both objectives remain active in the ordered ledger.

## 2026-11-14 — Direct check results before build-path clarification

- New Rust files pass rustfmt; `git diff --check` passes.
- Praetor reports 31 baseline and 31 final findings across the prover source
  directory, with zero new non-comment findings in changed files. This is a
  diagnostic comparison, not a clean whole-project gate or behavioral proof.
- Direct quick/build gates fail on read-only `/scratch`. Initial locked test
  commands also exposed the absent, untracked root Cargo.lock; the quick gate
  resolved cached dependencies offline and generated the local ignored lockfile.
  Setup instructions now explicitly cover lockfile generation.
- No required Rust test gate or Lean proof has passed yet; no commit is made.
  Raw counterexamples, gate logs, Praetor outputs and comparison are retained in
  the external evidence directory. Runner-wide verification remains unavailable
  because of the existing tracked directory symlink.

## 2026-11-14 — Authorized build-path exception and first regressions

The user explicitly authorized a dedicated build directory under
`/home/randozart`; use `/home/randozart/briev-builds/lean-assurance-*` for this
worktree. Keep the general parent `/scratch` rule unchanged.

- The new public-API regression was copied into a private baseline extracted
  from HEAD and failed on the SAT-model assertion. It passes against the fixed
  code with real Z3. All 26 prover unit tests passed in development and release.
- The broad compiler quick gate is building from cached dependencies, using two
  build jobs and disabled debug info/incremental compilation to control disk use.
- Kani 0.67.0 is already installed. Added legitimate prover feature declarations
  for its existing feature-gated harnesses and `cfg(kani)` recognition, plus a
  harness over the actual production parser: any eight-byte ASCII suffix after
  `sat\n` must remain SAT. This is bounded suffix coverage, not a universal parser
  or compiler theorem. Execute a private restored-bug mutant as a negative
  control before accepting the harness. Lean remains separately unverified.

## 2026-11-14 — End-to-end false-proof regression and setup correction

- Added a real body-prover regression for `square(unsat)` with the false contract
  `term == 1` and body `unsat * unsat`. At input zero this is false. Both new
  integration tests fail on the private HEAD baseline (the body is incorrectly
  listed as proven) and pass on the fix. SAT parameter counterexamples remain
  advisory under the existing API; the regression requires they are not proven.
- Initial broad quick gate: 3,785 library tests passed; 33/42 focused integration
  tests passed. The nine failures trace to stdlib discovery with an external
  Cargo target, including a missing verification report after import failure.
  Set the existing supported `BRIEV_STDLIB_PATH` to this worktree's `lib` and
  `BRIEV_HOME` to this worktree; rerun gates without weakening test expectations.
- Initial Kani and restored-bug attempts were interrupted after expensive string
  expansion and produced no proof verdict. A bounded retry avoids Rust's UTF-8
  unwrap error formatter while explicitly asserting successful ASCII decoding;
  the same input domain remains and unwinding assertions remain enabled. Its
  16-iteration bound exceeds the 12-byte input; acceptance still requires all
  unwinding checks to pass. Retry has a 180-second operational timeout.

## 2026-11-14 — Validation handoff

Implementation for this initial slice is finished: shared production response
parser, deterministic unit controls, two real-Z3 regressions (including actual
body-prover false-proof coverage), a bounded Kani harness and an explicitly
unverified pinned Lean model. The broader compiler proof and Lean backend are
still active milestones, not completed features.

Final gate outcomes, source hashes and review notes belong in
`/home/randozart/verification-evidence/briev-lean-assurance/final/`; finish all
in-tree edits before that validation. Keep unavailable/timed-out proof obligations
separate from passing tests. Preserve the standard evidence runner's existing
symlink refusal rather than claiming a runner-wide pass. Do not commit with
unresolved required gates. The next independent step is a tractable checked Lean
component/extraction experiment and the false-theorem negative control, followed
by VC semantics and typed-HIR export feasibility as ordered above.

## 2026-11-14 — Final review amendment

Removed the newly proposed Cargo `kani`/`kani_full` feature declarations before
acceptance: `--all-features` would activate older dormant harnesses guarded only
by features, whose `kani::proof` attributes are unavailable under ordinary rustc.
The new harness needs only `cfg(kani)` recognition, which remains declared.
Existing unsupported-feature warnings are preserved as baseline rather than
silenced. This avoids introducing an all-features build regression. It changes
final inputs, so refresh the focused tests, broad gates and external source
manifest for the amended state. Kani's bounded retry reached its 180-second
operational limit with no proof verdict; Lean remains unavailable.

## 2026-11-14T20:54:40+01:00 — Commit and main reconciliation policy

The user explicitly requests continuous commits of all progress and reconciliation
with main so confirmed fixes benefit main. This supersedes the earlier decision
to leave all work uncommitted pending formal tools. Checkpointing a pending proof
is authorized; it does not mark that proof accepted or waive verification.

Split the solver correctness fix, regression tests and its own architecture/bug
record from the Lean research checkpoint. Keep Lean's unavailable status and
Kani's incomplete bounded run explicit. Commit both logical pieces on
`feat/compiler-assurance-lean`, then merge current local main into that branch.
Backport the isolated solver-fix commit to local main, preserving unrelated
uncommitted work and main's new commits. Do not push or publish remotely.

At investigation, main is `5de99c4cd` (three commits beyond this worktree's base).
The original checkout has an unrelated `fuzz/Cargo.lock` modification and private
untracked files, including the initial short copy of this plan. Do not stage,
stash, remove, overwrite or attribute those to the solver fix. Keep the expanded
plan on the research branch; the main fix must have self-contained documentation.

Use historical validation as historical evidence only after Git changes. Run
fresh affected-crate/Z3 tests, build, quick and library gates for reconciled
sources, compare Praetor to baseline, and verify source identity after final
commits. Preserve the standard runner's tracked-directory-symlink limitation.
All logs remain in the external evidence directory. Future confirmed bug fixes
should follow the same isolated-commit/backport/reconcile pattern.

## 2026-11-14T20:59:07+01:00 — Progress committed and local main receives solver fix

- `acd31193c`: isolated solver fix, real-Z3 regressions, bounded Kani harness
  (still unaccepted), and self-contained solver-response architecture/bug record.
- `884941768`: research checkpoint: this plan, the Lean assurance boundary and
  pinned initial Lean model. Formal status remains unavailable/incomplete.
- Local main merged only the solver fix as `6144c8635`, preserving main's newer
  Float-division and counsel changes. No remote push was requested or performed.
- During integration, hundreds of unrelated unstaged edits appeared in the
  original main checkout. Only the reviewed seven-file merge index was committed;
  unrelated edits and private untracked files were preserved. Do not validate or
  edit that live dirty checkout as if it were an isolated source snapshot.
- Validate committed main in a detached worktree at
  `/home/randozart/Desktop/Projects/briev-assurance-main-validation`. The research
  branch now merges `6144c8635`; source-level proof status is unchanged. Historical
  test evidence remains historical until fresh checks on the integrated commit.
- New reconciliation logs and source/commit identities belong under the private
  external evidence directory's `reconciliation/` subdirectory. Future verified
  bug fixes should remain separate commits so main can receive them without
  importing pending Lean research. Continuous checkpoint commits are authorized.

## 2026-11-14T21:06:37+01:00 — Reconcile concurrent formatting commit

Main advanced to `0712ce724` while the first integrated-main checks were running.
That commit also added the initial short copy of this plan from the original
checkout, causing an add/add conflict. Compared both versions: after normalizing
the lifecycle status, main's entire short plan is an exact prefix of this expanded
plan. Retain the full decision history and active status here; no distinct main
rationale is discarded. The source formatting changes are retained from main.

Stop the older `6144c8635` broad check before claiming it covers the new main tip.
Update the clean detached validation worktree to the new tip and run fresh gates.
The isolated solver fix remains in main's ancestry; pending formal proofs remain
pending. Keep local reconciliation separate from remote publication.
