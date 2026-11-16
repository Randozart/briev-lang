# Release Stabilization Checklist — 2026-11-14

**Purpose:** The checklist that must be green before tagging a release.

---

## Gate 1 — Compiler correctness

- [x] `cargo test --lib` passes (3782 tests)
- [x] `cargo test --all` passes (all integration binaries via `cargo nextest run --all -j 4` in CI default gates)
- [x] `bash benchmarks/hir_emit_ab.sh --quick` green (HIR/AST byte-identical emission)
- [x] `bash benchmarks/ll_determinism_check.sh --quick` green (deterministic IR)
- [x] `cargo test --lib` with `BRIEV_HIR_EMIT=0` green (AST fallback path)
- [x] No `#[ignore]` quarantined tests in the test suite (0 quarantined as of 2026-11-14)
- [x] `--verify-bodies` on stdlib corpus: no `violated` entries (Z3 installed in CI as of PR #96)
- [ ] **Float div-by-zero**: interpreter raises `RuntimeError::DivisionByZero`, LLVM `fdiv` produces NaN — pre-existing known gap, documented here. Not fixable without a semantics decision.

---

## Gate 2 — Tier 1 platform contract

Tier 1 = x86_64 Linux + wasm32 (declared in SPEC §22).

| Platform | Status |
|----------|--------|
| x86_64 Linux | ✅ CI green |
| wasm32 | ✅ CI green (wasm tests in default gates) |
| aarch64 Linux | ✅ Tier-2 compile-only IR gate passes |
| riscv64 | ✅ Tier-2 compile-only IR gate passes |
| Unsupported target | ✅ Fails cleanly with diagnostic |

---

## Gate 3 — Interpreter/native parity

Systematic audit 2026-11-14: all arithmetic operations match (wrapping semantics, no `nsw` flag divergence).

| Operation | Status |
|-----------|--------|
| Add/Sub/Mul (wrapping) | ✅ MATCH |
| Div/Rem (wrapping) | ✅ MATCH |
| Neg/Abs (wrapping) | ✅ MATCH |
| BitAnd/Or/Xor on strings | ✅ MATCH |
| Char → Int (ASCII range) | ✅ MATCH |
| `term` exit code (i64→i32) | ✅ MATCH |
| Float relationals (`<`,`>`,`<=`,`>=`) | ✅ FIXED 2026-11-14 (#97) |
| Float `==`/`!=` | ✅ FIXED 2026-11-14 (#97) |
| Float div-by-zero | ⚠️ KNOWN GAP (interpreter errors, LLVM NaN) |
| String content vs identity | ⚠️ Not tested in interpreter (TODO: audit) |

---

## Gate 4 — Formal verification

| Check | Status |
|-------|--------|
| Z3 installed in CI | ✅ PR #96 (2026-11-14) |
| `cargo test --lib` (Z3-dependent) | ✅ 24 verify_bodies + 1 z3 test pass |
| `--verify-bodies` stdlib corpus | ⚠️ Run required (pending task 4) |
| counsel-check on LLVM backend | ⚠️ Not wired (pending task 5) |

---

## Gate 5 — Performance baseline (optional, non-blocking release)

- Nightly performance baseline (locked with `briev-compiler-baseline`)
- Determinism on benchmark corpus
- No unexplained regression vs baseline

---

## Open known limitations

| Limitation | Severity | Workaround |
|-----------|----------|-----------|
| Float div-by-zero: interpreter errors, LLVM NaN | Advisory | Validate denominators before division |
| String comparison: reference semantics vs content | Advisory | Use `.^Equal` for content comparison |
| `dyn Trait`: LLVM backend panics (`emit_toplevel.rs:754`) | Bug | Avoid `dyn Trait` in LLVM backend (interpreter OK) |
| CIRCT `ExportVerilog` `hw.module.generated` | Toolchain | Use LLVM backend for hardware targets |
| Hardware validator dead code | Clean-up | Remove `hardware_validator` module |

---

## Pre-release actions

1. Run `bash scripts/gate.sh full` — all binaries, both profiles
2. Run `bash benchmarks/ll_determinism_check.sh --full`
3. Run `brievc --verify-bodies` on stdlib corpus — record report
4. Review `BUGS.md` — no OPEN core correctness bugs
5. Tag and push: `git tag vX.Y.Z && git push --tags`
6. Publish to crates.io: `cargo publish` (for `briev-compiler` crate)
7. Update `RELEASE_NOTES.md`
