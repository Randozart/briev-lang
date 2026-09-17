# 2D/3D index desugar — `a[i, j]` for shape-bearing arrays

**Date:** 2026-09-17
**Status:** Active
**Companion:** cyberllama `docs/plans/2026-09-17-abv-attention-ab.md` (M2a
pre-step), `docs/plans/2026-09-14-matrix-type-and-shape-metadata.md` (the type
this rides on)

## Problem

Every GPU kernel body we write repeats row-major offset math —
`logits[base + k]`, `out[row * Nkv + col]`, the attention composition's
head/batch strides. The shape already lives on the type
(`Matrix<T, R, C>`, `spec Rows/Cols`, delivered 2026-09-14); the source
still makes us spell the arithmetic. Rule 17: 3+ occurrences → centralize.

## Design

**Syntax:** `a[i, j]` (and `a[i, j, k]`) on a shape-bearing array.
The parser currently *rejects* comma-in-index (`reject_multidim()`,
3 sites in `expressions.rs`) — sites 1-2 (slice paths) keep rejecting;
the simple-index site (line ~673) collects the indices instead.

**AST:** new `Expr::IndexN(Box<Expr>, Vec<Expr>)` — additive variant.
Pure parse artifact; nothing downstream matches it.

**Desugar (the whole feature):** one new analysis pass
(`src/analysis/desugar.rs`) that rewrites
`IndexN(base, [i0, i1, ..., in])` →
`Index(base, i0 * (shape[1]*...*shape[n]) + ... + in * shape[last] + in)`
— row-major fold — using the base's shape from the TypeUniverse
(`matrix_shape`: rows/cols/depth). Runs right after parsing, BEFORE the
accel analysis, so `GemmPlan::match_stmts`, `detect_reduction`, and every
backend keep matching the plain 1D `Index` forms they already understand.
**Zero new backend match arms** — LLVM/SPIR-V/PTX/interpreter lower the
desugared expression unchanged.

**Shape resolution:** state-field bases (`logits: Matrix<F32, R, C>`)
resolve via the field's type. V1 restriction: Identifier bases only;
anything else is a compile error ("a[i, j] needs a shape-bearing array
field"). Plain `Float[N]` vectors have no shape → the same error.

## Gates

- `a[i, j]` on a Matrix field: parse → desugar round-trip prints the 1D
  equivalent; interpreter evaluates identically to the manual form.
- `a[i, j]` on a plain `Float[N]`: helpful error.
- Slice-with-comma still rejects (`arr[i, j:c]` etc.).
- Accel: a node body using `m[i, j]` still reaches the cooperative /
  elementwise paths (shape detection sees the desugared 1D form).
- `cargo test --lib` green.

## Undo

The pass is additive and the AST variant unmatched downstream; removing
`IndexN` + the call site restores today's parser rejections.
