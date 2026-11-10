# Float formats as facts and products (2026-11-09)

**Status:** active. Decisions locked: **`Half` is canonical (LLVM-matching)**,
`Float16` is the stdlib alias, and **a type declaration must be able to
introduce a new float format without compiler edits** — formats are facts,
lowerings are products.

## Why

The compiler currently hardcodes float widths (`graph.rs` matches 16/32/64,
LLVM matches 16/32/64/80/128) and **width does not inherit through the base
chain** (`type MyF16: Float16;` silently becomes f32). That contradicts the
fundamentals doctrine: hardware capabilities are recorded as facts and
compiler products are derived from them. A programmer who declares a new
binary float must get casts, layout, state projection, ops and proofs from the
existing machinery — or a precise capability diagnostic naming the missing
fact, never a silent fallback.

## Vocabulary

Type-level facts (physical metadata, `spec`, §8.2 family):
- `Bits` — total width (exact).
- `Exponent` / `Mantissa` — format layout; `Bits = 1 + Exponent + Mantissa`.
- `Rounding` — `RNE` (default), future `RTZ`/….
- `Alignment` — ABI alignment (already supported).
A type with `Bits` only resolves against the standard-layout table; exp/mantissa
makes it self-describing.

Target-level facts (config/backend):
- Native formats per backend/flavor + the lowering each licenses (LLVM type,
  SPIR-V `OpTypeFloat` + required capabilities/extensions, PTX register type).
- Declared emulation products (exact softfloat) where native support is absent.
- Capability facts (Vulkan `shaderFloat16`/16-bit storage, OpenCL
  `Float16`/`Float16Buffer`, future float8/bfloat extensions).

## Products (one derivation, every consumer)

`compute_bits(ty)` = own `Bits` → `MaxBits` → `MinBits` → **declared base
chain** → target default. One helper used by the typechecker promotion, LLVM,
SPIR-V, PTX, interpreter and proofs. `(category=Float, width, exp, mantissa,
rounding)` selects a format fact; the fact yields the backend lowering or an
emulation product; absence yields a capability error naming the fact and fix.

- Arithmetic happens at the type's width (native IEEE semantics). Promotion:
  wider declared width wins; runtime narrowing requires `as`; literals narrow
  at compile time (RNE).
- Accumulation is a real type: composites take `acc_ty` (already in
  `matmul!`/`dot!`). f16 storage + f32 accumulate = `acc_ty Float`; native f16
  = `acc_ty Half`.
- ABI/state layout: `Bits/8`, alignment fact, endianness fact — already generic.

## P0 — unblock f16 (this session)

1. `Half` canonical; `lib/std/types/float.bv` declares `type Float16: Half;`.
   Keep `Float<Half>` as the variant spelling; `BFloat` is the bfloat16 fact.
2. Base-chain width inheritance in `resolve_llvm_type` / `resolve_spirv_shape`
   (and any typechecker width read): `type MyF16: Half;` resolves 16.
3. Literal narrowing: `let x: Half = 0.0;` / `0` from Float/Int literals at
   compile time; runtime f32→f16 requires `as`.
4. Native half arithmetic verified per backend where emitted; capability by
   width, not by type name.
5. `matmul_f16!` remains the f32-accumulate declared composite for now
   (retirement gate: fold into `matmul!(…, acc_ty)` once 3/4 land).
6. Fix the prelude anchor: `.abv` files with `node` but no `import`/`txn`
   currently get no stdlib prelude (this is what made `Float16` resolve to
   `Data` in the ceiling files). Anchor on `node` too.
7. f16 ceiling rows: use `Half` (+ explicit import for stdlib aliases) and
   record native vs f32-accumulate rows.

## P1 — format fact seam

8. `config/float-formats.dbvl` (pattern: `config/encodings.dbvl`): standard
   layouts (binary16/32/64/80/128, bfloat16) with LLVM/SPIR-V/PTX spellings and
   required capabilities/extensions.
9. Target native-format facts + capability rows; replace the hardcoded width
   matches with `(width, exp, mantissa, rounding)` lookups.
10. Capability errors: "target records formats {…}; `Float24` has no native
    fact and no emulation product — add a fact or use a supported format."

## P2 — self-describing formats

11. `spec Exponent/Mantissa/Rounding` in the metadata vocabulary + validation
    (`1+E+M = Bits`, known rounding); standard layouts remain config data.
12. Interpreter generic binary-float codec (reference semantics) parameterized
    by the format fact.
13. Emulation products: exact softfloat for interpreter + LLVM first;
    SPIR-V/PTX native-only initially with the derived diagnostic; webstack
    emulate or reject explicitly.
14. Proofs: rounding model from the fact; formats outside SMT FP theories
    promote to the nearest modeled width by default (opt-out documented).

## P3 — acceptance & docs

15. Derivation acceptance test: declare `Float24 { Bits: 24; Exponent: 8;
    Mantissa: 15; }` and run it on the interpreter + one emulating backend with
    **zero compiler changes**; the same type on a target without a fact yields
    the capability error.
16. Four-number ceiling matrix continues (blob harness for Briev timings is
    still the blocker); f16 rows for both accumulations.
17. Docs: `spec/SPEC.md` float format section, `docs/architecture/` type
    universe/casting docs, `MASTER-SYNTAX-REFERENCE`, tutorial; BUGS.md entry
    corrected (Float16→`Data` was missing registration/prelude, not protocol).

## Related decision — native tile type (deferred to after P1 + G3)

A **value-level tile** (`Tile<Elem, R, C>`) is justified for composition across
statements and composites, not for requesting speed. Existing surfaces: the
`tile<M,N>` node modifier (plan shape), `Matrix<T,R,C>` (logical 2D shape),
`shared tile: T[N]` (workgroup storage), and internal fragment/coopmat
lowering. A tile type must not duplicate them into another vocabulary.

Design rules when it lands:
- Fundamental-derived type family (like `Matrix`/`Vector`), ops in stdlib;
  metadata facts: `Scope: Registers|Subgroup|Workgroup|Cluster`,
  `Layout: RowMajor|Swizzled|FragmentA|B|C|Acc` (default derived), alignment.
  Element type uses the float-format facts above; accumulation is a type
  parameter (`Mma!(a, b, acc)`), consistent with `acc_ty`.
- Default unaffected: the compiler still derives tiles/stages/fragments from
  `Tensor`/`matmul!`/node modifiers. Acceptance test: the same kernel with and
  without `Tile` reaches the same ceiling; deleting stdlib tile composites
  leaves the derived path intact. A tile type required for peak would be a
  failing default (Rule 2).
- Boundary: modifiers shape the plan; tile types name values. Both present ⇒
  compatibility diagnostic, never precedence guessing.
- Kernel-local only (no ABI/state/FFI escape); proofs for linearity, liveness,
  scope, aliasing. Layout pinning is intent (ambiguity surface), not speed.
- Timing: after P1 (format facts) and G3 (cooperative-matrix-on-IL), because
  fragment layouts ARE the coopmat facts. A logical-only `Tile<Elem,R,C>` may
  land earlier if the ceiling harness needs a reusable block value.

Open questions: composition vs micro-layout control (decides logical vs
fragment v1); fundamental family vs stdlib over `Matrix`; author-pinned
layouts allowed; host-side tiles in scope.

## Discovered: f16 GEMM declaration kind (2026-11-09)

`matmul_f16!` misses the tensor channel because `mark_declared` writes the
macro NAME (`matmul_f16`) and `declared_matmul` gates on `"matmul"` — the
kernel falls to the general path (0.84 ms vs 0.123 ms at 512³). Clean fixes:
(a) a `declared_kind` fact on `$defn` that `mark_declared` writes, or
(b) fold `matmul_f16` into `matmul!` with `as acc_ty` substitution in the
composite expander. Do NOT add name lists in analysis. Tracked as the top W3
item.

## Verification

- `cargo test --lib`; `ll_determinism_check.sh --quick`; Praetor on changed
  dirs.
- On-device: f16 ceiling smoke on Vulkan/Level Zero/OpenCL; SPIR-V val for
  both flavors.
- No ABI change (Bits/8, alignment fact); descriptors already carry 2-byte
  elements.
