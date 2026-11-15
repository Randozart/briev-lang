# Production correctness — adoption and current-main reassessment

**Date:** 2026-11-15. **Branch:** `fix/compiler-correctness`.
**Implementation baseline:** `817f1d6982af8802586f6e7f2ed2aa37ec0ffec0`
(the fetched `origin/main` and local `main` when this work began).

## Scope and method

Adopt the [production correctness strategy](2026-11-14-production-correctness-strategy.md)
from `docs/production-correctness-plan`, commit `31164217`, and reassess its
ordered backlog against this baseline. Preserve that document verbatim as the
audit of `e9169750`; current findings and execution evidence belong here.

1. Compare the audited revision with current source, workflows, and intervening
   commits. For each backlog item distinguish landed pieces from remaining
   acceptance criteria; source presence alone is not an executed pass.
2. Query CI for the exact baseline revision and retain status/run identifiers.
3. Run root library tests, the extracted verifier's tests, the quick gate, and
   `cargo build`. Record actual selected/executed results and failures. Use
   `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1` to bound
   local build resources, with logs under `/tmp/opencode`.
4. Publish a source-linked status table and the next bounded implementation
   increment, with concrete acceptance checks.
5. Update `docs/plans/INDEX.md` to point to the strategy and this reassessment;
   validate Markdown links, whitespace, and changed-directory Praetor before
   committing the documentation increment.

This increment changes documentation only. Existing source rationale comments
and architecture contracts remain the evidence for the reassessment. The next
implementation increment must update the relevant architecture/harness guides
listed in strategy §10 alongside its code. No performance change is proposed;
runtime benchmark before/after measurements apply when an implementation
changes production performance.

## Reassessment record

**Conclusion:** retain the A0 → A1 → A2 ordering. Several production fixes and
small assurance components have landed, but none completes the corresponding
whole backlog item. A0 now starts with gate coverage/accounting and explicit
verifier regressions rather than reinstalling the already-installed CI solver.

Source references below are pinned to `817f1d69`. The comparison with the old
audit spans 475 files, 53,583 insertions and 26,749 deletions
(`git diff --shortstat e9169750..817f1d69`); `0712ce72` is a broad formatting
commit, so that volume is not a measure of semantic progress.

### Landed changes that alter the old assessment

| Area | Current evidence | Consequence for the backlog |
|---|---|---|
| Missing CI solver | PR #96 installs Z3 and checks its version: `.github/workflows/ci.yml:20–24`, `verification-nightly.yml:27–31`, `closed-region-proof.yml:25–29`. `BUGS.md:102–115` records the missing/present solver A/B. | The original missing-Z3 diagnosis is resolved in these workflows. An exact-revision full green gate still needs execution evidence. |
| Solver false proof | `acd31193` replaces substring verdict recognition; `6347e287` adds the production status kernel. `briev-verify/src/solver_response.rs:11–30` selects an exact first nonempty status. | A3 has a concrete repaired defect and reusable negative controls, not just a suspected parser problem. |
| Verifier arithmetic | `89dbaa68`; `briev-verify/tests/arithmetic_boundary.rs:7–45` covers wrapping literal arithmetic and exceptional division in both evaluators. | Reuse these controls in A3/B2. Division-domain, target-width and whole-VC soundness obligations remain. |
| Native arithmetic and Float boundaries | `74311bce`, `8278566c`; `tests/integer_wrapping.rs:21–90` compares seven full-width input pairs at O0/O3 under AST and HIR. `tests/cli_args_test.rs:220` onward adds native Float division checks. Both binaries are selected by the quick gate. | B2 already has an independent full-width arithmetic example. A2 should generalize its observation discipline rather than rebuild it. |
| Interpreter and aggregate ABI | PR #97 repairs Float comparisons; PR #94 repairs aggregate-handle call marshalling. Regressions are identified in `BUGS.md:1–63`. | Retain these fixed counterexamples; the old audit does not describe their present status. Wider interpreter/ABI campaigns are still needed. |
| Portable pointer widths | `da4142d6`; `BUGS.md:65–100` and [CI repair](2026-11-14-ci-repair.md) record pointer-width and browser/WASM repairs. The current `algo_gcd.bv` WASI probe emits verifier-clean IR and reaches the linker (details below). | The old i64/i32 failure is not reproduced by this fixture. Local WASI link prerequisites are missing; runtime parity remains unverified. |
| Formal assurance | `briev-verify/src/solver_response.rs:33–72`, `briev-verify/src/symbolic.rs:350` onward, and `verification/lean/`. [Solver response](../architecture/solver-response.md) records the bounded production proof; [Lean assurance](../architecture/lean-assurance.md) records checked model theorems and their limits. | C1/A3 have useful components. Historical proof results remain source-bound; this reassessment does not rerun them or establish Rust/Lean/native refinement. |

### Current backlog disposition

`Partial` means useful pieces exist but the strategy's full exit criterion is
unmet. `Open` means the cited gap remains in current source. `Queued` preserves
the later milestone without claiming it was executed in this assessment.

| Item | Status | Current evidence and remaining work |
|---|---|---|
| **A0 — trustworthy baseline** | **Partial; next** | Z3 setup is repaired. `scripts/gate.sh:27–36` still selects 12 integration binaries with nextest versus seven in the fallback. Root library commands omit dependency-crate tests. Three solver-boundary regressions are ignored by default; no dedicated required invocation appears in CI. `.github/workflows/ci.yml:44–108` still serializes independent suites in one job. Add explicit selection/outcome accounting and required verifier execution. |
| **A1 — harness controls** | **Partial** | Transition and memory fault hooks exist (`tests/deterministic_transitions.rs:392–396`, `tests/memory_campaign.rs:58–63`), but required workflows do not exercise them. The memory runner can accept missing Valgrind (`:287–290`) or a fixture timeout when zero exit is not required (`:244–279`, `:490–494`). `scripts/reduce.sh:19–25,44–49` still expects a candidate argument; `tests/reduction_scripts.rs:24–65` checks shell syntax and predicates, not a real reducer. Establish discriminating controls and outcome classes. |
| **A2 — strong differential oracle** | **Open** | `tests/deterministic_transitions.rs:363–374` forces `BRIEV_HIR_EMIT=0`; its trace already preserves i64 values. `tests/interpreter_corpus_parity.rs:21–139,217–240` still uses exit-byte/checksum observations, including a permutation-insensitive sort sum. `src/typed_program.rs:561–580` compares the model with the interpreter only. Exercise default HIR and compare full values, ordered collections and effects. |
| **A3 — proof boundary** | **Partial** | Exact status recognition and wrapping regressions landed. `briev-verify/src/smt.rs:61–89` still ignores `_timeout_ms`, ignores stdin write failure and does not interpret process status. Root feature-gated Kani harnesses and the extracted crate need distinct activation/accounting. Complete process/protocol controls, small-domain soundness and consumer checks. |
| **A4 — ownership matrix** | **Partial** | The memory campaign compiles through the default path (`tests/memory_campaign.rs:313–328`) and defines families 0–7 (`:44–54`). Nightly still selects only 0–4, with family 1 non-blocking (`.github/workflows/memory.yml:25–54`). Nested-enum regressions cover both emitters/O0/O3 with optional Valgrind (`tests/nested_enum_allocation.rs:19–70`). Require detectors, complete family results and value/lifetime observations together. |
| **B1 — structured breadth** | **Partial** | Fifteen transition families and replay/shrinking exist, but the generated default is family 0/seed 1/four cases (`tests/deterministic_transitions.rs:444–500`). Typed generation still has no native arm. Preserve existing families and add explicit execution budgets plus validity-preserving shrinking. |
| **B2 — optimization/numerics** | **Partial** | Reuse the new integer-wrapping and Float regressions. `tests/opt_level_parity.rs:89–104,123–142` still compares one lowering while also changing LTO/fast-math. Separate Briev/LLVM/numeric-mode axes; keep independent arithmetic oracles and proof-consumer controls. |
| **B3 — persistent reactor/faults** | **Open** | `tests/deterministic_transitions.rs:328–345` restarts a binary for each event and passes state via environment variables. It is useful function-transition testing, not a persistent reactor/schedule oracle. Add the modeled persistent host boundary after A1/A2. |
| **B4 — cross-target/ABI product** | **Partial** | ABI and browser pointer repairs landed. The local `.bv` WASI probe reaches linking with valid IR but lacks WASI libraries; there is no runtime verdict. The nightly file still lists wasm corpus execution as queued (`.github/workflows/verification-nightly.yml:8–12`). `tests/relocatability.rs:20–49` copies a build-tree compiler with symlinked resources. Provision the target toolchain, then qualify runtime parity, real packages and independent host ABI boundaries. |
| **C1 — assurance depth** | **Partial** | Bounded Kani and Lean model work now exists. `scripts/verify.sh:27–32` still selects neither the feature-gated root proof set nor the extracted crate's proof set explicitly. `.github/workflows/closed-region-proof.yml:3–6` remains branch/manual-only. Preserve theorem domains and proof identity; add required named discovery/execution before broader proof/device claims. |
| **C2 — continuous qualification** | **Queued** | Exact-candidate package/target/soak evidence and recurring discovery ownership remain exit requirements. CI source configuration and historical release checklists cannot substitute for those results. |

### Specific accounting corrections

- The nextest quick selection has **12** integration binaries; the fallback has
  **7**. Missing from the fallback: `unit_test_blocks`,
  `interpreter_corpus_parity`, `hir_reclamation_parity`, `reduction_scripts`,
  and `verify_report`. This was checked directly from both selection branches.
- `briev-verify/tests/solver_boundary.rs:7–88` now has **three** ignored Z3 tests,
  including wrapping-monotonicity rejection. A plain package test pass skips
  them. The non-blocking ignored suite in `ci.yml:81–83` is not a required
  solver-boundary gate. Use the explicit command in the execution record below.
- Source inventory is now 126 top-level integration `.rs` files, 13 compile-fail
  fixtures, 11 directive fixtures, 57 tier1 `.bv` files, six tier1 goldens,
  13 HIR snapshots, five fuzz targets and five workflows. There are 15 root
  source Kani markers and ten extracted-verifier markers. These are inventory
  counts only, not executed-case or proof totals.
- The older release checklist's arithmetic-parity and no-ignored-tests claims
  are historical. Later wrapping fixes and the current ignored solver tests
  require the narrower, source-bound evidence above. Preserve the old record.

## Next implementation increment: A0 gate integrity

**First bounded change:** make the existing quick gate select and account for
the same required suites under nextest and cargo-test fallback, including the
extracted verifier and its explicit Z3 regressions.

1. Centralize the existing quick-suite selection rather than maintaining two
   divergent lists. Keep the conformance-sweep exclusion explicit; the full
   root library run covers it separately.
2. Preflight the required compiler/linker/solver capabilities. A missing tool,
   empty selection or test-runner failure must fail with its own outcome.
   Account for all three ignored solver-boundary tests by explicit selection;
   do not enable every ignored test indiscriminately.
3. Add the smallest versioned result manifest around these existing commands:
   revision and working-tree state, selected package/binary/test identities,
   tool versions, attempted/executed/skipped counts, exit/outcome and log paths.
   Preserve failure evidence. A process exit of zero alone is insufficient.
4. Verify both tool-selection paths against the same declared test identities;
   test missing-tool, failed-child, empty-selection and malformed-result
   controls. Run the actual quick gate and the separate root/verifier suites.
5. Wire the required verifier checks and always-retained result artifacts into
   CI. Independent CI job separation can follow as the next A0 increment once
   the common selection/outcome contract is established.

**Documentation with that implementation:** create
`docs/architecture/compiler-correctness.md` for the implemented gate contract;
update gate/workflow rationale comments and
`docs/guides/deterministic-testing.md` for invocation/evidence. Update
`docs/guides/memory-defect-campaign.md` with A1 detector/control changes when
that runner adopts the contract. Preserve existing rationale, date additions,
and explain what would retire transitional adapters. Keep this reassessment
historical and link subsequent execution plans from INDEX.

**Acceptance:** the fallback and nextest selections agree; the prover's unit
suite and all three explicit Z3 regressions execute; missing capability and
zero execution cannot report success; failures retain their manifest/logs;
required repository gates pass. Then proceed to A1 controls and A2 HIR/full
observations. A3 process handling must preserve the existing completed-UNSAT
plus model-unavailable behavior: a blanket rejection of every nonzero Z3 exit
would break `solver_boundary.rs:28–30`.

## Execution evidence

### Upstream CI observation

At the exact baseline, [CI run 43688163075](https://github.com/Randozart/briev-lang/actions/runs/43688163075)
and [Fuzz run 43688163092](https://github.com/Randozart/briev-lang/actions/runs/43688163092)
were in progress when first queried. A subsequent job-detail observation showed
CI's release build, effect-shape gate, library suite and compile-only target
checks passing, with all-target tests still running and later checks pending.

**Final observation at 2026-11-15 07:25:16 UTC:** the same CI run reports success
through all-target tests, the non-blocking ignored-test step, binary tests,
tier1 fixtures, accel non-hang and browser gates. The contract census is running;
IR determinism, parity corpora and counsel checks are pending. Raw job statuses
are in `ci-observation.json` under the local log directory. This supersedes the
old audit's missing-Z3 library failure for the observed steps; the complete run
is still **IN PROGRESS**, not a recorded full CI pass. Fuzz was not re-queried.

### Local environment and checks

Linux x86-64; Rust/Cargo 1.99.0; clang/LLVM 21.1.8; Z3 4.13.3; nextest 0.9.151;
Valgrind 3.26.0; Kani 0.68.0 installed. C-Vise is unavailable. Proof-tool
availability is not a proof result; no Kani/Lean/device campaign is run by
this documentation increment.

Tests use the compiler sources at `817f1d69`, with the documentation adoption
and this assessment present in the working tree. Logs are retained under
`/tmp/opencode/briev-correctness-reassessment-817f1d69/`.

- `cargo test -p briev-verify --test solver_boundary -- --ignored`: **3 passed,
  0 failed, 0 ignored**; 0.14 s test time, 7.66 s wall time including the build.
  Covers the real-Z3 model/error boundary, the false square-body proof and
  wrapping-monotonicity rejection. Log: `solver-boundary.log`. This independent
  package-only check used
  `CARGO_TARGET_DIR=/tmp/opencode/briev-correctness-verifier-target`, with the
  same three build-resource environment settings as the other checks.
- `praetor validate --warn --target docs/plans`: **passed**. These Markdown
  changes do not exercise source complexity checks or establish semantic proof.
- Imported strategy matches `31164217` byte-for-byte; local Markdown
  links/anchors, table column consistency and `git diff --check HEAD` pass.

| Command | Executed result | Test time / command wall time | Log |
|---|---|---|---|
| `cargo test -p briev-verify` | **PASS:** 27 unit + 2 arithmetic integration tests; 3 solver tests ignored here and explicitly executed above; 0 doc tests | <0.01 s per test binary / 11.44 s | `verifier-tests.log` |
| `cargo test --lib` | **PASS:** 3,788 passed, 0 failed, 0 ignored | 285.73 s / 510.35 s including compilation | `library-tests.log` |
| `bash scripts/gate.sh quick` | **PASS (nextest):** 3,787 library tests, one conformance-sweep exclusion covered by the preceding full library run; 53 integration tests across 12 binaries, 0 selected tests skipped; 117 other binaries outside the selection | 84.845 s library + 84.691 s integration / 268.90 s including compilation | `quick-gate.log` |
| `cargo build` | **PASS** | 62.74 s wall | `build.log` |

The build and verifier checks report the same five existing verifier warnings:
unused `Formatting`/`MatchArm` imports, undeclared `kani`/`kani_full` feature
conditions, and an unreachable pattern. Quick-gate compilation also reports
warnings in untouched integration/binary test files. Compiler/test sources are
identical to the baseline; this documentation increment introduces no new
source warnings. The cargo-test fallback, full local integration suite, release
compiler qualification and hardware campaigns were not run.

### WASI prerequisite probe

Using the freshly built development compiler:

```bash
BRIEV_HOME="$PWD" ./target/debug/brievc build tests/directives/algo_gcd.bv \
  --triple wasm32-unknown-wasi --split \
  --out /tmp/opencode/briev-correctness-reassessment-817f1d69/wasm-gcd
opt -passes=verify -disable-output \
  /tmp/opencode/briev-correctness-reassessment-817f1d69/wasm-gcd/algo_gcd.ll
```

The build exits **1** after 3.63 s at linking; the separate LLVM verifier exits
**0**. The former i64/i32 rejection does not appear in this emitted module.
`wasm-ld-21` reports unavailable `-lm`, `-ldl`, `-lc` and WASI
`libclang_rt.builtins.a`; it also warns that supplied host runtime archives
contain non-Wasm objects. Log: `wasm-gcd.log`; command/status:
`wasm-gcd-result.json`. Classify this as an unavailable local target-link
configuration, not a successful Wasm build or runtime parity result. B4 must
establish the target sysroot/runtime link configuration and then rerun the
corpus. One verifier-clean fixture is not evidence for every pointer-width
consumer.
