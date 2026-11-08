# Nested object field storage width

Correctness fix, 2026-11-07. A nested four-Bool obj occupies a four-byte image
but its parent stores an integer-width handle. Summing image sizes for parent
fields allocates 12 bytes for `{ Int, Flags }` while emitting an eight-byte store
at offset 8. Valgrind confirms the overrun in a standalone reproduction and the
scheduled multi-user application driver.

Introduce a shared field-storage size query that distinguishes obj handles from
object images. Use it for ordinary field offsets, structural size refresh and
allocation sizes. Preserve inline static-struct, scalar and packed representations.
Cover small/large nested objects, adjacent fields, declaration order and target
handle widths. Verify the reduced program under native O0/O3 and Valgrind before
the application run. Complete library tests, backend determinism and toolchain
gates; document unavailable tools. Update BUGS.md and backend architecture with
the representation contract and preserve dated rationale comments.

## Implementation evidence

The adjacent-field reproduction exposed a second image-size walk in member reads.
Both member reads and event projections now use the shared offset query. Boxed
instance layouts and fixed-collection member offsets use field storage widths too.
The native fixture fails on the old compiler with an invalid write and corrupted
handle read; the fixed compiler passes native O0/O3 with zero Valgrind errors
and zero remaining allocations.

The upstream-main-based compiler cannot compile the complete journey because it
lacks the integration's mixed-integer-width fix (`icmp eq i8` with an i64 operand).
Application verification therefore uses the existing integration plus these same
layout changes. Praetor and cargo-kani are not installed on this host; their
gates cannot be claimed.

The full driver rebuilt against the patched integration gets past the original
441-byte allocation overrun. Its real-server Valgrind run still traps: the first
reported error is a use-after-free in `ws_accept_key`, on a 33-byte block allocated
by `b64_loop` and freed in `journey_load_start_upgrade`. Later reads in
`journey_transport_send` access a 24-byte block allocated by
`journey_transport_live_send` and freed in `journey_load_poll`. These are additional
lifetime failures, not a passing multi-user checkpoint. Evidence is retained in
the application `target/journey-layout-fixed/` (build logs, results, Valgrind and
SQL output). No successful rating/durable-run claim is made.

Final build completes without Rust warnings. The full ten-program IR determinism
gate passes. The toolchain matrix passes all three programs with the installed
clang-18, using MovieLens input and the script's synthesized similarity data.
Only one distinct Clang version is installed, so cross-version parity is not
established. AST/HIR native O0/O3 fixture runs all pass Valgrind.

The full library run finished with 3,455 passes and 78 failures, all under
`backend::llvm::tests`. Both new layout tests passed. The failures report fixture
contract rejection in `parse_bv_source`, before layout emission. A clean detached
worktree at base `fee0cd0d` is being checked against the same failing test group;
do not treat the full suite as passing. The native integration test is run
separately because the failing library command stopped the chained invocation.

The automated native integration test passes (one test, both AST/HIR emissions
at O0/O3). Final compiler SHA-256:
`d75671c2659265a5de0aee190862344e4aa94583422d5ae62c3c258e5e912a27`.
Application-integration compiler SHA-256:
`7b649ba606da9fcde57d38ba073b4ea48eefbb18688c0b0477ae5d4f8b586cdc`.

Clean-base comparison completed: `fee0cd0d` has 220 passing and 78 failing
LLVM backend tests. The sorted failure-name list is byte-identical to the
78 failures in the patched full-suite run. Thus no additional failing tests
were observed; the new layout regressions and automated native test pass.
This is not a green full-suite gate. Changes remain uncommitted, and no PR has
been published, pending resolution of the baseline contract-fixture failures
and the unavailable Praetor/Kani checks. Logs:
`/tmp/opencode/object-storage-{library,baseline,native}-tests.log`.

## 2026-11-08 gate resolution

After integrating upstream `735d1023`, corrected the backend fixture helper to
use the same transaction/definition tautology gate as production compilation.
The prior 78 failures came from using a simultaneous protocol invariant checker
on node before/after states. The full library suite now passes 3,543 tests;
the 304-test LLVM group, native AST/HIR O0/O3 regression, warning-free compiler
build and ten-program determinism gate pass. See the November 8 sync/fixture
plan for exact commands and remaining unavailable tooling. Application lifetime
failures remain separate from this now-green library gate.
