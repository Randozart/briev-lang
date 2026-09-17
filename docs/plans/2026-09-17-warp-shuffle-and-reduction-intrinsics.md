# Warp Shuffle, Reduction & Vote Intrinsics — Phase 2

**Date:** 2026-09-17
**Status:** Active
**Depends on:** Phase 1 (commit `d9501df3`) — ShuffleDown#, ShuffleXor#, SubgroupFAdd#, SubgroupFMax#, SubgroupFMin#, Exp# fixes
**Motivation:** Flash attention requires fused multiply-add for intermediate accumulators, warp ballot for lane predicates, lane-index broadcast for cross-lane data exchange, and warp-level max/min reductions on PTX. These are GPU-unique hardware primitives that cannot be expressed in pure `.bv` code.

---

## Phase 1 — COMPLETED (commit d9501df3)

| Intrinsic | Signature | Status |
|-----------|-----------|--------|
| `ShuffleDown#(val, delta) -> val` | `(T, Int) -> T` | Done — PTX + SPIR-V |
| `ShuffleXor#(val, lane_mask) -> val` | `(T, Int) -> T` | Done — PTX + SPIR-V |
| `SubgroupFAdd#(v) -> Float` | `(Float) -> Float` | Done — SPIR-V (PTX pending) |
| `SubgroupFMax#(v) -> Float` | `(Float) -> Float` | Done — SPIR-V (PTX pending) |
| `SubgroupFMin#(v) -> Float` | `(Float) -> Float` | Done — SPIR-V (PTX pending) |
| `Exp#(x) -> Float` | Generic float | Done — all backends |

---

## Phase 2 — THIS PLAN

### New intrinsics

| Intrinsic | Signature | PTX (sm_80+) | SPIR-V | LLVM (CPU) | Interpreter |
|-----------|-----------|--------------|--------|------------|-------------|
| `Fma#(a, b, c) -> val` | `(Float, Float, Float) -> Float` | `fma.rn.f32` | `GLSL.std.450 Fma` | `@llvm.fma.f32` / `@llvm.fma.f64` | `a.mul_add(b, c)` |
| `SubgroupBallot#(pred) -> mask` | `(Bool) -> Int` | `vote.sync.ballot.b32 %out, %pred, 0xFFFFFFFF` | `OpGroupNonUniformBallot` → extract `.x` → cast to Int | Return 1 | Return 1 |
| `SubgroupBroadcast#(val, lane) -> val` | `(T, Int) -> T` | `shfl.sync.idx.b32 %out, %val, %lane, 0x1F, 0xFFFFFFFF` | `OpGroupNonUniformBroadcast` | Return val | Return val |

### PTX reductions for existing intrinsics

| Intrinsic | PTX emission |
|-----------|-------------|
| `SubgroupFMax#(v)` | `redux.sync.max.f32 %out, %v, 0xFFFFFFFF` (sm_80+, register-only) |
| `SubgroupFMin#(v)` | `redux.sync.min.f32 %out, %v, 0xFFFFFFFF` (sm_80+, register-only) |

**Note on `redux.sync`:** Register-only warp reduction. No shared memory needed. Requires sm_80+. All PTX targets sm_86 (verified in `general.rs:154`).

---

## Files to modify per intrinsic

### Fma# — 9 files

| File | Change |
|------|--------|
| `src/intrinsic_signatures.rs` | Add signature `("Fma#", params vec![(a, Float), (b, Float), (c, Float)], ReturnKind::Native("Float"))` + add to `REGISTERED_INTRINSICS` + add to test list |
| `src/interpreter/intrinsics.rs` | Add `"Fma#"` arm: `arg_as_f64(args, 0)?, arg_as_f64(args, 1)?, arg_as_f64(args, 2)?` → `a.mul_add(b, c)` |
| `src/backend/llvm/intrinsics.rs` | Add `is_float_ternary` match for "Fma" → `@llvm.fma.f32`/`@llvm.fma.f64` (3 args) |
| `src/backend/llvm/normalizer.rs` | Add "Fma" to `STANDARD_OPS` |
| `src/backend/spirv/lower.rs` | Add `"Fma#"` arm: emit 3 args, call `builder.glsl_fma(ty_id, a, b, c)` |
| `src/backend/spirv/normalizer.rs` | Add "Fma#" to `build_supported_ops()` |
| `src/backend/ptx/general.rs` | Add `"Fma#"` in `emit_intrinsic_call`: emit 3 args, `fma.rn.f32 %out, %a, %b, %c` |
| `src/analysis/accel.rs` | Add `"Fma#"` to `expr_is_pure()` |
| `docs/reference/MASTER-SYNTAX-REFERENCE.md` | Add `Fma#` to math section |

### SubgroupBallot# — 9 files

| File | Change |
|------|--------|
| `src/intrinsic_signatures.rs` | Add signature `("SubgroupBallot#", params vec![(pred, Bool)], ReturnKind::Native("Int"))` + list + test |
| `src/interpreter/intrinsics.rs` | Add arm: return `Ok(i64_to_bits(1))` |
| `src/backend/spirv/lower.rs` | Add arm: `OpGroupNonUniformBallot` with Subgroup scope → extract component `.x` via `OpCompositeExtract` → cast u32 to Int via `OpUConvert` |
| `src/backend/spirv/normalizer.rs` | Add |
| `src/backend/ptx/general.rs` | Add in `emit_intrinsic_call`: `vote.sync.ballot.b32 %out, %pred, 0xFFFFFFFF` |
| `src/analysis/accel.rs` | Add to purity |
| `docs/reference/MASTER-SYNTAX-REFERENCE.md` | Add to GPU section |

### SubgroupBroadcast# — 9 files

| File | Change |
|------|--------|
| `src/intrinsic_signatures.rs` | Add signature `("SubgroupBroadcast#", params vec![], ReturnKind::Inferred)` + list + test |
| `src/interpreter/intrinsics.rs` | Add arm: return `args.first().cloned()` (identity) |
| `src/backend/spirv/lower.rs` | Add arm: `OpGroupNonUniformBroadcast` with Subgroup scope |
| `src/backend/spirv/normalizer.rs` | Add |
| `src/backend/ptx/general.rs` | Add in `emit_intrinsic_call`: `shfl.sync.idx.b32 %out, %val, %lane, 0x1F, 0xFFFFFFFF` |
| `src/analysis/accel.rs` | Add to purity |
| `docs/reference/MASTER-SYNTAX-REFERENCE.md` | Add to GPU section |

### SubgroupFMax# / SubgroupFMin# — PTX (1 file)

| File | Change |
|------|--------|
| `src/backend/ptx/general.rs` | Add in `emit_intrinsic_call`: `redux.sync.max.f32 %out, %v, 0xFFFFFFFF` / `redux.sync.min.f32 ...` |

### Docs — 2 files

| File | Change |
|------|--------|
| `docs/reference/MASTER-SYNTAX-REFERENCE.md` | Add Fma#, SubgroupBallot#, SubgroupBroadcast# |
| `docs/architecture/intrinsics-vs-stdlib.md` | Update GPU subgroup row with new intrinsics |

---

## SPIR-V emission details

### SubgroupBallot# — composite extract pattern

```rust
// 1. Create uvec4 type
let u32_ty = self.builder.u32_type();
let uvec4_ty = self.builder.type_vector(u32_ty, 4);

// 2. Emit ballot
let scope = self.builder.u32_const(spirv::Scope::Subgroup as u32);
let ballot = self.builder.group_non_uniform_ballot(uvec4_ty, res, scope, pred);

// 3. Extract .x component (lanes 0–31)
let x_idx = self.builder.u32_const(0);
let x_comp = self.builder.extract_dynamic(u32_ty, ballot, x_idx);

// 4. Cast u32 → i64 (Briev Int)
let int_ty = self.type_id(&Type::int())?;
let result = self.builder.u_convert(int_ty, x_comp);
```

### SubgroupBroadcast# — direct op

```rust
let scope = self.builder.u32_const(spirv::Scope::Subgroup as u32);
let res = self.builder.gen_id();
self.builder.emit(Instruction::new(
    spirv::Op::GroupNonUniformBroadcast,
    Some(ty_id),
    Some(res),
    vec![
        Operand::IdRef(scope),
        Operand::IdRef(v),
        Operand::IdRef(lane),
    ],
));
```

### Fma# — GLSL.std.450

```rust
let ty_id = self.type_id(&vty)?;
let res = self.builder.glsl_fma(ty_id, a, b, c);
```

---

## LLVM emission details

### Fma# — ternary float intrinsic

New `is_float_ternary` category (after `is_float_unary`):

```rust
let is_float_ternary = matches!(op_name, "Fma");
if is_float_ternary {
    let llvm_name = op_name.to_lowercase(); // "fma"
    let (float_suffix, float_llvm_ty, ret_ty) = match llvm_ty.as_str() {
        "double" => ("f64", "double", Type::float64()),
        _ => ("f32", "float", Type::float()),
    };
    writeln!(out, "{}{} = call {} @llvm.{}.{}({} {}, {} {}, {} {})",
        indent, v, float_llvm_ty, llvm_name, float_suffix,
        float_llvm_ty, arg_regs[0].name,
        float_llvm_ty, arg_regs[1].name,
        float_llvm_ty, arg_regs[2].name).ok();
    return BTypedRegister { name: v.to_string(), ty: ret_ty };
}
```

---

## PTX emission details

### Fma#

```ptx
fma.rn.f32 %out, %a, %b, %c;
```

### SubgroupBallot#

```ptx
vote.sync.ballot.b32 %out, %pred, 0xFFFFFFFF;
```

### SubgroupBroadcast#

```ptx
shfl.sync.idx.b32 %out, %val, %lane, 0x1F, 0xFFFFFFFF;
```

### SubgroupFMax# / SubgroupFMin#

```ptx
redux.sync.max.f32 %out, %v, 0xFFFFFFFF;
redux.sync.min.f32 %out, %v, 0xFFFFFFFF;
```

---

## Implementation order

1. **Fma#** — all backends (interpreter, LLVM, SPIR-V, PTX, accel, signatures)
2. **SubgroupBallot#** — all backends
3. **SubgroupBroadcast#** — all backends
4. **SubgroupFMax#/Min# PTX** — add `redux.sync` emission to `emit_intrinsic_call`
5. **Docs** — MASTER-SYNTAX-REFERENCE.md + intrinsics-vs-stdlib.md
6. **Test** — `cargo test --lib`
7. **Praetor** — changed files

---

## Risk assessment

- **Low risk**: Fma# — follows exact same pattern as Sqrt#/Sin#/Exp# for unary; ternary is trivial extension
- **Low risk**: SubgroupFMax#/Min# PTX — single `redux.sync` instruction, no shared memory
- **Medium risk**: SubgroupBallot# SPIR-V — requires composite extract + type cast (uvec4 → u32 → i64)
- **Low risk**: SubgroupBroadcast# — direct `OpGroupNonUniformBroadcast`, straightforward

---

## Success criteria

- `cargo test --lib` passes (2269+ tests)
- Fma# emits correct PTX (`fma.rn.f32`), SPIR-V (`GLSL.std.450 Fma`), LLVM (`@llvm.fma.f32`)
- SubgroupBallot# emits correct PTX (`vote.sync.ballot.b32`) and SPIR-V (`OpGroupNonUniformBallot`)
- SubgroupBroadcast# emits correct PTX (`shfl.sync.idx.b32`) and SPIR-V (`OpGroupNonUniformBroadcast`)
- SubgroupFMax#/Min# emit `redux.sync.max/min.f32` on PTX
- CPU fallback: Fma# uses `mul_add`, ballot returns 1, broadcast returns val
- Praetor on all changed files: no NEW diagnostics
