# 2026-11-15 — Native Stability: Proof Layer

**Date:** 2026-11-15
**Status:** In progress

## Context

Native stability work from the prior session (div/0 trap, `--safe` flag, `emit_memcpy` DRY,
`SysCall#` c_long portability) is complete and merged. The remaining gap is the **proof
layer**: making the verification gate blocking, fixing contracts counsel can actually prove,
and introducing Kani harnesses for safety-critical paths that counsel cannot reach.

## Current census (counsel-check 0.6.0, local, Z3-backed)

| Contract | File | Status |
|----------|------|--------|
| `Interval::contains` | `src/analysis/region.rs` | **proven** |
| `ErrorConventions::is_null` | `src/ffi/error.rs` | **proven** |
| `UnsupportedCounts::total` | `src/hir/stats.rs` | **proven** |
| `CallGraph::node_count` | `src/analysis/call_graph.rs` | unproven (usize, `.len()`) |
| `CallGraph::edge_count` | `src/analysis/call_graph.rs` | unproven (iterable sum) |
| `BadTargetRegistry::push_width` | `src/backend/bad/registry.rs` | unproven (`&str`, `parse()`) |
| `LlvmBuilder::is_empty` | `src/backend/llvm/builder.rs` | unproven (`Vec::is_empty()`) |
| `LlvmBuilder::len` | `src/backend/llvm/builder.rs` | unproven (`Vec::len()`) |
| `CompilerContext::pointer_bytes` | `src/backend/llvm/context.rs` | unproven (u64 return) |

325 files scanned, 105 refused, **0 violated**.

## Work queue

### 1. Fix `pointer_bytes` for provability

`src/backend/llvm/context.rs::pointer_bytes` returns `u64`. Counsel's pure-integer
subset supports i64 but not u64 returns. Change the return to `i64` — the value is
always 4 or 8 (pointer width in bytes), so the i64 range is more than sufficient.
Callers use the value in arithmetic contexts where i64 is the native type anyway.

**Exit:** counsel-check proves `#[post(term >= 0)]` on `pointer_bytes`.

### 2. Remove `continue-on-error: true` from counsel-check CI

The per-file census shows **0 violated** across all scannable files. The counsel-check
step in `.github/workflows/ci.yml` (line ~119) can now be made blocking. The other
`continue-on-error` (line ~82) is for a different step (praetor) and stays as-is.

**Exit:** counsel-check CI step fails the build on any violated contract.

### 3. Kani harness: SMT proof cache key correctness

`briev-verify/src/smt.rs` memoizes Z3 results keyed by the SMT-LIB query string.
The cache is invalidated by Z3 version. Kani harness to verify:
- Two identical query strings produce the same cache key.
- Different query strings produce different cache keys (no aliasing).
- Version change invalidates all cached entries.

**Exit:** `kani kani/prove_smt_cache.rs` passes.

### 4. Kani harness: SysCall# c_long width cast

`src/interpreter/intrinsics.rs::exec_syscall` casts system-call numbers and 6
arguments to `libc::c_long`. On 64-bit Linux and macOS, `c_long == i64`.
On 32-bit, `c_long == i32`. Kani harness to verify the cast does not truncate
for valid syscall numbers (0..255) and argument values within i64 range.

**Exit:** `kani kani/prove_syscall_width.rs` passes.

### 5. Verify

- `cargo test --lib` — 3802+ pass
- `bash scripts/counsel-check.sh` — 0 violated, increased proven count
- `kani` runs pass on both harnesses

### 6. Commit + push

## What is NOT next

- Kani harnesses for the casting graph (`find_path`, `resolve_llvm_type`) — larger
  effort, multi-session.
- Making counsel-check blocking on the **praetor** step — that's a separate gate.
- Touching the live foreign lanes or GPU branch work.
