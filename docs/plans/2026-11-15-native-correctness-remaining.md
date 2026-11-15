# 2026-11-15 — Native Correctness Remaining Work (HIR + Cleanup)

**Date:** 2026-11-15
**Status:** Research complete — plan for next BUILD session

## What's done (this session + prior)

- Float div/0 native codegen matches interpreter (`@llvm.trap()`)
- `--safe` conservative compile profile
- `emit_memcpy` DRY refactor
- `SysCall#` c_long portability
- Counsel gate blocking in CI (0 violated census)
- 2 Kani harnesses (SMT cache + syscall width), 10/10 proofs verified
- H5 default flip: `BRIEV_HIR_EMIT` defaults to **true**, AST is opt-out

## What's left

### 1. HIR fidelity gaps (the `Other` tags)

The H5 flip landed with HIR as the default emission path. However, the HIR
lowering (`src/hir/lower.rs`) still has **5 AST variants** that fall through
to `HirExprKind::Other(tag)` / `HirStmtKind::Other(tag)`:

| AST variant | Tag | Status | Impact |
|-------------|-----|--------|--------|
| `Expr::Within` | `"within"` | **LIVE** — parsed, consumed by 10+ analysis modules, backend, interpreter | Programs using `within` expressions fall back to AST emission via `hir_expr_lossless` |
| `Expr::Exists` | `"exists"` | **LIVE** — parsed (`frgn_guard`), consumed by backend (`emit_expr.rs:2433`), interpreter, typechecker | `Exists` checks (compile-time function existence) fall back to AST |
| `Expr::DerivationBlock` | `"derivation"` | **LIVE** — parsed, consumed by `transition_graph`, `export_abi`, backend | Derivation blocks fall back to AST |
| `Expr::FormattingAnnotation` | `"formatting-annotation"` | **LIVE** — parsed, consumed by `reclamation`, `dataflow`, `defn_liveness` | Formatting annotations fall back to AST |
| `Statement::InlineDefn` | `"inline-defn"` | **LIVE** — inline defn statements | Falls back to AST |

**Impact:** For programs that use any of these constructs, the HIR path
detects the `Other` node via `hir_expr_lossless` / `hir_stmt_lossless`, falls
back to the AST path for that node. The `BRIEV_HIR_EMIT=0` opt-out is the
escape hatch. This is correct but means the HIR path is not yet the *sole*
path — the AST arms remain as fallback.

**What it takes to close:**
1. Add HIR variants for each of the 5 (or confirm they're dead and remove the
   AST variants — but they're all live).
2. Add lowering in `src/hir/lower.rs` for each.
3. Add `to_ast` reconstruction for each.
4. Add a `hir_expr_lossless` / `hir_stmt_lossless` arm for each.
5. Parity test + corpus sweep.

**Effort:** Each variant is small (1–2 fields), but the parity test + corpus
sweep per variant is the bulk. Estimate: 1–2 hours per variant, 5–10 hours
total for all five.

### 2. AST arm deletion (H5 retirement)

The H5 plan says: "AST arms deleted family-by-family (Rule 6 inverts for H5),
`to_ast` shrinks to zero." This requires:
- All 5 `Other` variants closed (above).
- The `hir::to_ast` bridge eliminated — every HIR node must be emittable
  directly without reconstructing an AST.
- The `BRIEV_HIR_EMIT=0` opt-out removed after one clean release cycle.
- The AST `Expr`/`Statement` enums shrink to only the variants that HIR
  doesn't cover (ideally zero).

**Prerequisite:** Item 1 must be complete first.

### 3. H5 ports (remaining analysis ports)

Per the H5 plan, the remaining HIR ports (in order) are:
- **`reclamation`** — HIR parity (just changed in #79/#81; reference-verified,
  AST-only). **First in the queue.**
- **`electronics`** — HIR port.
- **`dependency_graph`** — DONE (2026-11-10).
- **`casing`** — DONE (2026-11-10).
- **`purity`** — DONE (2026-11-10).
- **`destination_ownership`** — R1 facts ported; backend consumer flip pending.
- **`accel`** — unblocks `coalescing` + `image_storage`.
- **`gpu_schedule`** — last.

Each: named consumer, parity tests, flip under the flag with a debug equality
assert. The pattern is well-established (termination, watchdog, loop_shape,
dependency_graph, casing, purity all done).

### 4. Kani harnesses (remaining)

- **Casting graph** (`find_path`, `resolve_llvm_type`) — the single source of
  truth for type lowering. Proving termination + valid path existence.
  Larger effort (multi-session).
- **SMT proof cache** — DONE (5/5 verified).
- **SysCall# width** — DONE (5/5 verified).

### 5. Open issues (GitHub)

- **#2** — `#export` v2: Bool/String/struct marshalling, header completeness,
  CI C-bridge test.
- **#4** — macOS confirmation of `SysCall#` fix (needs a Mac host).
- **#5, #7, #8** — cannot reproduce without `test-project-otto` fixture.

### 6. Housekeeping

- `docs/plans/2026-11-15-proof-structural-derivation-how-far.md` — untracked
  from a prior session, should be committed.
- The "Verification nightly" CI failure (2026-11-15T11:57) — needs
  investigation (separate from the per-commit CI gate).

## Recommended work queue (priority order)

| # | Task | Effort | Dependency |
|---|------|--------|------------|
| 1 | HIR: add `Expr::Exists` variant + lowering + `to_ast` + parity test | 1–2 h | None |
| 2 | HIR: add `Expr::Within` variant + lowering + `to_ast` + parity test | 1–2 h | None |
| 3 | HIR: add `Expr::DerivationBlock` variant + lowering + `to_ast` + parity test | 1–2 h | None |
| 4 | HIR: add `Expr::FormattingAnnotation` variant + lowering + `to_ast` + parity test | 1–2 h | None |
| 5 | HIR: add `Statement::InlineDefn` variant + lowering + `to_ast` + parity test | 1–2 h | None |
| 6 | H5 port: `reclamation` HIR parity (next in the H5 port queue) | 2–4 h | None |
| 7 | H5 port: `destination_ownership` backend consumer flip | 1–2 h | #6 |
| 8 | Commit untracked `proof-structural-derivation` doc | 5 min | None |
| 9 | Investigate "Verification nightly" CI failure | 30 min | None |
| 10 | Kani: casting graph harness (larger, multi-session) | 4–8 h | None |

**Immediate (this session):** Items 1–5 (the 5 HIR `Other` variants) are the
highest-value native correctness work. They close the last fidelity gaps
between AST and HIR, enabling the eventual AST arm deletion and making the
HIR path the sole emission path.
