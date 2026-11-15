# A0 — upstream integration and hosted qualification

**Date:** 2026-11-15. **Branch:** `fix/compiler-correctness` at `b9741653`.
Continues [independent CI jobs](2026-11-15-a0-independent-ci-jobs.md).
The user authorized publishing this branch and verifying hosted CI.

## Integration findings and plan

The branch started from freshly fetched `main` at `817f1d69`. Before publishing,
`origin/main` advanced to `8b69cbe2` (seven commits). A dry-run merge identified
four conflicts: CI, BUGS.md, and the two float-formatting corpora.

1. Merge the fetched upstream revision, retaining its typed-verifier/operator
   fixes, Lean exporter and per-file Counsel census. Keep the separate optional
   Counsel job and its always-uploaded attempt-specific log. Preserve upstream
   rationale comments when combining the workflow.
2. Retain the validated exceptional-value inputs (overflow and `Sqrt#`) in
   both formatting corpora. Restore the four trailing golden lines removed
   upstream: `inf`, `-inf`, `nan`, `nan`. Preserve both bug diagnoses and explain
   the integration resolution rather than erasing either history.
3. Add upstream's six `typed_boundary` tests to the centralized required
   selection, explicitly including its ignored real-Z3 float regression. Use
   existing exact-selection and Cargo execution paths; other ignored tests
   remain excluded. Controls must reject a missing required typed regression
   and establish selection under both quick/verifier modes and both runners.
4. Update the gate architecture's required selections, testing guide and INDEX; retain the
   adopted strategy and prior dated execution records verbatim. Record fresh
   validation counts here, and the coverage gap/resolution in BUGS.md.
5. Validate controls, dual-runner selection/result parity, library tests, builds,
   parity, determinism, workflow syntax/graph and changed-file Praetor. Run the
   full gate before publication because integrated upstream changes shared
   analysis. Use bounded local build resources and retain raw logs in
   `/tmp/opencode/briev-ci-hosted-validation/`.
6. Commit integration, push to `origin`, create a PR against `main`, and inspect
   CI for the exact head/merge revision. Verify independent job results,
   required aggregate, verifier manifest and artifact transfer. Diagnose and
   correct failures with retained evidence, then update the PR with results.

Local environment: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
CARGO_BUILD_JOBS=1 TMPDIR=/tmp/opencode`. No performance claim is made.

## Execution record

Results follow as the integration and hosted checks complete.

### Integration and static checks

All four conflicts are resolved. Upstream compiler sources are retained
unchanged; the workflow invokes its per-file Counsel script in the independent
non-blocking job. Both formatting goldens are byte-identical to `b9741653`
(39 lines), and the adopted strategy still matches `31164217` verbatim.

All **32 infrastructure controls** pass, including required typed-regression
execution and exclusion of an unrelated ignored probe. Actionlint 1.7.12,
workflow dependency/aggregate/artifact audits, shell/Python/YAML syntax,
Markdown links, conflict/whitespace checks and Praetor on the 28 branch-owned
files relative to `origin/main` pass. Evidence: `static/` under the validation
root. Upstream's CI at `8b69cbe2` is green
([run 43699982130](https://github.com/Randozart/briev-lang/actions/runs/43699982130));
this is baseline context, not qualification of this branch's new job graph.

### Integrated local acceptance — PASS

The integrated tree passed every command below. Logs, command arguments,
statuses and durations are retained under `integration/` in the validation
root. The only changes after execution are documentation of these results.

| Check | Result | Wall time |
|---|---|---|
| Quick, nextest discovery | 3,892 passed / 18 suites | 315.520 s |
| Quick, Cargo fallback | 3,892 passed / 18 suites | 256.859 s |
| `cargo test --lib` | 3,801 passed; zero failed/ignored | 274.585 s |
| `cargo build` | PASS; no new warnings | 55.736 s |
| Release compiler build | PASS | 345.087 s |
| Full workspace gate | 4,254 passed / 134 binaries; four ignored | 790.228 s |
| Target table / cross-target IR / rejection | PASS | 2.564 s |
| Effect-shape guard | PASS | 23.982 s |
| Accel non-hang | PASS | 1.909 s |
| Quick IR determinism | Three byte-identical pairs | 6.635 s |
| Six parity corpora | Six passed; both float outputs match all 39 original lines | 10.459 s |
| Browser host / heap / build / snapshot | All four commands passed | 1.407 s total |

The quick manifests have identical selected test identities, exclusions and
outcomes across runners: zero failures/unreported/not-run and one explicit
conformance exclusion. Each includes **38 verifier cases**, with all four
ignored Z3 regressions explicitly executed (the four ordinary full-workspace
skips). Manifest paths relative to `integration/`:

- `gate-nextest/20261115T095630Z-faf364cc5819/manifest.json`
- `gate-cargo/20261115T100146Z-62a5b11a907d/manifest.json`

These checks qualify the integrated local tree; hosted scheduling and artifact
service qualification follows publication.
