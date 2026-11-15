# 2026-11-14 Stable Native Code — Plan (revised, post-execution)

**Date:** 2026-11-14 (original) / 2026-11-15 (revision)
**Status:** Decision 1 done. Decision 2 re-scoped — the plan's pure-integer target list is
largely outside counsel-check's subset, and the CI gate is not functional. Revised plan below.

---

## Original plan (2026-11-14, kept for provenance)

### Decision 1 — Float div-by-zero: LLVM matches the interpreter

Interpreter raises `RuntimeError::DivisionByZero` for `Float / 0.0`; LLVM emitted
silent NaN. Fix: emit `fcmp oeq %denom, 0.0` → `br i1` → `call void @llvm.trap()`
before `fdiv`. No C dependency (`__briev_panic` in `briev_rt.c` is not used).

### Decision 2 — Incremental contract addition: LLVM backend pure-integer functions

Add `#[pre]`/`#[post]` contracts to functions in `src/backend/llvm/` whose bodies
are in counsel's pure-integer subset. Original target list:

| Function | File |
|----------|------|
| `get_local` | `emit_expr.rs` |
| `struct_byte_size` | `emit_expr.rs` |
| `ordering_to_llvm` | `emit_expr.rs` |
| `hir_expr_lossless` | `emit_expr.rs` |
| `hir_stmt_lossless` | `emit_expr.rs` |
| `hir_pattern_lossless` | `emit_expr.rs` |
| `hir_expr_family_covered` | `emit_expr.rs` |
| `LlvmBackend::get_local_type` | `emit_expr.rs` |
| `LlvmBackend::struct_field_offset` | `emit_expr.rs` |
| `LlvmBackend::struct_alignment` | `emit_expr.rs` |

CI gate: `counsel-check` exits 0 (all proven). Remove `continue-on-error: true`
once every target has a proven contract.

---

## What was actually done (2026-11-14 session)

### Decision 1 — COMPLETE

- `3a2fea32f` — `fix(llvm): Float div/0 traps in native codegen`
  - Zero-check + `@llvm.trap()` at the top-level `Expr::BinaryOp` dispatch
    (`src/backend/llvm/emit_expr.rs:~2169`).
  - 3 regression tests in `tests/cli_args_test.rs`:
    - `float_div_by_zero_traps_in_native_mode` (SIGILL, signal 4 → exit −4)
    - `float_div_nonzero_returns_correct_value` (1.0/2.0 = 0.5, exit 0)
    - `float_div_four_returns_correct_value` (1.0/4.0 = 0.25, exit 0)
  - `build_link_run_argv` extended to report signal kills as negative exit codes.
- Extended in `74311bce0` (post-merge): the guard now uses the numeric
  mixed-operand adapter **before** checking the promoted divisor's Float
  category, so `7 / denom` (integer-spelled literal, Float denominator) no
  longer emits `fcmp oeq i64 %float_register, 0.0`.
- Two guards now exist: top-level dispatch (~2182) and intrinsic/template path
  (~11573).

**Exit criterion met:** `1.0 / 0.0` traps in native codegen.

### Decision 2 — PARTIAL / RE-SCOPED

- `8f9ebf394` — `feat(counsel): add contracts to ordering_to_llvm, sum_field_sizes,
  struct_byte_size`
  - `ordering_to_llvm`: `#[post(term == 1)]` (total function)
  - `sum_field_sizes`: new pure helper, `#[post(term >= 0)]`
  - `struct_byte_size`: `#[post(term >= 1)]` (fallback 8)
  - `use counsel::post;` import added.
- `997243b2f` — `feat(counsel-seed-v2): add 4 advisory contracts`
  (from PR #100, applied to main, PR closed)
  - `CallGraph::node_count`: `#[post(term >= 0)]`
  - `Interval::size`: `#[post(term >= 0)]`
  - `count_statements_recursive`: `#[post(term >= 0)]`
  - `canonical_rid`: `#[post(term >= 0)]`

**Reality check (counsel-check 0.6.0, local, no Z3):**
- `emit_expr.rs` is **refused** by counsel-check (glob imports + non-i64 types).
  The three contracts in that file are inert — never proven, never violated.
- counsel requires **signed i64** arithmetic; `u64` and `&str` returns are refused.
- counsel-check over the whole `src/` tree **exits 2** (refused at
  `src/accel_rt.rs` for an unexpanded attribute macro). No JSON census produced.
- Per-file scan: 351 of 430 `.rs` files are scannable; 79 are refused.
- Per-file census across all 351 scannable files: **0 proven, 0 unproven,
  0 violated, 6264 skipped**. The existing contracts (`Interval::contains`,
  `ErrorConventions::is_null`, `UnsupportedCounts::total` — claimed "proven" in
  PR #98) are NOT proven locally without Z3; they may be proven in CI where
  Z3 is installed.
- Z3 is not installed locally. The proving phase requires CI.

---

## Revised Decision 2 — what actually works and what to do next

### Why the original target list doesn't work

The plan's 10 target functions are all in `emit_expr.rs`, which counsel-check
refuses (glob imports, `u64`/`&str` types, method calls). None of them can be
proven. The `hir_*_lossless` functions are not standalone pure-integer fns —
they are gates inside `emit_expr` that inspect HIR views.

The functions that ARE in counsel's pure-integer subset are small, mostly in
`src/analysis/`, `src/hir/`, `src/ffi/`, and a few in `src/backend/llvm/`
(`builder.rs`, `context.rs`, `types.rs`).

### What the CI gate actually needs

The counsel-check step in `ci.yml:110` currently runs:
```
counsel-check --format human src/
```
which exits 2 (refused at `accel_rt.rs`). With `continue-on-error: true`, the
failure is swallowed and the step contributes nothing. To make it a real gate:

1. **Per-file scanning** (lower-risk, matches counsel's file-independent design):
   iterate `.rs` files under `src/`, run counsel-check per file, collect verdicts.
   Refused files are logged as skipped, not failures. The step fails only on a
   `violated` verdict.
2. **Or fix the refusal** at `accel_rt.rs` (find the unexpanded attribute macro
   and either rewrite or allowlist). Lower priority.

### Revised work queue

| # | Action | Status |
|---|--------|--------|
| 1 | Fix LLVM float div/0 | **DONE** (`3a2fea32f` + `74311bce0`) |
| 2 | Regression tests for div/0 | **DONE** (3 tests in `cli_args_test.rs`) |
| 3 | Add contracts to `ordering_to_llvm`, `sum_field_sizes`, `struct_byte_size` | **DONE** (`8f9ebf394`) — honest, not provable (file refused) |
| 4 | Add 4 advisory contracts (node_count, size, count_stat, canonical_rid) | **DONE** (`997243b2f`) |
| 5 | Make counsel-check CI step per-file scanning | **TODO** — change `ci.yml:110` + `scripts/counsel-check.sh` |
| 6 | Get real census from CI (Z3 available there) | **TODO** — after #5 |
| 7 | Expand to provable contracts (pure-integer subset, i64, no casts/methods) | **TODO** — candidates in `analysis/region.rs`, `hir/stats.rs`, `ffi/error.rs`, `backend/llvm/builder.rs`, `backend/llvm/context.rs`, `backend/bad/registry.rs` |
| 8 | Remove `continue-on-error: true` (make gate blocking) | **TODO** — only after #6 shows 0 violated |

### Provable-contract candidates (for #7)

Survey before picking (read body, confirm it's in the subset — no casts, no
method calls on unknown types, no `while let`, i64 arithmetic only):
- `src/hir/stats.rs` — `UnsupportedCounts` already has a proven `total`; sibling
  counters may be provable.
- `src/analysis/region.rs` — has `use counsel::{post, pre}`; check small helpers.
- `src/ffi/error.rs` — `ErrorConventions::is_null` proven; sibling null checks.
- `src/backend/llvm/builder.rs` — has `use counsel::{post, pre}`; check small fns.
- `src/backend/llvm/context.rs` — has `use counsel::{post, pre}`; check small fns.
- `src/backend/bad/registry.rs` — has `use counsel::{post, pre}`; check small fns.

For each: write a `#[pre]`/`#[post]` that is **actually provable** (not just
honest), verify with counsel-check locally if Z3 is available, else rely on CI.

---

## What is NOT next

- Do not re-investigate the `--verify-bodies` relational-over-sum "bug" —
  retracted as correct bitvector wraparound behavior (BUGS.md 2026-11-14
  RETRACTION).
- Do not try to prove unbounded arithmetic postconditions — bound the values
  or accept the refusal.
- Do not try to make the counsel gate blocking before the per-file scan works
  and CI produces a real census with 0 violated.
- Do not touch the live foreign lanes or GPU branch work.
