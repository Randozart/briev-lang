# Stop test option fixtures drifting from BuildOptions

## Evidence

PR #136's hosted Workspace tests job fails before running tests:
`E0063: missing field march_override` in `src/compile.rs:4064`.
The revision is upstream-derived `efd99d7b`; the packaging PR does not change
this source. Prior repairs (#129/#131) added other missing fields by hand,
but this exhaustive test fixture failed again on the next option addition.

## Correction and documentation

Use `BuildOptions::default()` with only the fixture's actual differences:
its file path, Webstack backend and 32-bit integer width. Compare every old
field against the default before deleting duplication. Preserve the dated
rationale explaining the earlier failures and why library-only tests missed
them. Record the diagnosis in `BUGS.md`; this changes no runtime/compiler API
and requires no architecture change.

Run existing binary/view tests (the direct compile failure oracle), library
tests, build, quick gate and all-target compile checking. Run Praetor against
the baseline source and record actual outcomes. This is test infrastructure,
not a code-generation or performance change.

## Execution record

- `cargo test --bin brievc`: all 18 binary/view tests pass.
- `cargo test --lib`: 3,935 passed, 2 ignored, zero failed.
- `cargo build`: passed with the same 12 existing verifier warnings.
- `bash scripts/gate.sh quick`: 4,056 selected/passed; zero failed,
  missing or unreported cases. Manifest:
  `target/gate-results/20261119T081251Z-5f3bcd32044e/manifest.json`.
- `cargo check --all-targets`: passed. Existing warnings remain in untouched
  test code, including the previously reported unused `BindRoute` import.
- Praetor: the same 16 baseline diagnostics, identical sites and metrics.
- The hosted pre-fix failure is retained at
  https://github.com/Randozart/briev-lang/actions/runs/44481350672/job/135415641534.
