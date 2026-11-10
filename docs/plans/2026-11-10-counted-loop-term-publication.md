# Counted-loop term publication

## Goal and evidence

Restore CI termination without changing authored contracts or disabling loop
folding. Upstream run 42811020216 and fork run 42779354309 hit the 45-minute
job limit in the effect-shape guard. At main `d7c4869d`, the escaped mixed
fixture hangs even with `BOUND=1`; stateless terminates. The first bad CI
window contains `49b9440c`, which routed bare terms through the ordinary
statement emitter inside the counted-body walker.

The generated `txn_direct` and `txn_temp` return through `term.met` before
the loop-exit counter stores. The reactor therefore observes unfinished
counters and repeats the nodes. An emitted-IR A/B, linked with identical
`clang -O3 -flto -march=native -ffast-math` commands and the C runtime,
confirmed causality: the original times out; adding current-counter stores
on both successful term edges terminates and prints two `1000000` lines at
`BOUND=1000001`, matching stateless output. This is a correctness repair,
not a performance optimization.

## Implementation

1. Pin the mixed fixture with timeout-backed native execution and exact output
   assertions. Exercise zero, one, and print-boundary iteration counts, plus
   a focused consumer of published accumulator state and conditional terms.
2. Scope a publication obligation to internally folded transaction bodies.
   On the successful bare-term edge, materialize the current loop-carried
   fields through the existing native-typed state-store helper before cleanup
   and return. Keep postcondition-false edges and inline-member exits intact.
   Sort field emission; do not clear SSA maps needed by sibling paths.
   Track the actual post-false continuation for enclosing guarded merge phis.
   Invalidate pre-guard cached values of modified carriers at those merges.
3. Bound effect-guard executions so a future livelock reports its case instead
   of exhausting the entire CI job.
4. Preserve the counted-body rationale and document the publication boundary
   in `docs/architecture/backend-architecture.md` and the cause in `BUGS.md`.

## Verification

- Reproduce the new regression against the unmodified release compiler.
- Focused native tests at `-O0` and `-O3`, including P-gated term regressions.
- Effect-shape quick guard; library tests; debug/release builds; CI test suite.
- Full IR determinism corpus and installed-toolchain matrix.
- Praetor on changed Rust files, compared with the unchanged baseline.

## Results

- Before the fix, both native tests failed at `BOUND=1` with timeout status
  124. Adding a guarded accumulator assignment before the term separately
  reproduced invalid LLVM: the merge phi named `.cmgb` rather than `term.cont`.
- The bounded shell gate against the original compiler passed every shape
  check, then reported `mixed_effect_shape auto`, status 124, at its configured
  three-second execution limit.
- The guarded-term observer exposed a stale last-value cache after the merge:
  the tail used the pre-guard sum despite the correct carrier phi, printing 29
  instead of 32 at `BOUND=4`. An IR operand A/B confirmed the missing increment;
  the guard merge now drops those stale cache entries.
- Focused native tests PASS: both new regressions, both existing P-gated-term
  tests, and the counted nested-foreach regression. New cases execute at O0/O3.
- `cargo test --lib`: 3577 passed, zero failures. `cargo build`: PASS, no warnings.
- The initial `cargo test --all` reached three failures in unchanged
  `ffi_proof_engine_tests` with local Z3 4.13.3. The exact same test binary
  passes all 20 tests without Z3 on PATH; proof-engine and test sources are
  identical to the base commit.
- Praetor on the three changed backend files and the new test: no new
  diagnostic categories versus the base; existing oversized-function metrics
  remain (line counts shifted, guarded-body complexity decreased).
- Release compiler build, full effect-shape quick guard, all ten determinism
  corpus cases, accel non-hang gate, target table, and AArch64/RISC-V compile-only
  fixtures PASS. The release observer prints exactly `4`, `32`, `3` in both
  escaped and stateless modes.
- Both new fixtures pass at O0/O3 with Clang 18.1.8 and 21.1.8 (16 combinations).
  The existing toolchain harness reports identical recommender output on both;
  its other two rows skip missing `u.data`, and its empty-output glob makes the
  script exit 2 afterward. No performance claim is made from these runs.
- The no-fail-fast integration sweep also exposed unchanged downstream blockers:
  typed HIR collection snapshots still expect missing types after A2 typing;
  the canonical hold formatter has an unwired `format_guarded` helper; the
  published-allocation fixture contains committed conflict markers; the CLI
  test-runner assertion expects a different failure-message format; optional
  Valgrind 3.26 reports the runtime probe's `realloc(ptr, 0)`; and contract
  integration tests depend on Z3 availability. These are recorded for follow-up,
  rather than claiming the whole suite is green. Tests using an external Cargo
  target directory also need `BRIEV_STDLIB_PATH` for generated source files.
