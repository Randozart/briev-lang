# A0 — quick-gate selection and execution evidence

**Date:** 2026-11-15. **Base:** `770c3b7a` on `fix/compiler-correctness`.
Executes the first increment in the
[current-main reassessment](2026-11-15-production-correctness-reassessment.md).

## Contract and evidence

The quick gate must select the same Rust test identities with nextest and with
cargo test. It must execute the extracted verifier's unit/arithmetic tests and
the three explicitly named Z3 regressions. Missing required tools, empty test
selection, incomplete/malformed results, child failure and timeout cannot pass.
Every invocation retains a versioned manifest and separate command logs.

Current defects: `scripts/gate.sh` selects 12 integration binaries with nextest
but only seven with cargo; neither quick arm selects `briev-verify`. The three
Z3 integration tests are ignored by default and CI has no required invocation.
The conflict-marker guard treats a Git error as absence of conflicts and uses
a shared temporary file. Its `full` header incorrectly promises both profiles.

The prior reassessment executed 3,788 root library tests, 27 verifier unit tests,
two arithmetic tests, three explicit Z3 tests, and quick's 3,787 + 53 tests on
the unchanged compiler sources. This is the pre-change correctness baseline,
not a performance benchmark. Production code generation is unaffected.

## Implementation

1. Keep `scripts/gate.sh` as the entry point; add a small Python standard-library
   orchestration/reporting layer around the existing Cargo/nextest runners.
   One selection definition supplies both adapters. Add `verifier` mode for CI
   and explicit `--runner auto|nextest|cargo` selection for parity validation.
2. Discover exact test names before execution. Nextest uses its JSON list and
   versioned `libtest-json-plus` 0.1 result interface; cargo uses one libtest
   target per invocation with explicit list/pretty formats. Require discovered
   names, terminal results and suite totals to agree. Ignore no malformed result.
3. Preflight tools and retain revision/dirty state, environment, tool versions,
   selection/exclusions, per-test outcomes, command statuses and logs in unique
   artifact directories. Bound commands and terminate their process groups on
   timeout/interruption. Persist partial results on failure.
4. Add behavioral controls for selection parity, missing capabilities, zero or
   mismatched results, duplicated/truncated output, failed children and timeout.
   Run them from an ordinary Rust integration-test entry in both quick paths.
5. Run `verifier` as a required CI step and always upload its evidence. Keep
   full-suite semantics and existing rationale; correct the profile claim and
   make conflict-marker failures explicit without shared temporary state.

## Protocol experiments

Before implementation, nextest 0.9.151 successfully listed and executed the real
27 verifier unit + two arithmetic tests using JSON list / result version 0.1.
Cargo's stable libtest accepts multiple exact filters and `--include-ignored`;
the three named Z3 tests are discoverable together. The previous root-library
log contains exactly 3,788 distinct terminal test lines matching its summary.
Raw probes: `/tmp/opencode/briev-a0-format-probe/`.

These adapters are explicit compatibility boundaries, not a replacement Rust
test runner. Format changes must fail closed with retained raw logs. Hidden
early returns within individual tests remain an A1 responsibility.

## Verification and documentation

- Behavioral runner controls, including actual child processes for failure and
  timeout; compare real nextest/cargo quick selection and outcomes.
- `cargo test --lib`, `bash scripts/gate.sh quick`, forced cargo quick,
  `cargo build`, changed-directory Praetor and whitespace/shell checks.
- Add `docs/architecture/compiler-correctness.md` for the manifest/selection
  contract; update `docs/guides/deterministic-testing.md`, INDEX and relevant
  workflow/script comments. Preserve dated rationale and document how to retire
  adapters if stable structured Cargo results become available. Record harness
  bugs and fixes in BUGS.md. No language syntax or production pass changes.

## Execution record

Results and command identities will be appended after implementation checks.

### Reporter compatibility finding

The first real nextest verifier run passed 27 unit + two arithmetic cases,
then the gate rejected the solver phase as invalid evidence. nextest 0.9.151
executed the three ignored solver cases successfully but emitted repeated
suite starts, premature zero-pass finishes with `ignored: 3`, and a final
`filtered_out: 18446744073709551613`. Repeating with `--run-ignored only`
instead of `--include-ignored` reproduced it. Original evidence:
`/tmp/opencode/briev-a0-verifier-nextest/20261115T100421-058d648de088/`.

Named suites now execute through Cargo in either runner mode; nextest still
performs its JSON discovery. Suite records name both adapters, and the strict
JSON validator still rejects the defective stream. Retire this compatibility
routing only after a pinned nextest version passes the real ignored-suite
protocol check. Cargo's exact multi-filter execution passes the same three
solver cases without accepting optional ignored cases.

### Initial implementation checks

- `python3 -B scripts/tests/test_gate.py`: **20 passed**, including reporter
  defect rejection, Cargo routing, XDG-managed Z3 discovery, real child
  failure/timeout, and malformed-result controls.
- `bash scripts/gate.sh verifier --runner nextest`: **32 passed**; manifest
  `/tmp/opencode/briev-a0-verifier-nextest/20261115T080911Z-70da435ef7d7/manifest.json`.
- `bash scripts/gate.sh verifier --runner cargo`: **32 passed**; manifest
  `/tmp/opencode/briev-a0-verifier-cargo/20261115T081003Z-4428183fcfd9/manifest.json`.
- Both use `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1`.
  Initial checks: `bash -n scripts/gate.sh`, `rustfmt --check
  tests/gate_runner.rs`, and `praetor validate --warn --target scripts` pass.
- `.gitignore`'s broad `test_*` scratch rule initially hid the Python controls;
  explicitly unignore `scripts/tests/test_*.py` so required CI sources are
  included in the commit.

### Full-run and mixed-suite adapter findings

The first real quick nextest run passed. Cargo then executed all 3,787 selected
library tests but the parser rejected libtest's ` - should panic` display
suffix as part of a test identity. The adapter now removes that formatting
annotation while retaining the actual terminal verdict, with positive and
negative controls. Re-parsing the retained stream validates every discovered
identity and total. The failed attempt remains in
`/tmp/opencode/briev-a0-validation/quick-cargo/20261115T081308Z-46e220282540/`.

A separate dependency-free Rust crate with one ordinary, one expected-panic,
and one ignored test exposed the wider nextest reporter defect: even when
the ignored case stays disabled, the reporter emits a premature finish and
repeated suite start. Raw probe output and the crate are under
`/tmp/opencode/briev-a0-protocol-fixture/`. Executing that discovered inventory
through the gate's Cargo adapter gives **two passes, one excluded**, preserving
the ignored policy. Routing now applies to binaries containing ignored cases;
unaffected binaries retain batched nextest execution. Retirement requires
coherent output with ignored cases both enabled and disabled.

The expanded behavioral controls pass **22/22**. The final full quick runs,
identity/outcome comparison, library suite, builds and IR-determinism gate are
recorded separately under `/tmp/opencode/briev-a0-final-validation/`.

### Required-capability negative control

Invoked the real Python CLI in `verifier --runner cargo` mode with a temporary
PATH containing Git/Cargo/rustc but no Z3, and an empty managed-install root
via `XDG_DATA_HOME`. It exits **1**, records `infrastructure_error` at
`required tool z3`, retains all three declared suite policies, and discovers
or executes zero tests. Evidence:
`/tmp/opencode/briev-a0-missing-solver-control/20261115T082105Z-b9458ebe5be0/manifest.json`.
This is an expected failing invocation, not a verifier pass.

### Final acceptance — PASS

Executed on `770c3b7a` plus this increment, with
`CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1` and
`TMPDIR=/tmp/opencode`. Full logs, command arrays, exit codes and durations:
`/tmp/opencode/briev-a0-final-validation/results.json` and adjacent logs.

| Check | Result | Command wall time |
|---|---|---|
| `bash scripts/gate.sh quick --runner nextest` | **3,873 passed**, 17 suites; one conformance-sweep exclusion | 173.784 s |
| `bash scripts/gate.sh quick --runner cargo` | **3,873 passed**, 17 suites; same exclusion | 260.344 s |
| Compare selected/ignored/excluded identities and every terminal outcome | **Exact equality** across both manifests; no failed, unreported or unattempted selected tests | Included above |
| `cargo test --lib` | **3,788 passed**, including the conformance sweep; zero failures/ignored | 276.002 s |
| `cargo build` | **PASS**, no new warnings | 0.175 s |
| `cargo build --release --bin brievc` | **PASS**, same five baseline verifier warnings | 344.854 s |
| `bash benchmarks/ll_determinism_check.sh --quick` | **PASS**: recommender, multinode, pp-types byte-identical across separate processes | 6.656 s |

The 3,873 quick results comprise 3,787 root library tests, 54 focused
integration tests (including the gate's **22 Python controls** behind one Rust
test), and 32 verifier tests. All three named real-Z3 tests pass in each quick
run. In nextest mode, only `solver_boundary` needs Cargo execution in the
current selection; its discovery remains nextest. The manifest projection
comparison is retained as `runner-parity.json`.

Final quick manifests, relative to that validation directory:

- `quick-nextest/20261115T081717Z-4fcd2fafd9f9/manifest.json`
- `quick-cargo/20261115T082011Z-2fc933fe80f0/manifest.json`

Shell syntax, Rust formatting for the wrapper, Python syntax, changed Markdown
relative links, CI YAML/required-step checks, and `git diff --cached --check`
pass. Praetor passes on `scripts/` and on a directory containing copies of all
changed files with the repository's configuration. Debug/release warnings are
the five previously recorded verifier warnings; the new wrapper adds none.

**Increment complete:** shared selection, required verifier execution,
fail-closed reporting, retained evidence, controls, CI wiring, and current
architecture/guide/bug documentation. Next A0 work is independent CI job
separation; A1 addresses internal harness skips/negative controls, followed by
A2 default-HIR and full-value oracles.
