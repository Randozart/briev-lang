# A0 — independent required CI jobs

**Date:** 2026-11-15. **Base:** `2b64e8c8`, `fix/compiler-correctness`.
Continues [gate integrity](2026-11-15-a0-gate-integrity.md) and implements the
failure-isolation contract in the [strategy](2026-11-14-production-correctness-strategy.md) §6.1.

## Contract and investigation

An unrelated failed library test must not prevent verifier, native, or browser
evidence. Required checks still fail as a group when any required job fails,
is cancelled, or does not run. Preserve the `Default gates` check name for
existing branch protection. Optional quarantined/Counsel work stays visible
in separate jobs without becoming a required guarantee.

At this base, `.github/workflows/ci.yml` serializes all checks in one job.
Actual dependencies:

- Library tests and the contract census compile their own test binary. The
  census selects the conformance test (`scripts/contract_census.sh:19–33`);
  it uses a **warn-policy overlay**, despite the workflow's old “deny” label.
- Verifier checks only need Rust, nextest, and Z3; gate controls need Python/Git
  and ordinary POSIX tools.
- Most integration tests use Cargo's debug `CARGO_BIN_EXE_brievc`, but
  `tests/enum_list_lifetime_regression.rs:18` explicitly requires the release
  compiler. Workspace/quarantine jobs must receive that build.
- Native/browser shell checks invoke `target/release/brievc`.
  `src/compile.rs:1324–1326,2702–2706,2776–2780` also reads the two runtime
  archives in `target/compiler-in-briv/`; transferring only the executable
  would lose a real dependency. `build.rs:142,200` produces those archives.

The completed previous increment is the local baseline: both quick runners
3,873 passed / 17 suites, root library 3,788 passed, 22 gate controls, builds
and quick IR determinism passed. This change reorganizes CI execution and
evidence; it makes no compiler-throughput claim.

## Implementation

1. Independent required jobs: gate controls, verifier, library, release build,
   workspace tests, native matrix (effect shape, targets, accel non-hang,
   determinism, parity), browser, and contract census. Only consumers of the
   release artifact depend on its producer. Matrix fail-fast is disabled.
2. Centralize repeated Rust/tool/cache preparation and artifact restoration
   in local composite actions. Keep the current Ubuntu image, Rust channel,
   and pinned nextest version. Preserve all existing required commands and
   rationale comments when moving them.
3. Share a tar archive containing the release compiler and both runtime
   archives, preserving executable mode. Check the producing revision and
   file hashes when restoring; restore after cache actions. This is a build
   dependency for same-run jobs, not a distributable release package.
4. Keep `gates` / `Default gates` as an `always()` aggregate over all required
   jobs. A small standard-library Python checker accepts only a nonempty map
   of successful required results; failure/cancellation/skips/malformed input
   fail. Retain a versioned aggregate JSON artifact and job summary.
5. Retain per-job logs and existing verifier/simulation/Counsel artifacts with
   `always()`. Make shell pipelines preserve child failure. Independent
   quarantined/Counsel jobs retain their explicitly non-blocking policy.

## Verification and documentation

- Behavioral controls for aggregate success vs failure/cancellation/skips,
  empty/malformed input, retained failure summaries, compiler artifact mode,
  wrong revision, corruption, and missing build inputs. Include controls in
  the ordinary `gate_runner` integration entry.
- Actionlint and YAML parsing; inspect the job dependency graph, aggregate
  required-job coverage, matrix fail-fast, optional-job policy, artifact names,
  and command preservation against the base workflow.
- Exercise the real compiler artifact in an isolated source checkout, running
  native/browser consumers with restored runtime archives; run the moved
  commands locally where prerequisites exist. Record local results separately
  from GitHub-hosted execution (which needs the branch to be published).
- `cargo test --lib`, recorded quick gate, `cargo build`, quick IR determinism,
  changed-file Praetor, shell/Python syntax, and whitespace/link checks before
  committing. Use the previous resource-bounded build environment.
- Update `docs/architecture/compiler-correctness.md` with the job graph,
  aggregation/artifact contract and limits; update the testing guide and INDEX;
  log the serial suppression and incorrect census label in BUGS.md. Preserve
  prior dated plans and append execution results here.

## Execution record

Results will be appended as the implementation is validated.

### Validation-driven fixture correction

The isolated artifact consumer passed target, effect-shape, accel and
determinism gates, then `float_cases` trapped in the parity corpus. The original
compiler in the primary checkout reproduces SIGILL (`-4`), so this is not a
transfer regression. The emitted IR traps at each user-level `/ 0.0`, matching
the intentional 2026-11-14 behavior pinned by `tests/cli_args_test.rs:220–310`.
Both `float_cases.bv` and `float_fmt_cases.bv` still used zero division merely
to supply infinity/NaN to their formatting oracles.

Correct those two fixtures to construct the same exceptional values using
the existing float32 overflow literal and `Sqrt#(-1.0)`, whose signature and
interpreter implementation already exist. The four-value native probe emits
exactly `inf`, `-inf`, `nan`, `nan`. Keep all existing goldens and contracts;
retain the separate division-trap tests. Re-run the complete six-corpus parity
command and compare both 39-line formatting outputs to their existing goldens.
Original failure/probes: `/tmp/opencode/briev-ci-split-validation/`.

### Infrastructure and transfer controls

- **31 Python controls pass:** the 22 runner controls plus nine CI aggregation
  and compiler-transfer controls. Failure/cancellation/skips, malformed/empty
  status maps, duplicate fields, missing archives, lost executable mode,
  corruption, wrong revisions and absent checkout revision all fail closed.
- Actionlint **1.7.12** passes the workflow (download checked against its
  published SHA-256 manifest). PyYAML parses the workflow/composites. A graph
  audit confirms every non-optional job belongs to `Default gates.needs`, only
  release consumers depend on `compiler`, native fail-fast is disabled, all
  jobs have always-run evidence uploads, and required steps do not tolerate
  errors. Existing native/test commands are preserved; census output moves to
  its evidence directory and its label now reflects the warn overlay.
- The actual aggregate CLI accepts an all-success needs map and rejects each
  of the eight required job failures in turn. Compiler failure plus skipped
  consumers also fails. A Bash pipeline control confirms `tee` cannot mask
  command failure. Raw controls: `aggregate-controls/` under the log directory.
- Artifact names include attempts; consumers use the producer's explicit
  output name, so a downstream-only rerun can reuse the producer's successful
  earlier-attempt artifact while preserving failed logs.

### Isolated compiler consumer

A clean local clone of `2b64e8c8` received only the packed release executable
and two runtime archives, plus the new dispatch/restore helpers. It used a
separate resource root and initially empty HOME/cache. No Cargo build was run
there. This reproduces the CI producer/consumer boundary without relying on
the primary checkout's build directory.

| Restored-artifact check | Local result |
|---|---|
| Restore revision/hash/executable checks | PASS |
| Target table, two cross-target IR builds, unsupported-target rejection | PASS |
| Effect-shape structural/output guard | PASS |
| Accel reactor non-hang | PASS |
| Quick IR determinism | PASS: three byte-identical pairs |
| Six parity corpora after the two fixture corrections | PASS: six; both formatting corpora retain all 39 golden lines |
| Browser host transport, heap, compiled collection snapshot | PASS |
| Explicit-release enum-list lifetime regression fixture | PASS: `enum_list_lifetime=true` |

Initial logs and the preserved parity failure: `consumer/` under
`/tmp/opencode/briev-ci-split-validation/`; corrected parity/browser/enum-list
logs: `consumer-corrected-fixtures/`. The original compiler independently
reproduces the stale fixture's SIGILL (`original-float-parity/`). The alternate
Data-to-Float construction exposed a separate unequal-width bitcast rejection,
recorded as an open compiler bug in BUGS.md; the validated formatting inputs
use the existing literal/intrinsic path.

These are **local** checks, not an exact-revision GitHub-hosted workflow pass.

### Repository acceptance — PASS

Used `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1`,
`TMPDIR=/tmp/opencode`. Commands/statuses/durations and full logs are in
`/tmp/opencode/briev-ci-split-validation/repository/`.

| Check | Result | Wall time |
|---|---|---|
| Recorded quick gate (auto → nextest) | 3,873 passed; zero failed/unreported/not-run; one explicit conformance exclusion | 182.873 s |
| `cargo test --lib` | 3,788 passed, zero failures/ignored | 287.958 s |
| `cargo build` | PASS, no new warnings against the prior increment | 0.210 s |
| `cargo test --bin brievc` | 18 passed | 0.821 s |
| `cargo test --test daily_use_smoke` | 42 passed | 354.215 s |
| Contract census | One conformance test passed; 3,787 filtered out | 249.542 s |

Quick evidence, relative to `/tmp/opencode/briev-ci-split-validation/`:
`repository/gate-results/20261115T092744Z-a707e1ad0f8c/manifest.json`.
The census reports zero announcements; its actual test result establishes
nonzero execution, not a count of proved obligations.

The final **31-control** suite also passes through `cargo test --test
gate_runner` after the missing-checkout-revision control was added. Shell
syntax, Rust wrapper formatting, Python/YAML syntax, Actionlint, relative
Markdown links, changed-file Praetor and whitespace checks pass. The restored
release artifact supplied the quick IR-determinism gate above. The two fixture
repairs pass their original six-corpus/39-line goldens and the subsequent
conformance census.

**Increment complete:** independent required jobs, shared verified build
artifact, fail-closed aggregate, retained attempt-specific evidence, controls,
and documentation. Publishing the branch/PR is required to qualify the actual
GitHub scheduling, caches, artifact service and required-check execution at
the resulting commit. Full local workspace execution and hosted CI are not
claimed by these selected local checks. Next implementation work is A1's
per-harness skip/detector/negative controls, followed by A2's stronger oracles.
