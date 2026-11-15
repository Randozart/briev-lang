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
| `verifier` | `briev-verify` library, `arithmetic_boundary`, and the three exact `solver_boundary` names below |

Quick integration binaries: `diagnostics`, `hir_snapshots`, `hold_law`,
`publish_reclamation`, `deterministic_transitions`, `unit_test_blocks`,
`interpreter_corpus_parity`, `hir_reclamation_parity`, `reduction_scripts`,
`verify_report`, `integer_wrapping`, `cli_args_test`, `gate_runner`.

Required solver cases:

- `solver_status_is_not_taken_from_model_or_error_text`
- `a_sat_model_named_unsat_cannot_prove_a_false_body_contract`
- `wrapping_addition_cannot_prove_unbounded_monotonicity`

Only the explicitly named solver cases enable ignored execution. Other
ignored cases are excluded with an explicit reason. The conformance sweep
still belongs to the separate full root-library run. Discovery must find
every required binary and named case, with a nonempty selection per binary.
New ordinary tests within these binaries are discovered automatically.

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

CI runs these controls and `scripts/gate.sh verifier --runner nextest` as
required steps, then always uploads `target/gate-results/` as
`verifier-gate-evidence`. Existing full-suite CI commands remain separate.
Independent CI jobs and broader evidence adoption are subsequent A0 work.
