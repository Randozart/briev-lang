# Collector prerequisite carve-out — 2026-11-05

This branch packages four already exercised measurement primitives independently
of the dirty application integration branch: checked native time, Linux process
stat parsing, Linux auxiliary-vector units and bounded mergeable histograms.
No Rust compiler change or net-stack workaround is included.

The clock API returns errors before reading output storage and checks conversion
overflow. now_ns retains its nonnegative contract by trapping if unavailable.
Process sampling retains PID/start identity and kernel units. Auxiliary-vector
parsing rejects malformed/duplicate/missing entries. Histograms retain counts,
overflow and maximum; quantiles are bounded-bin nearest ranks, never averages
of percentiles. Architecture docs travel with these module contracts.

Tests: four native fixtures, each checked and executed independently. The auxv
fixture constructs a binary image directly rather than importing HTTP slicing;
its test is about the binary parser and actual kernel units. A new integration
test runs all four fixtures and compares golden output. Full library validation
runs only after focused and application checks, per WORKFLOW.md.

Application verification uses this feature together with the separate
fix/read-file-descriptor-lifetime branch. The reusable collector checkpoint
passes functional accounting/schema gates, nine Valgrind cases and repeated
5000-sample runs. Standalone feature tests do not require that fix, but sustained
file sampling requires its descriptor cleanup. No throughput or capacity claim.

Focused native checks pass for all four modules. Final library verification
passes 3,499 tests with zero failures and the previously delegated source sweep
excluded. The measurement_primitives integration test runs all four fixtures
successfully. Praetor passes lib/std/os, lib/std/stats, fixture sources and the
new native integration test. Application functional and memory gates passed
with the independent descriptor-lifetime fix applied for integration.
Final cargo build completed without warnings.
