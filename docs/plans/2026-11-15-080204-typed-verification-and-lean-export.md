---
schema: "codex-work-plan/v1"
created_at: "2026-11-15T08:02:04+01:00"
timezone: "Europe/Amsterdam"
status: "implemented"
baseline_commit: "817f1d6982af8802586f6e7f2ed2aa37ec0ffec0"
supersedes: []
superseded_by: []
related: ["docs/plans/2026-11-14-201223-compiler-assurance-and-lean-export.md"]
---

# typed-verification-and-lean-export

## Purpose

Prevent integer-model proofs of noninteger programs and begin source-bound restricted Lean exports

## Approach and alternatives

Reproduce typed Float false proofs and exceptional arithmetic behavior before changes; retain declaration/type context, exclude unsupported domains from proofs and summaries, then export only a reviewed typed subset into the checked IntegerVC model

- Extend the existing bitvector encoder to IEEE floats and all widths now: rejected for this step because their rounding/trap semantics need independent models; silently erasing types or assuming contracts would defeat assurance

## Scope and documentation

Type/definedness gates at body verification and summary boundaries; explicit unsupported coverage; restricted pure 64-bit expression export and Lean checking

## Verification

- Before/after false-proof regressions with real Z3 and frontend/native counterexamples; package debug/release and root build/quick/library; Lean positive and false-contract controls; Kani kernels; baseline-aware Praetor; source/tool/artifact freshness

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-15T08:02:04+01:00 — Plan created before implementation.

### 2026-11-15T08:46:00+01:00 — Implementation and design evidence

- Baseline real-Z3 tests reproduced two false Float proofs at `817f1d698`;
  complete logs live outside source at
  `/home/randozart/verification-evidence/briev-lean-assurance/typed-export/`.
- Added resolved type admission before helper summaries and loop extraction.
  Compiler target configuration is shared with native codegen; unsupported
  backends receive no model. Standard operators and fixed-width storage metadata
  must agree with signed 64-bit arithmetic. Globals are conservatively skipped,
  including shadowed names. Wider arithmetic support would require distinct
  models; hardcoded custom type names were rejected.
- Division/remainder accepts only nonzero, non-minus-one literal divisors until
  definedness obligations exist. Generated loop contracts are rechecked.
- The restricted backend translates a selected typechecked AST definition into
  direct Lean word expressions, with a runnable export example and checked
  contract obligation. It does not use the assignment VC theorem as an exporter
  correctness proof. Source/native and exporter refinement remain open.
- Kernel-based simplification and `bv_omega` were selected after inspecting the
  pinned Lean 4.24.0 source and official proof-validation reference. `bv_decide`
  adds `Lean.ofReduceBool`; the existing propext-only axiom policy is preserved.
- Initial end-to-end controls: cancellation checked; false overflow/identity
  contracts rejected by Lean; hash drift, missing report and user axiom rejected.
  Kani checked the production admission kernel for every u64 width (4 checks,
  2 covers). This is not a proof of the compiler's metadata resolver.
- Affected contracts/docs: standalone prover clients now supply resolved source
  types; compiler wrapper retains its resolved universe. `lean-assurance.md`,
  `verifier-arithmetic.md`, constructor intent and `BUGS.md` describe these limits.
- Final commands: root build, quick gate, full library tests; package debug/release
  including ignored Z3 tests; example build and export gate; pinned Lean models;
  admission/solver/arithmetic Kani harnesses; changed-scope Praetor compared with
  baseline; rustfmt/whitespace/link/security checks; quick/full IR determinism and
  installed-clang matrix. Preserve matrix missing-input and standard-runner
  symlink limitations explicitly. Evidence uses regular file hashes and literal
  symlink targets, since the standard runner cannot snapshot `test-project-otto`.
- Falsification: a known unsupported typed declaration marked proven, a false
  contract accepted by the export checker, or target-width drift breaks this
  design. Positive source tests and axiom/hash negative controls discriminate
  actual proof checking from merely generating Lean text.

### 2026-11-15T09:06:00+01:00 — Pre-commit results and final-validation freeze

- Root build passed; quick gate passed 3796 library and 53 integration tests;
  full `cargo test --lib` passed 3797 tests. Package debug/release, including
  real-Z3 regressions, passed. Actual CLI probes prove an Int64 identity and
  explicitly skip Float, Int32 and a wasm32 target; all four builds succeed.
- All four Kani harnesses passed. The nine pinned Lean model theorems and their
  false controls passed; the generated contract passed with only propext, while
  two false source contracts and three invalid-evidence controls were rejected.
- Shared native target configuration preserves codegen behavior. Full IR
  determinism passed all ten cases. Installed-clang matrix: recommender passes;
  dataset-dependent multinode/giantnode rows skip and the script exits 2 on its
  empty results glob. This remains incomplete coverage, not a passing matrix.
- Praetor baseline comparison found no new blocking diagnostics. Existing counts
  package 31→29, analysis 353→353, backend 763→763, root 1974→1974; examples and
  the Python export checker have zero blocking findings. Function length remains
  advisory. Rustfmt, Ruff, whitespace, local links and changed-file secrets pass.
- The standard evidence runner remains unavailable because `test-project-otto`
  is a tracked external directory symlink. Direct regular-file/literal-link
  manifests and complete logs are retained externally. No gate was weakened.
- This plan/result note and checker formatting were completed after the initial
  Rust checks; those pre-commit source fingerprints are historical, not final
  freshness evidence. Commit the completed implementation, reconcile local main,
  then rerun the final commands against the stable committed state. Final logs,
  manifests and the completion summary remain outside reusable source.
- Export scope also rejects parameter mutation, binding shadowing, member-form
  operator behavior, custom protocol variants, range constraints and oversized
  storage metadata. Refinement of the exporter and native compiler is unproved.

### 2026-11-15T09:08:00+01:00 — Final formatting correction

The exhaustive changed-file formatting check found the new backend module
out of alphabetical order. Reorder that declaration without changing behavior;
include both module roots in rustfmt checks and inspect committed whitespace
against the baseline rather than an empty working diff. Stop the first final run,
commit the correction, reconcile main and restart final validation. Earlier
attempts remain historical evidence, not the final source-bound result.
