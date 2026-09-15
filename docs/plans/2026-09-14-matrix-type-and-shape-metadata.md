# The Matrix type: shape as parameterized type metadata

**Date:** 2026-09-14
**Companion:** `docs/architecture/backend-type-dispatch.md`, the capability
frontier (types as protocol + metadata).

## The thesis

The GPU GEMM shape (M/N/K, a/b/y) is currently *reconstructed* by parsing
the node body (`GemmPlan::match_stmts` — the decomposition lets, the
`foreach k`, the reduction). Per the types-as-metadata pillar, shape should
live on the **type**: a derived `Matrix<T, R, C>` whose R/C parameters are
the row/col counts, with `spec Rows: R; spec Cols: C` declaring which
parameters carry the shape. `Float` is the fundamental; `Matrix` is a
derived type (C++ "fundamental" terminology). This is general (GPU *and*
CPU — a shaped 2D array) and uses the existing numeric type-arg mechanism
(`Stack<Int, 8>`, `types.rs:235-244`).

## Why this shape

- **No type explosion** — one `Matrix<T, R, C>` family; the shape is the
  args, not per-shape derived types.
- **Single source** — the field's type args are the extent; no
  `count == rows×cols` reconciliation.
- **2D first** — GEMM operands are 2D (M×K, K×N, M×N); the reduction depth
  K emerges from the operand match (`q.cols == kt.rows`). A 3D tensor is a
  later, separate type.
- **Generic (Rule 15)** — the compiler reads the shape through the universe
  (`spec Rows` role-mapping + the args), never by name-matching "Matrix".
- **`spec`, not `!>`** — shape is a first-class type contract, important to
  GPU and CPU; it reverses the 2026-09-02 "container dims have no spec key"
  decision (the `Format` key is the precedent).

## The type

```briev
// lib/std/types/tensor.bv
type Matrix<T, R, C> {
    spec Rows: R;    // R is the row-count param
    spec Cols: C;    // C is the col-count param
};
// field: let q: Matrix<Float16, 128, 128>;   // Applied("Matrix", [Float16, Number(128), Number(128)])
```

## Compiler changes

1. **`ResolvedType.type_params: Vec<String>`** — the normalizer stores the
   type's param names in order (from `td.type_params`). Currently absent
   (`register_types.rs:363,475` set `vec![]`). Additive; benefits every
   generic type.
2. **Spec whitelist** — add `Rows`, `Cols`, `Depth` at the four sync sites:
   `spec_name_to_key` (definitions.rs:3610), `spec_display_key`
   (canonical.rs:423), the known-specs error message (definitions.rs:2921),
   and the 2026-09-02 comment (definitions.rs:3617-3623) whose rationale
   reverses.
3. **Parser** — `parse_spec_value` (definitions.rs:2912) accepts an
   **identifier** (a type-param reference) *or* an integer for rows/cols/
   depth: `spec Rows: R` → `Identifier("R")`, `spec Rows: 128` → `Int(128)`.
   The Endian/Format arms show the identifier path; the default arm stays
   for the rest.
4. **Reader** — `TypeUniverse::matrix_shape(ty) -> Option<(u64, u64)>`:
   - `Type::Applied(name, args)` → the declaration's `spec Rows/Cols`;
   - a `PropertyValue::Identifier(p)` value resolves `p` via `type_params`
     → index → the arg's `Number(n)`;
   - a `PropertyValue::Int(n)` value is the fixed shape.
5. **GEMM detection** — `match_stmts` reads M/N/K from the a/b/y fields'
   `Matrix` types when present (consistency: `q.cols == kt.rows == K`,
   `kt.cols == s2.cols == N`); the body remains the computation; the body-
   parsing reconstruction is the fallback for untyped programs.

## Gates

- Parser/normalizer: `spec Rows: R` resolves to the param; `spec Rows: 128`
  is fixed; the canonical round-trip prints `spec Rows: R`.
- `matrix_shape` unit tests (param-ref + literal + non-matrix returns None).
- The attention-decode (f16 + f32) uses `Matrix`-typed fields; correctness
  signatures unchanged; `cargo test --lib` green.

## Open follow-ups

- 3D tensors (`Tensor<T, ...>`) when transformer shapes need them.
- CPU backends reading `matrix_shape` for iteration/sizing.
- The f16 tensor epilogue fusion (the packed-f16 mul miscompile, tracked in
  the gpu_schedule plan).

## Undo

`type_params` is additive (default empty). The spec keys are whitelisted
(the `Format` precedent). The GEMM detection keeps the body-parsing
fallback — untyped programs are byte-identical.