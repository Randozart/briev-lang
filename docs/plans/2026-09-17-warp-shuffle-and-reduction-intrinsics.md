# Warp Shuffle & Reduction Intrinsics + Exp# Generalization

**Date:** 2026-09-17
**Status:** Active
**Motivation:** Flash attention requires warp-level reductions (row-max, row-sum) and fast exp for softmax. These are GPU-unique hardware primitives that cannot be expressed in pure `.bv` code — they must be compiler intrinsics. Exp# is broken on LLVM/interpreter and needs generalization over float types.

---

## What we're adding

### New intrinsics (GPU-unique hardware primitives)

| Intrinsic | Signature | PTX | SPIR-V | LLVM (CPU) |
|-----------|-----------|-----|--------|------------|
| `ShuffleDown#(val, delta) -> val` | `(T, Int) -> T` | `shfl.down.sync 0xFFFFFFFF, val, delta, 31` | `OpGroupNonUniformShuffleDown` | No-op (identity for single-lane) |
| `ShuffleXor#(val, lane_mask) -> val` | `(T, Int) -> T` | `shfl.xor.sync 0xFFFFFFFF, val, lane_mask, 31` | `OpGroupNonUniformShuffleXor` | No-op (identity for single-lane) |
| `SubgroupFMax#(v) -> Float` | `(Float) -> Float` | `red.max.s32` pattern (PTX lacks direct subgroup max; use shared memory + shuffles) | `OpGroupNonUniformFMax` | No-op (identity for single-lane) |
| `SubgroupFMin#(v) -> Float` | `(Float) -> Float` | Same as above | `OpGroupNonUniformFMin` | No-op (identity for single-lane) |

### Exp# generalization

| Backend | Current | After |
|---------|---------|-------|
| Interpreter | **Missing entirely** | Add `Exp#` with `arg_as_f64` + `.exp()` |
| LLVM | **Missing** — falls through to broken `emit_external_call` | Add `"Exp"` to `is_float_unary` match → emits `@llvm.exp.f64`/`@llvm.exp.f32` |
| SPIR-V | Already generic | No change |
| Webstack | **Missing** from supported ops | Add `"Exp#"` to supported set |
| Type signature | `Native("Float")` hardcoded | Keep `Native("Float")` — same as Sqrt#/Sin#/Cos# |

---

## Files to modify

### New intrinsics — each needs changes in 4-5 files:

| File | Change |
|------|--------|
| `src/intrinsic_signatures.rs` | Add 4 signature arms + `REGISTERED_INTRINSICS` entries |
| `src/backend/spirv/normalizer.rs` | Add 4 to `build_supported_ops()` |
| `src/backend/spirv/lower.rs` | Add 4 match arms in `emit_intrinsic_call()` |
| `src/backend/ptx/general.rs` | Add intrinsic call emission (currently no Call handling) |
| `src/analysis/accel.rs` | Add 4 to `expr_is_pure()` |
| `src/backend/llvm/normalizer.rs` | Do NOT add (GPU-only) |

### Exp# fixes — 4 files:

| File | Change |
|------|--------|
| `src/intrinsic_signatures.rs` | No change (already registered) |
| `src/interpreter/intrinsics.rs` | Add `"Exp#"` arm |
| `src/backend/llvm/intrinsics.rs` | Add `"Exp"` to `is_float_unary` match |
| `src/backend/llvm/normalizer.rs` | Add `"Exp"` to `STANDARD_OPS` |
| `src/backend/webstack/normalizer.rs` | Add `"Exp#"` to supported ops |

---

## Implementation order

1. **Exp# fixes** (smallest, unblocks everything else)
   - Interpreter: add missing arm
   - LLVM: add to `is_float_unary` + `STANDARD_OPS`
   - Webstack: add to supported ops
   - Test: `cargo test --lib`

2. **SubgroupFMax# / SubgroupFMin#** (follows existing SubgroupFAdd# pattern exactly)
   - Signatures, SPIR-V emission, accel purity
   - PTX: shared memory reduction pattern
   - Test: `cargo test --lib`

3. **ShuffleDown# / ShuffleXor#** (new pattern — no existing reference)
   - Signatures, SPIR-V emission, accel purity
   - PTX: `shfl.down.sync` / `shfl.xor.sync` emission
   - Test: `cargo test --lib`

4. **PTX intrinsic dispatch** (prerequisite for intrinsics in PTX kernels)
   - Add `Call` handling to `general.rs` for intrinsic calls
   - Currently the general emitter only handles `Identifier`, `Decimal`, `Float`, `Index`, `BinaryOp`, `UnaryOp`

5. **Docs update** (architecture docs)
   - `docs/architecture/intrinsics-vs-stdlib.md` — add new intrinsics
   - `docs/architecture/agent-reference.md` — add new intrinsics to reference table

---

## PTX emission details

### ShuffleDown#

```ptx
shfl.down.sync.b32 %out, %val, %delta, 31;
// For f32: shfl.down.sync.b32
// For f16: pack into u32, shuffle, unpack
```

Mask `0xFFFFFFFF` = all lanes participate. Width `31` = max delta.

### ShuffleXor#

```ptx
shfl.xor.sync.b32 %out, %val, %mask, 31;
```

### SubgroupFMax# / SubgroupFMin#

PTX doesn't have a direct subgroup max/min. Implementation:
1. Each lane writes its value to shared memory
2. `bar.sync 0`
3. Tree reduction in shared memory using `max.f32` / `min.f32`
4. Read result from lane 0

This is slower than a warp shuffle reduction but correct. For flash attention, the SPIR-V path is the primary target; PTX can be optimized later with `red.max.s32` patterns or cooperative groups.

### LLVM CPU fallback

For single-lane CPU execution, all shuffles are identity (return the input value). The LLVM backend emits:
```llvm
%out = add <ty> %val, 0  ; no-op
```

---

## Risk assessment

- **Low risk**: Exp# fixes — purely additive, existing patterns to follow
- **Low risk**: SubgroupFMax/Min — exact same pattern as SubgroupFAdd#
- **Medium risk**: ShuffleDown/Xor — new pattern, but PTX and SPIR-V have well-documented instructions
- **Medium risk**: PTX intrinsic dispatch — the general emitter needs a new code path for `Expr::Call`
- **No risk**: Doc updates — purely additive

---

## Success criteria

- `cargo test --lib` passes (all existing tests + new intrinsic tests)
- Exp# works on all backends (interpreter, LLVM, SPIR-V)
- SubgroupFMax#/Min# emit correct SPIR-V
- ShuffleDown#/Xor# emit correct PTX and SPIR-V
- CPU fallback is identity (no-op) for shuffles
- Praetor on all changed files: no new diagnostics
