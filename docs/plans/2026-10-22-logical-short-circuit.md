# Logical short-circuit lowering

2026-10-22. Correctness prerequisite found during recommender I02. Main-based
compiler lane `codex/boolean-short-circuit`; application edits remain in their
original checkout. No performance optimization or speedup claim is made.

Evidence: `idx > 0 && items[idx-1]...` in a native contract fixture traps at
index zero. Generated IR loads index -1 and calls the RHS before `and i8`.
The reference interpreter explicitly short-circuits And/Or. The ordinary LLVM
expression path and loop-exit evaluator both eagerly emit the RHS.

Add one shared control-flow emitter: evaluate LHS once, branch to either the
constant short result or RHS, join through dedicated predecessor blocks and an
i8 phi. Nested expressions must update the current block; terminating RHS paths
must not supply a nonexistent phi value. Canonical Bool coercion remains shared.
Add specific AST arms before the existing BinaryOp fallback and use the same
emitter for logical exit expressions. Preserve all existing optimization arms,
type dispatch and rationale. Bitwise operators remain eager.

Validation: minimal runtime trap/counter gates, nested logical expressions,
loop conditions, installed-clang IR verification, native and WASM recommender
contract gates, full library failure-name comparison and directory Praetor.
Run the applicable determinism/toolchain gates; document environmental failures.
Update backend architecture and BUGS.md with the same compiler change.

The pre-change complete runtime table is in the application checkout's
`benchmarks/results/2026-10-22-recommender-i01-baseline.md` (6fb27e01, with pinned
compiler/runtime archives); it includes every row and the pthread-link failure.
This lane makes no timing comparison against that table. No benchmarking runs
concurrently with compiler tests/builds.

The runtime regression also covers expression-block mutations of locals and
parameters: discover local writes before body emission and hoist mutable ABI
parameters into native entry slots. No slot may be created only on the guarded
edge. Typed parameter loads preserve Bool, Float and pointer representations.
At a logical merge, retain only unchanged incoming pointer/state/last-value cache
entries; RHS-only cached registers do not dominate the skipped edge.

Focused final verification: three logical regressions pass, including native
-O0/-O2 invalid-read/effect tests, mutable Int/Float/Bool/String parameters,
RHS-only pointer caching, nested exit predicates and terminating RHS. The existing
result-gate width test passes after resetting the live callable-transaction post
block's termination/current-block state. Native parameter metadata resets with
binding maps, including save/restore around outlined cold guards.

The initial full library run was 3225 passed / 39 failed (875.70 s): the approved
38 environment failures plus the convergence test, subsequently fixed and rerun.
The broader LLVM subset was 329 passed / one toolchain failure: installed llc
cannot target spirv64 in kernel::tests::compile_to_spirv_emits_binary_magic.
This subset failure does not expand the accepted full-suite baseline. Final
integration must compare the full application suite's exact 38-name set.
`cargo build` completed without warnings. Directory Praetor has existing backend
diagnostics; the new logical modules have none. Application native/WASM and
determinism/toolchain validation are recorded in the I01/I02 execution record.
