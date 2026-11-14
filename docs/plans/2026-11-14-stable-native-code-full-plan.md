# Stable Native Code — Full Plan

**Date:** 2026-11-14
**Decisions closed:**
- Interpreter is the reference. LLVM must match it.
- Add `#[pre]`/`#[post]` contracts as a matter of course, starting with the LLVM backend's pure-integer functions.

---

## Decision 1 — Float div-by-zero: LLVM matches the interpreter

**Current state:**
- Interpreter: `RuntimeError::DivisionByZero` when denominator is zero
- LLVM: `fdiv` produces NaN (IEEE 754, silent)

**Fix:** LLVM backend must emit a branch that checks for zero denominator before the `fdiv`, and errors instead.

**Changes required:**
1. `src/backend/llvm/emit_expr.rs` — the `fdiv` emission site. Add a zero check: `fcmp one %denom, 0.0` → `br %divzero_block, %fdiv_block`. The zero block calls `call void @llvm.trap()` — no C dependency, no runtime stub. The trap is the correct LLVM primitive for a precondition violation (undefined due to precondition).
2. Regression test: a `Float` div/0 must fail in native mode (crashes/panics). Verify with a probe program.
3. The `__briev_panic` in `briev_rt.c` is NOT used — eliminates a C dependency.

**Exit criteria:** `1.0 / 0.0` in native codegen calls the panic path (verified by testing).

---

## Decision 2 — Incremental contract addition: LLVM backend pure-integer functions

**Scope:** Every function in `src/backend/llvm/` whose body is in counsel's pure-integer subset:
`let`, `if`/`else`, `return`, constant `for`, `i8`/`i16`/`i32`/`i64`/`u8`–`u32`/`u64`/`usize`/`isize`, arithmetic, comparisons.

**Audit result (counsel-check probe on `emit_expr.rs`):**

Functions whose bodies ARE in the subset and have no contracts (prime targets):

| Function | File | What it computes |
|----------|------|-----------------|
| `get_local` | `emit_expr.rs` | register lookup by name |
| `struct_byte_size` | `emit_expr.rs` | byte size of a struct |
| `ordering_to_llvm` | `emit_expr.rs` | ordering enum → i32 |
| `hir_expr_lossless` | `emit_expr.rs` | HIR expr has no truncation |
| `hir_stmt_lossless` | `emit_expr.rs` | HIR stmt has no truncation |
| `hir_pattern_lossless` | `emit_expr.rs` | HIR pattern has no truncation |
| `hir_expr_family_covered` | `emit_expr.rs` | HIR expr families covered |
| `LlvmBackend::get_local_type` | `emit_expr.rs` | type of a register |
| `LlvmBackend::struct_field_offset` | `emit_expr.rs` | field offset in bytes |
| `LlvmBackend::struct_alignment` | `emit_expr.rs` | field alignment in bytes |

Functions with contracts (already there — don't touch):
`__briev_panic`, `__briev_exit`, etc.

**Approach:** One function per commit. Add `#[pre]`/`#[post]` (or the Briev equivalents via the counsel crate's Rust attribute surface). Run `counsel-check` after each addition — it must say "proven."

**Contract idioms for the LLVM backend:**
```rust
// A pure-integer function with a range fact
/// Returns the struct byte size. Must be non-zero for non-empty structs.
/// 2026-11-14: contract added as part of stable native code initiative.
#[pre(fields > 0, "empty struct has no size")]
#[post(ret > 0, "struct size must be positive")]
fn struct_byte_size(fields: usize) -> u64 { ... }

// A pure-integer guard
/// Offset must be within the struct's byte size.
/// 2026-11-14: contracts as a matter of course — counsel-check proves it.
#[pre(offset < size, "offset {offset} out of range [0, {size})")]
fn struct_field_offset(offset: u64, size: u64) -> u64 { offset }
```

**CI gate:** `counsel-check` exits 0 (all proven). The `continue-on-error: true` is removed once every target function has a proven contract.

**Exit criteria:**
- Phase 1: 5 contracts added, `counsel-check` reports 0 unproven on the targets
- Phase 2: All 10+ functions have contracts, `counsel-check` exits 0
- The `continue-on-error: true` on the CI step is removed → counsel-check becomes a blocking gate

---

## Relationship to the other pillars

The Float div-by-zero fix is a correctness fix (one emission site). The contract work is an incremental stability practice — each contract added is tested, proven, and checked in CI. They are independent; work both in parallel.

---

## Work queue

| # | Action | Pillar | Status |
|---|--------|--------|--------|
| 1 | Fix LLVM float div/0: emit zero-check + `__briev_panic` | Decision 1 | pending |
| 2 | Add panic stub to `briev_rt.c` | Decision 1 | pending |
| 3 | Regression test: Float div/0 fails in native | Decision 1 | pending |
| 4 | Add contracts to `struct_byte_size` (+ counsel-check probe) | Decision 2 | pending |
| 5 | Add contracts to `struct_field_offset` | Decision 2 | pending |
| 6 | Add contracts to `ordering_to_llvm` | Decision 2 | pending |
| 7 | Add contracts to the four lossless predicates | Decision 2 | pending |
| 8 | Remove `continue-on-error: true` from counsel-check CI step | Decision 2 | pending |
