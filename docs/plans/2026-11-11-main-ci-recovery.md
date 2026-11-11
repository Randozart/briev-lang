# Main CI recovery (2026-11-11)

## Goal and evidence

Restore the blocking CI gates without suppressing failing tests. Baseline:
upstream `834e806d`, run `42903656766`. Build, library tests, and target checks
passed; `cargo nextest run --all -j 4` failed (exit 100). Subsequent gates were
skipped. The last observed successful main CI run was `42528935284` at
`219147d0` (November 8). Log-download endpoints return HTTP 403 with the
available credentials; job metadata and annotations remain readable.

## Investigation and implementation

1. Reproduce with CI's nextest version (0.9.151), retaining full local logs.
2. Isolate each failure and distinguish compiler defects, test isolation,
   and missing toolchain prerequisites using focused reproductions.
3. Fix root causes and run the affected tests, then the complete blocking
   gates, including steps skipped by the upstream failure.
4. Publish a dedicated branch/PR with evidence and remaining limitations.

## Documentation and verification

Preserve rationale comments and add dated explanations at changed behavior.
Record confirmed defects in `BUGS.md`; update architecture documentation if
compiler interfaces change. Record results here. Before committing, run
`cargo test --lib`, `cargo build`, and Praetor on changed source directories.
This is correctness/CI work, not a performance optimization campaign.

## Confirmed upstream failures

Credentials were repaired during investigation and the full job log became
available: 3,973/3,976 tests passed. The three failures were:

- `core_usable_gate::frgn_only_in_interop_lanes`: the new stb_image foreign
  declarations lived under `lib/std/image/`, outside the interop allowlist.
  Move declarations to `lib/glue/stb_image.bv`; retain the public wrapper path.
- `tier2_test_runner::test_runner_pass_fail_golden`: the CLI correctly rejected
  stdout `3\n` against `2\n`, but the test expected the retired wording
  `golden mismatch` instead of `stdout mismatch`. Assert the current diagnostic,
  expected/actual bytes, failed count, and nonzero exit.
- `daily_use_smoke::arena_folded_loop_uses_arena`: the runtime sum was correct,
  but the allocation fell back to malloc. Investigate the value/alias distinction
  in assignment edges: loading a scalar must not make that scalar alias the
  source buffer merely because the load reads it.

## Focused verification

- New scalar-read regression failed before the fix (`Malloc` versus `Arena`).
- After the shared value-alias traversal: all 10 allocation analysis tests and
  all four native arena tests pass, including the original CI failure.
- The existing public image import builds, links the vendored C implementation,
  and correctly rejects a corrupt-image input after moving the declarations.
- Praetor: changed runner test passes; allocation analysis has the same 14
  diagnostics as baseline, with no new diagnostics.
- An initial nextest run overlapped a rebuild and lost its library executable
  mid-run. Its result is invalid; final verification must run sequentially.

## Downstream gate investigation

The first release build passed the effect-shape guard, target table, AArch64
and RISC-V compile-only checks, and accel reactor gate. The browser collection
snapshot then exposed retired top-level `when` syntax in its fixture. Changed
only that residence declaration to `hold`; the existing WASM snapshot gate
passed, including Unicode, 1,100-item growth, empty snapshots, and reset.
Browser host transport and heap tests and quick IR determinism also passed.
Final release verification after the allocation fix remains to be recorded.

## Sequential full-run results

- `cargo build`: pass, no warnings.
- `cargo test --lib`: 3,595 passed, zero failures.
- `cargo nextest run --all -j 4`: 3,970/3,978 passed, zero skipped. All three
  tests that failed upstream now pass. The eight remaining local failures are
  seven Z3-enabled proof assertions (`ffi_proof_engine_tests` and
  `integration_features`) and Valgrind 3.26's `realloc() with size 0` diagnostic
  in `reclamation_runtime`. Those tests passed in upstream run 42903656766;
  this host has Z3 4.13.3 and Valgrind installed, unlike the workflow's explicit
  prerequisites. No tests or optional tools were disabled to obtain a pass.
- Kani harness added and launched with `--features kani,kani_full --harness
  pointer_alias_escape_survives_value_flow`; compilation completed and symbolic
  verification is still running. No proof-success claim yet.
- Release/downstream gates and GitHub CI validation remain in progress.
