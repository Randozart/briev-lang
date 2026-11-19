# Restore shell process intrinsics

## Baseline

On `efd99d7b`, `std/process.bv`'s `run("exit 7")` and
`spawn_with_output("printf probe")` fail in the LLVM normalizer: `Spawn#` and
`SpawnWithOutput#` are absent from its supported set. Existing unreachable
emission passes the String length header to `system`/`popen`; capture also emits
the retired two-word String representation. The interpreter has neither arm.

## Implementation

1. Implement reference interpreter shell execution with inherited process I/O,
   exit status decoding and byte-preserving stdout capture.
2. Repair native lowering and admit the two intrinsics together with execution
   regressions. Commands cross the existing String-to-C byte boundary; captured
   output uses the runtime's current owned `[length][bytes][NUL]` String ABI.
3. Consolidate the legacy `ShellCmd` runtime capture onto the same checked
   buffer implementation. Bound allocation growth, preserve the original
   allocation on realloc failure, and close every opened stream.
4. Test empty/nonempty/long/binary output, nonzero exit and signal termination,
   direct intrinsics and stdlib wrappers, interpreter and AST/HIR native at
   O0/O3; use Valgrind for native accesses. Run library/quick, determinism and
   toolchain gates. Record actual outcomes and limitations.

Document the repaired ABI and root causes in `BUGS.md` and the backend
architecture guide. Preserve provenance comments while retiring stale code.
No optimization or performance claim is made. New allocation handling is C;
validate it with native memory checks rather than claiming Rust Kani coverage.

## Execution record

- `cargo test --lib`: 3,936 passed, 2 ignored, zero failed, including the
  added shell status/byte-preservation unit test.
- `cargo build`: passed with the same 12 existing verifier warnings.
- `bash scripts/gate.sh quick`: 4,057 selected/passed; zero failed,
  missing or unreported cases. Manifest:
  `target/gate-results/20261119T073704Z-c098a074933b/manifest.json`.
- Expanded integration: 2/2 passed. Interpreter assertions and native AST/HIR
  at O0/O3 agree; Valgrind checks the generated programs and standalone capture.
- The standalone C regression covers lengths 0, 4095, 4096, 4097 and 8192,
  nonzero shell exit with binary output, and the legacy `ShellCmd` entry.
  AddressSanitizer/UBSan with leak detection passed.
- Release build passed; full IR-determinism corpus: 13/13 byte-identical.
- Praetor: no new diagnostic sites; the native dispatcher cyclomatic/cognitive
  metrics decrease from 31/68 to 27/60. New capture/interpreter helpers and
  regression tests have no diagnostics.
- All toolchain corpus programs execute under Clang 18/21. The unmodified
  matrix fails its byte comparison on printed elapsed times. A controlled
  comparison against `efd99d7b`, retaining raw outputs and excluding only the
  labelled timing observations, passes for both larger programs on both
  Clang versions. This is not a passing unmodified matrix or a timing claim.
