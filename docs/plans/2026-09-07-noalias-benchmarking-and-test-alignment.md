# noalias Benchmarking + Test Alignment + FFI Audit

**Date:** 2026-09-07
**Status:** Planning → Implementation

## Context

Three parallel goals:

1. **noalias via dispatch change:** The reactor inlines every txn body, erasing the `noalias` function boundary. The `@txn_*` function already carries `noalias nocapture` via `state_ptr_param` — the reactor just never calls it. Stopping inlining for multi-txn ticks restores the boundary, enabling LLVM `mem2reg` + SROA within each transaction.

2. **Test alignment:** 12 broken integration tests (API mismatches from old Parser/TypeChecker/ProofEngine APIs). 4 are dead code (test removed backends/modules). 8 need mechanical fixes. All red on `cargo check --all-targets`.

3. **FFI audit:** Confirm GLUE config-driven architecture is efficient and infinitely extensible. Verify no regressions.

## Part 1: noalias Benchmarking

### The Change

**File:** `src/backend/llvm/dispatch.rs` lines 122-157

Current: the general sequential dispatch path inlines every txn body via `emit_inline_txn_body()`.

New: for non-accel, non-fused txns in multi-txn ticks, call `@txn_<name>(ptr %state)` instead of inlining.

**Rationale:** The `@txn_*` function is emitted with `state_ptr_param` which includes `noalias nocapture` (context.rs:305, mod.rs:2454). When the reactor calls it, LLVM sees the noalias boundary and can promote field loads to SSA registers within the transaction body. When inlined, this boundary is lost.

**Scope:** Sequential reactor only, `dispatch.len() >= 2`. Single-txn ticks stay inlined (no benefit from function boundary). Accel kernel path unchanged (already calls `@txn_*`). Parallel reactor unchanged for now.

### Benchmarking Protocol

1. Create fresh baseline worktree at HEAD (`5d1d7e45`)
2. Run `build_and_bench.sh --runtime` on baseline, record results
3. Implement change
4. Run `build_and_bench.sh --runtime` on changed tree, record results
5. Compare ratios, document in `benchmarks/results/`

### Risk

Function-call overhead may hurt single-txn benchmarks. Mitigation: only applied to multi-txn ticks. Primary target: `sparse_dispatch` (8 nodes). Single-node benchmarks (nbody, float_math) should be unaffected.

## Part 2: Test Alignment

### Delete (dead code — modules/backends removed)

| File | Why |
|---|---|
| `tests/ffi_comprehensive_tests.rs` | Tests `ffi::loader`, `ffi::types`, `ffi::resolver`, `ffi::validator` — never existed as public modules |
| `tests/ffi_stdlib_tests.rs` | Same |
| `tests/test_rust.rs` | Tests `backend::rust` — removed |
| `tests/test_c.rs` | Tests `backend::c` — removed |

### Fix (API alignment)

| File | Changes |
|---|---|
| `tests/ffi_parser_tests.rs` | `Parser::new(code)` → `Parser::new(tokenize(code).unwrap(), code)`; `.parse()` → `.parse_program()`; `ForeignBinding` struct reshape |
| `tests/ffi_typechecker_tests.rs` | Parser API; `TypeChecker::new()` → `check_program(items, universe)`; `TypeError` import |
| `tests/ffi_proof_engine_tests.rs` | Parser API; `ProofEngine::new()` → free functions |
| `tests/integration_features.rs` | Parser API; `TypeChecker`/`ProofEngine` imports |
| `tests/fuzz_frontend.rs` | Parser API (8 call sites) |
| `tests/fuzz_fault_injection.rs` | Parser API; `Value::Int` → `Value::Atom(Atom::Int(...))`; `StateDecl.expr` removed; `ast::Program` removed |
| `tests/glue_test.rs` | `extract_bridge_info` needs 3rd arg `None` |
| `src/compile.rs` | `webstack_opts()` missing `isr_mechanism: None` |

### Replace Deleted FFI Tests

Unit tests inside existing module test suites (parser, typechecker, backend) testing the GLUE config-driven architecture:
- `frgn` declarations with `from` provenance
- `frgn` validation (signature vs binding mismatch)
- Contract obligations on `export` functions
- `frgn` → parse → typecheck → backend → verify LLVM IR

## Part 3: FFI Architecture Audit (read-only)

Checklist:
1. `BoundaryOwnership` model handles all current `frgn` patterns
2. `ResolvedFrgn` dispatch covers all `FromSpec` variants
3. GLUE config loading is robust (missing config = clear error)
4. `export` pipeline generates correct wrappers for all registered languages
5. `gen.bv` plugin escape hatch documented and functional
6. metropipe (runtime IPC) tested and documented

## Execution Order

1. Plan doc (this file)
2. Baseline worktree + benchmark capture
3. Delete dead test files
4. Fix broken test files
5. FFI audit (concurrent with test fixes)
6. Implement noalias dispatch change
7. Verify correctness + benchmark
8. Document results
9. Commit + push each logical step
