# Language Stability + Counsel Integration Plan

**Date:** 2026-11-14
**Goal:** Stable native code — the interpreter and LLVM codegen match for all inputs, and the counsel crate formally verifies Briev and the Rust FFI boundary.

---

## Pillar 0 — Close interpreter/native gaps (foundational)

Every divergence between the reference interpreter and LLVM codegen is a stability bug. The Float relationals fix (#97) closed one; the HIR/AST A/B fix (c394451a5) closed another. More exist.

**What to audit** (systematic, not guess-and-fix):
- Division by zero semantics (Rust panics in debug, wraps in release; Briev Int semantics?)
- NaN handling in Float comparisons (`NaN < x` is always false in both)
- Overflow semantics (debug panics vs release wrapping)
- String comparison (content vs pointer identity)
- Bitwidth differences (i32 vs i64 on 32-bit, usize vs u64)
- Boolean short-circuit (`&&`/`||` — interpreter eager vs native branch)
- Uninitialized/poison values
- `assert` vs `expect` failure mode (panic vs exit code)
- Pattern match completeness at runtime
- `endprogram <expr>` vs `term <expr>` return value semantics

**Exit gate:** `bash benchmarks/hir_emit_ab.sh --quick` green + `cargo test --lib` green + no known divergences in BUGS.md.

---

## Pillar 1 — Make `--verify-bodies` run in CI (Z3 is now live)

PR #96 installed Z3 in CI. `--verify-bodies` should now discharge its obligations.

**What to check:**
1. Run `cargo test --lib` — the `verify_bodies` tests in `briev-verify/` now have their SMT back-end.
2. Run `brievc --verify-bodies` on the stdlib corpus and record what the prover surfaces.
3. Any `unproven` entries become advisory warnings; `violated` entries are hard errors (already enforced).

**Exit gate:** `--verify-bodies` runs clean on the stdlib corpus in CI with no `violated` entries.

---

## Pillar 2 — Counsel integration: verify the Briev compiler itself

The `counsel-crate` is production-grade (v0.5.0). The `briev-counsel-lab` fork has counsel-macros as a dependency. The gap is that the **Briev compiler's own Rust code** is not verified by counsel.

**A. Use counsel to verify the LLVM backend's critical Rust paths**
- `src/backend/llvm/emit_expr.rs` — struct literal emission, register typing
- `src/backend/llvm/emit_toplevel.rs` — parameter prologue, call marshalling
- `src/backend/llvm/mod.rs` — `_start` asm, owned allocation

These are the paths where bugs cause silent miscompiles (like the aggregate-arg-ABI bug caught by PR #94's regression test, but found by inspection not by a prover).

**B. Author Briev contracts on the counsel prover's own AST**

The prover lives in `counsel-crate/src/check/prover/`. It handles:
- `eval_symbolic` — substitutes names eagerly, tracks interval bounds
- `smt` module — encodes to Z3 BitVec
- `walk` module — straight-line path execution, guard splitting

Adding Briev `[pre][post]` contracts on these functions makes the prover self-verifying. The `briev-counsel-lab/labs/` directory is empty scaffolding — fill it.

**C. Wire `counsel-check` into the Briev compiler test suite**

`counsel-check` (the static half) verifies Rust source files at check time. Wire it into `tests/` as an additional CI gate for the Rust FFI boundary.

**Exit gate:** `counsel-check` runs over `src/backend/llvm/` and `src/interpreter/` in CI with `proven` outcome on critical paths.

---

## Pillar 3 — Unquarantine and stabilize Stage 3 of the stability plan

The 2026-11-02 stability plan (Stage 3) has remaining items:
1. ~~Generic callable txn specialization~~ — **done** (two-struct `iter_chain` golden)
2. **C bridge/CLI tests** — `tests/cli_args_test.rs` was just fixed (stdlib-path + _start argc)
3. Make `cargo test --all` a required release gate
4. Add conformance coverage for every fixed bug

**Immediate action:** unquarantine `tests/cli_args_test.rs` (it passes now), make it a required gate.

---

## Exit Criteria for "Stable Native Code"

The language is stable when:
1. `cargo test --lib` green (3782 tests)
2. `cargo test --all` green (all integration binaries)
3. `bash benchmarks/hir_emit_ab.sh --quick` green (byte-identical HIR/AST emission)
4. `bash benchmarks/ll_determinism_check.sh --quick` green (deterministic IR)
5. `cargo test --lib` with `BRIEV_HIR_EMIT=0` green (AST path still works)
6. `counsel-check` runs over LLVM backend critical paths with `proven` verdict
7. No OPEN core correctness bugs in `BUGS.md`
8. Z3-dependent tests pass in CI (`--verify-bodies` on stdlib corpus)

---

## Work Queue (in order)

| # | Action | Pillar | Status |
|---|--------|--------|--------|
| 1 | Audit interpreter/native gaps (systematic) | 0 | pending |
| 2 | Unquarantine `tests/cli_args_test.rs` | 3 | pending |
| 3 | Make `cargo test --all` a required release gate | 3 | pending |
| 4 | Run `--verify-bodies` on stdlib corpus, record report | 1 | pending |
| 5 | Wire `counsel-check` into LLVM backend CI path | 2 | pending |
| 6 | Add `[pre][post]` contracts to counsel prover critical functions | 2 | pending |
| 7 | Fill `briev-counsel-lab/labs/` with self-verification examples | 2 | pending |
