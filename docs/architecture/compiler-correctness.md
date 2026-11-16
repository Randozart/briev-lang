# Compiler correctness gates and execution evidence

**Implemented 2026-11-15:** A0's first increment. The
[strategy](../plans/2026-11-14-production-correctness-strategy.md),
[current-main reassessment](../plans/2026-11-15-production-correctness-reassessment.md),
and [execution record](../plans/2026-11-15-a0-gate-integrity.md) distinguish
this gate contract from the remaining oracle, proof, and ownership work.

## Selection and runners

`scripts/gate.sh quick` and `scripts/gate.sh verifier` use the Python
standard-library orchestrator `scripts/gate.py`. Cargo/libtest and nextest
remain the Rust test runners. `scripts/gate_selection.py` is the single
selection definition for both adapters:

| Mode | Required selections |
|---|---|
| `quick` | Root library excluding names containing `conformance_sweep`; the 13 integration binaries below; all `verifier` selections |
| `verifier` | `briev-verify` library, `arithmetic_boundary`, the three exact `solver_boundary` names, and six exact `typed_boundary` names below |

Quick integration binaries: `diagnostics`, `hir_snapshots`, `hold_law`,
`publish_reclamation`, `deterministic_transitions`, `unit_test_blocks`,
`interpreter_corpus_parity`, `hir_reclamation_parity`, `reduction_scripts`,
`verify_report`, `integer_wrapping`, `cli_args_test`, `gate_runner`.

Required solver cases:

- `solver_status_is_not_taken_from_model_or_error_text`
- `a_sat_model_named_unsat_cannot_prove_a_false_body_contract`
- `wrapping_addition_cannot_prove_unbounded_monotonicity`

Required typed-boundary cases (integrated from upstream 2026-11-15):

- `floating_increment_is_not_proven_by_integer_bitvectors` (ignored real-Z3 regression)
- `a_float_local_cannot_acquire_integer_cancellation_guarantees`
- `a_floating_helper_cannot_launder_a_word_summary`
- `exceptional_division_is_not_certified_by_total_smt_arithmetic`
- `resolved_models_accept_only_the_supported_word_width`
- `a_global_float_cannot_be_treated_as_a_free_word`

Only these explicitly named solver/typed cases enable ignored execution. Other
ignored cases are excluded with an explicit reason. The conformance sweep
still belongs to the separate full root-library run. Discovery must find
every required binary and named case, with a nonempty selection per binary.
New ordinary tests in the library/arithmetic/quick integration selections are
discovered automatically. The exact solver/typed selections require list
updates for additional required cases; discovery records all other exclusions.

`--runner auto` prefers installed nextest and otherwise chooses Cargo.
`--runner nextest` requires nextest; `--runner cargo` forces the fallback.
Nextest discovery batches compatible targets; Cargo discovery/execution uses
one target per command to preserve unambiguous test identities.

**Reporter compatibility (2026-11-15):** nextest 0.9.151's
`libtest-json-plus` reporter emits repeated suite starts, premature empty
finishes, and unsigned-underflow exclusion totals when explicitly executing
ignored tests. Both `--include-ignored` and `--run-ignored only` reproduce it.
A mixed ordinary/ignored suite also produces malformed lifecycles with the
ignored test disabled. Binaries containing ignored tests, and explicitly
named selections, therefore execute through Cargo even when nextest discovers
them. Unaffected binaries remain batched under nextest. Each manifest records
the actual discovery and execution runners.
Retire this routing after the pinned nextest version produces one coherent
suite lifecycle and correct counts for both enabled and disabled ignored
cases, including the real solver suite. The
malformed-stream regression must continue rejecting the old output.

`full` remains the workspace-wide runner for the current Cargo profile. It
checks conflicts, then runs `cargo nextest run --workspace -j 4` or
`cargo test --workspace`. It does not produce this manifest or explicitly
enable the ignored solver tests; run `verifier` as well. AST opt-out and
release-profile qualification remain distinct invocations.

## Evidence contract: `briev-gate/v1`

Each recorded invocation allocates a unique UTC-time/UUID directory under
`target/gate-results/` (override with `--artifacts DIR`). `manifest.json` is
atomically replaced before/after child commands and on finalization. Numbered
stdout/stderr logs are retained even when discovery, compilation, execution,
or parsing fails. Paths in command records are relative to that directory.

The manifest contains:

- **Context:** schema, mode, requested/effective runner, working directory,
  revision, dirty-file status, host, Python/tool versions, allowlisted build
  and transition-campaign environment knobs, timestamps, command timeout.
  Dirty-file status identifies a modified tree; it is not a source snapshot.
- **Selection:** declared package/target/filter policy before preflight;
  discovered suites keyed by `package/kind/binary`; sorted selected and
  ignored-attribute test names; exclusions (`ignored`, `quick_conformance_exclusion`, or
  `not_explicitly_selected`). A preflight failure has policy but no discovered
  test identities.
- **Commands:** exact argument arrays, labels, start/duration, exit status,
  outcome, stdout/stderr paths. The conflict scan's expected exit code is 1
  (no matches); a Git error cannot masquerade as absence of conflicts.
- **Tests:** `not_run` before a batch is attempted, `unreported` while awaiting
  terminal evidence, then `passed` or `failed`. Suite `evidence_valid` becomes
  true only after result identities and summary totals agree. It describes
  valid reporting, not a passing process. `execution_runner` is null until
  the suite is attempted; `discovery_runner` records the listing adapter.
- **Counts:** selected, excluded, passed, failed, not_run, unreported.
  Attempted = selected − not_run; terminally reported = passed + failed.
  These count Rust test functions, not generated cases, native processes,
  obligations proved, or instructions exercised inside each function.
- **Final outcome:** `pass`, `fail`, `infrastructure_error`, `timeout`, or
  `interrupted`, with `finished_at` and a diagnostic on failure. An unfinished
  manifest (`not_run`, no `finished_at`) never establishes success.

Only all selected tests reporting pass, consistent summaries, and successful
execution commands can produce `pass`. Failed test reports or failed test
processes fail the gate. Missing tools, malformed discovery/results, empty
selection, and incomplete reporting fail closed. A malformed stream can
contain individual pass events; its top-level outcome is still non-pass.

All child commands have a wall-clock limit (default 1,800 seconds per command,
`--timeout-seconds N`). Timeout or interruption kills the command's process
group and retains partial raw logs; unreported tests are not invented as
passes. SIGKILL/power loss can leave an unfinished manifest. Python startup
failure or an unwritable artifact directory exits unsuccessfully without a
manifest; those are bootstrap prerequisites for recording evidence.

## Protocol and capability boundaries

`scripts/gate_protocol.py` requires agreement between discovery identities,
terminal outcomes, and suite totals. Nextest uses JSON discovery plus
`libtest-json-plus` version 0.1; unknown/missing/duplicate events, duplicate
JSON keys, inconsistent counts, or a skipped required test are errors.
Cargo uses stable libtest `--list --format terse` and captured
`--format pretty --color never`, checking terminal identities and its sole
summary. Retire the text adapter when Cargo provides a stable structured
result protocol with equivalent controls.

Required capabilities: Git, Python 3, Cargo, rustc, working Z3; quick also
requires clang, opt, llc, and GNU timeout. Z3 can come from PATH or the managed
`$XDG_DATA_HOME/briev-compiler/bin/z3` installation (default
`$HOME/.local/share/briev-compiler/bin/z3`). Nextest is required only when
selected. The explicit solver tests assert solver availability and exercise
real proof-boundary queries.

A Rust test reporting success does not certify that it avoided internal
early-return skips, used a strong oracle, or exercised default HIR. A1/A2
close those per-harness gaps. This increment accounts for runner-visible
selection and results; Kani/Lean and device proof claims require their own
named execution evidence.

## Controls and CI

`scripts/tests/test_gate.py` exercises both result adapters and real child
processes for missing tools, nonzero exit, timeout, zero-output success,
invalid/truncated/duplicate reports, selection parity, and retained artifacts.
`tests/gate_runner.rs` includes these controls in both quick paths and the
ordinary integration suite. Run them directly with
`python3 -B scripts/tests/test_gate.py`.

`scripts/tests/test_ci.py` adds aggregate-status and compiler-transfer controls.
The Rust wrapper and CI discover both Python control modules:

```bash
python3 -B -m unittest discover -s scripts/tests -p 'test_*.py' -v
```

### Independent CI jobs (2026-11-15)

`.github/workflows/ci.yml` keeps the existing `Default gates` check as a
required-result aggregate. Only actual release consumers depend on the
compiler producer; library/verifier failures cannot suppress sibling checks.

| Required job | Prerequisite | Contents |
|---|---|---|
| `gate-controls` | None | Runner and CI behavioral controls |
| `verifier` | None | Recorded `verifier --runner nextest`, including all named Z3 cases |
| `library` | None | Existing root library nextest command |
| `compiler` | None | Release compiler build and same-run artifact |
| `workspace` | `compiler` | Existing workspace nextest, binary, and tier-1 fixture commands |
| `native` | `compiler` | Five independent matrix jobs: effect shape, target surface, accel non-hang, determinism, parity |
| `browser` | `compiler` | Host transport, heap, compiled collection snapshot |
| `contracts` | None | Existing conformance census with its warn-policy overlay |

The native matrix uses `fail-fast: false`. `quarantine` (requires `compiler`)
and `counsel` are explicitly non-blocking independent jobs and are excluded
from the required aggregate. The census is a required **diagnostic execution**;
its warn overlay does not establish a deny-policy proof guarantee. Workspace
tests continue using Cargo's debug compiler where they select
`CARGO_BIN_EXE_brievc`; the release artifact serves shell consumers and the
existing explicit release-path integration regression.

`gates` has `if: always()` and directly needs every required job ID above.
`scripts/ci_summary.py` accepts only a nonempty result map whose entries are
all `success`. Failure, cancellation, skipping, empty/malformed evidence, and
duplicate JSON fields fail the aggregate. GitHub supplies the native matrix's
combined result. An upstream compiler failure leaves its consumers visibly
skipped and still fails `Default gates`.

The `briev-ci/v1` aggregate (`required-jobs.json`, also in the job summary)
records `revision`, `run_id`, `run_attempt`, `finished_at`, required `jobs`
and their GitHub results, `outcome` (`pass`/`fail`), and `error` when applicable.
These are job verdicts, not executed-test/proof totals. Verifier still carries
the detailed `briev-gate/v1` manifest; the other existing suites retain logs.
Broader per-case accounting remains a separate A0 task.

### Compiler transfer and retained artifacts

Local composite actions centralize tool preparation and restoration. The
compiler artifact is a tarball containing:

- `target/release/brievc` (executable mode preserved)
- `target/compiler-in-briv/libbriev_accel_rt.a`
- `target/compiler-in-briv/libbriev_gpu_rt.a`
- producing revision and SHA-256 checksums

The two archives are runtime link inputs read by `src/compile.rs`; the
executable alone is insufficient. `scripts/ci_compiler_artifact.sh` rejects
missing inputs, a nonexecutable compiler, a different checkout revision, or
checksum disagreement. Consumers restore **after** cache setup, replacing
cached copies with the checked same-run build. This artifact assumes the
matching source checkout and Ubuntu toolchain; it is not release-package
qualification.

The producer returns its exact artifact name through a job output. Names
include the run attempt, preserving prior failure evidence and allowing a
downstream-only rerun to reuse the successful producer's artifact. All log,
counterexample and aggregate uploads use `always()` and attempt-specific
names. Native uploads also include the matrix case to avoid collisions.
Explicit Bash shells preserve failure through `tee` pipelines.

Key artifact prefixes: `verifier-gate-evidence-`, `ci-library-`,
`ci-workspace-`, `ci-native-<case>-`, `ci-browser-`, `ci-contracts-`, and
`ci-required-jobs-`. The existing simulation counterexample uses
`deterministic-transition-failure-<attempt>`. See the
[CI split execution record](../plans/2026-11-15-a0-independent-ci-jobs.md)
for local checks versus GitHub-hosted qualification.
