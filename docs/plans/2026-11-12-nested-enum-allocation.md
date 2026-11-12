# Nested enum allocation bounds — 2026-11-12

Base `94eb8583`: authenticated application smoke and SQL pass, but server
Valgrind finds an eight-byte store immediately beyond a sixteen-byte block.
A standalone `List<Action>` containing `Publish(Accepted(Payload{...}))`
reproduces an invalid write and invalid read, with zero lost allocations.

The constructor currently sizes its allocation from the ambient expected type
before resolving the variant's actual enum. Payload emission resolves that enum
later and writes its full image. The allocation and writes therefore disagree
when the expected context is absent or belongs to an enclosing value.

Plan: resolve the constructor's concrete enum before destination selection and
allocation, then consume that same type for allocation width, result-slot
eligibility and payload emission. Preserve existing nested-payload rationale.
No type-name or application-specific dispatch is introduced.

Verification: native nested enum/list regression under AST/HIR, O0/O3 and
Valgrind; existing enum and object regressions; required library/quick/full
gates, build/Praetor, determinism and toolchain checks for backend changes.
Rebuild the actual server and repeat the instrumented journey before declaring
the external defect fixed. Update BUGS and backend architecture documentation
with the allocation/payload agreement contract in the same fix commit.

This is a bounds-correctness repair, not a throughput optimization.

The expanded regression exposed a second cause: depth-limited planning sized
`Envelope(Action(Event))` with a two-word nested Action, whereas standalone
Action construction/projection uses three words. Resolving the root constructor
alone therefore fixed two-level images but still overran three-level images.
The planner now permits inline nested images only when their complete enum
payload tree fits the remaining cap; otherwise it stores the nested handle.
This keeps construction, copies and match consumers on the canonical standalone
layout and bounds recursive enum planning. Tests cover depth 2 versus 3 and
mutually recursive declarations. Candidate v2 rebuild is in progress.

Candidate v2 built successfully. The expanded native fixture passes all four
AST/HIR × O0/O3 Valgrind cases, with `nested_enum_allocation_ok`; the baseline
fails the same fixture with status 91. The initial candidate only fixed the
two-level allocation. Praetor has no diagnostics in `enum_layout.rs` or the new
Rust regression; changed `emit_expr.rs` diagnostic headers match baseline.
The real application is being rebuilt against candidate v2 before the required
final compiler gates and instrumented server run.

## Final verification

- Real application rebuilt with v2: authenticated journey, independent SQL and
  exact receipts passed. Parent and both forked workers report zero Valgrind
  memory-access errors (`--leak-check=no`); no leak-readiness claim is made.
- `cargo test --lib`: 3682 passed. Nested enum and nested object native
  integration suites passed. Debug/release builds produced no warnings.
- Quick gate: 3681 passed/1 skipped library cases and 41 passed integration
  cases. Full gate: 4069 passed, 9 failed out of 4078.
- All nine full-gate failures reproduce on unchanged `94eb8583`: C boundary
  and C++ roundtrips fail the HIR/AST reclamation-plan assertion; three
  `ffi_proof_engine_tests` and four `integration_features` contract cases fail.
  The exact suites were rerun on base with `cargo test --test c_driver_boundary
  --test c_driver_cpp --test ffi_proof_engine_tests --test integration_features
  --no-fail-fast`. The full gate is not represented as green.
- Ten-program IR determinism passed. Installed clang 18 toolchain corpus:
  recommender, multinode and giantnode all passed.
- Kani was canceled at the user's request due to system-memory risk. No Kani
  proof is claimed; the unverified experimental harness was removed. Do not
  restart Kani without approval.
- Source changes are confined to enum planning/construction, regression tests,
  and documentation. Collector/driver leak investigations remain separate.
