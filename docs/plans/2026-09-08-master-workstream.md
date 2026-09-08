# Master Workstream Plan

**Date:** 2026-09-08
**Status:** Phases 1-6 COMPLETE. Phase 7 (PTX tier) and Phase 8 (housekeeping) remain.
**Context:** GEMM is at HW peak (24.3 TFLOP/s, 95% of RTX 3060 FP16-accum tensor
peak — see `docs/plans/2026-09-08-gemm-occupancy-campaign.md`). All portable GPU
levers exhausted. The remaining work is the optional PTX tier and housekeeping.

---

## Completed

| Phase | Status | Commit | Notes |
|-------|--------|--------|-------|
| 1a — P1 strength reduction | DONE | `dd5f5e26` | 156→11 div/mod (145 eliminated). u32_and helper + masks. |
| 1b — P0 prefetch re-enable | DONE | `335d028d` | Config already had prefetch=1. |
| 2 — Pointer arithmetic intrinsics | DONE | pre-existing | PtrAdd#/PtrSub#/PtrDiff#/PtrEq#/PtrLt# all registered + emitted. |
| 3 — Atomic ordering keywords | DONE | pre-existing | relaxed/acquire/release/bartered + parameterized intrinsics. |
| 4 — Portable SIMD intrinsics | DONE | pre-existing | SimdAdd#/SimdSub#/SimdMul#/SimdFma# (memory-to-memory). |
| 5 — ISR handlers + linker sections | DONE | pre-existing | isr[<mech>] handler @, section placement, body restrictions. |
| 6 — noalias + test alignment | DONE | `12c0b3b8`, `d0220812` | Dispatch change + 4 dead tests deleted + 8 tests fixed. |

---

## Phase 1: GPU Quick Wins (DONE)

Low effort, high confidence, zero risk.

### 1a — P1: Strength-Reduce Power-of-Two Div/Mod

**File:** `src/backend/spirv/gemm.rs`
**Plan:** `docs/plans/2026-09-06-p0-p1-implementation.md` §P1
**Change:** Replace `OpUDiv`/`OpUMod` by power-of-two constants with
`OpShiftRightLogical`/`OpBitwiseAnd`. Eliminates ~891 div/mod instructions.

**Steps:**
1. Add `u32_shr` helper (after `u32_binop`, ~line 55)
2. Replace divisors in `emit_smem_fill` (scalar path)
3. Replace divisors in `emit_fill_pair_dram`
4. Replace divisors in `emit_fill_pair_smem`
5. Replace divisors in quad fill functions

**Gate:** `spirv-val` clean, `cargo test --lib` green, GPU time unchanged,
spirv-dis shows shift/and replacing div/mod.

### 1b — P0: Re-Enable D2 Prefetch

**File:** `config/ir-lowering.dbvl`
**Plan:** `docs/plans/2026-09-06-p0-p1-implementation.md` §P0
**Change:** Flip `spirv_coopmat_fill_prefetch: 0 → 1`. Prerequisite
(`spirv_coopmat_fill_pairs: 1`) already met.

**Gate:** Correctness gate (ycmp.c A/B vs baseline), GPU timestamp measurement.

---

## Phase 2 (DONE): Pointer Arithmetic (1 session)

Phase 1 of `docs/plans/2026-09-06-cpp-expressiveness.md`. No dependencies.

### 2a — Intrinsics

New intrinsics with GEP emission:

| Intrinsic | Signature | LLVM Emit |
|-----------|-----------|-----------|
| `PtrAdd#` | `(Ptr<T>, Int) -> Ptr<T>` | `getelementptr inbounds` |
| `PtrSub#` | `(Ptr<T>, Int) -> Ptr<T>` | `getelementptr inbounds` (neg offset) |
| `PtrDiff#` | `(Ptr<T>, Ptr<T>) -> Int` | GEP subtraction → `ptrdiff_t` |
| `PtrEq#` | `(Ptr<T>, Ptr<T>) -> Bool` | `icmp eq ptr` |
| `PtrLt#` | `(Ptr<T>, Ptr<T>) -> Bool` | `icmp ult ptr` |

**Files:** `intrinsic_signatures.rs`, `emit_toplevel.rs`/`emit_expr.rs`
**Gate:** `cargo test --lib`, LLVM IR golden test per intrinsic.

### 2b — Operator Sugar

`op Add(Int)` / `op Sub(Int)` / `op Sub(Ptr<T>)` on `Ptr<T>`.
**Files:** parser (if `impl Ptr<T>` supported), typechecker.
**Gate:** Parse + typecheck tests.

### 2c — Docs

Update `spec/SPEC.md` §14.2, §15.2 and `docs/architecture/agent-reference.md`.

---

## Phase 3 (DONE): Atomic Ordering (1-2 sessions)

Phases 3-5 of cpp-expressiveness. Context-sensitive keywords + parameterized intrinsics.

### 3a — Lexer Keywords

Add `relaxed`, `acquire`, `release`, `bartered` as context-sensitive tokens
(valid only before `atomic`). `seq` already exists.

**Files:** `src/lexer.rs`, `src/vocab.rs`
**Gate:** Parse round-trip tests (valid + invalid contexts).

### 3b — Parser + AST

Parser: ordering prefix on `atomic` declarations.
AST: ordering field on atomic declarations.

**Files:** `src/parser/`, `src/ast/top.rs`
**Gate:** Parse tests, equality + display tests.

### 3c — Typecheck

Ordering inheritance on field access (`obj.f = obj.f + 1` on
`bartered atomic` → `atomicrmw add ... acq_rel`).

**Files:** `src/typechecker/mod.rs`
**Gate:** Typecheck tests.

### 3d — Parameterized Intrinsics

Existing intrinsics gain ordering parameter:

| Intrinsic | New signature |
|-----------|---------------|
| `AtomicLoad#` | `(ptr, Relaxed)` |
| `AtomicStore#` | `(ptr, val, Release)` |
| `AtomicCas#` | `(ptr, old, new, Bartered, Relaxed)` |
| `AtomicXchg#` | `(ptr, val, Bartered)` |
| `AtomicAdd#` | `(ptr, val, Bartered)` |
| `Fence#` | `(Acquire)` |

New atomics: `AtomicSub#`, `AtomicOr#`, `AtomicAnd#`, `AtomicXor#`, `AtomicLoadN#`.

**Files:** `intrinsic_signatures.rs`, `emit_expr.rs`
**Gate:** LLVM IR golden tests.

### 3e — Backward Compat + Docs

Default = `seq` (no keyword). All existing code unchanged.
Update `spec/SPEC.md` §19.x.

---

## Phase 4 (DONE): Portable SIMD (1-2 sessions)

Phase 6 of cpp-expressiveness. Memory-to-memory ABI.

### 4a — Intrinsics

`SimdAdd#`, `SimdSub#`, `SimdMul#`, `SimdFma#` — all memory-to-memory:
`(dst: Ptr<T>, a: Ptr<T>, b: Ptr<T>, count: Int)`.

**Files:** `intrinsic_signatures.rs`, `emit_expr.rs`
**Gate:** LLVM IR: `<4 x float>` chunks on AVX, scalar fallback, `#?` remark.

### 4b — Target Detection

Wire `target_features` from LLVM target machine to intrinsic lowering.

**Files:** `roofline.rs`, `config/targets.toml`
**Gate:** Correct resolution per target.

### 4c — SIMD Keywords DEFERRED

`simd`/`nosimd` duplicate existing surface. `simd<N>` needs loop-metadata
plumbing — deferred until a benchmark demonstrates a width-pinning win.

---

## Phase 5 (DONE): ISR Handlers + Linker Sections (2-3 sessions, high complexity)

Phases 8-9 of cpp-expressiveness. Signed off. Design in
`docs/plans/2026-09-06-isr-handlers-and-sections.md`.

### 5a — Config + Loader

`config/isr-targets.dbvl` (mechanism rows) + `IsrMechanism` struct + loader.
**Files:** `config/`, `src/target.rs`

### 5b — Lexer + Parser + AST

Lexer: `Isr` token. Parser: `isr[<mech>] handler @ (lit|Name): name() { ... }`.
AST: `TopLevel::IsrHandler(IsrHandler)`.
**Files:** `src/lexer.rs`, `src/parser/definitions.rs`, `src/ast/top.rs`

### 5c — Typecheck

Mechanism resolution (explicit → config → error), named-vector resolution,
duplicate-vector check, body restrictions (no Malloc#, no Float, no Spawn#,
bounded frame).
**Files:** `src/typechecker/mod.rs`

### 5d — Board Files + Resolver

`lib/boards/<board>/interrupts.dbvl` + resolver lookup.
**Files:** `src/address_resolver.rs`

### 5e — Backend

Fill `EmbeddedConfig.interrupts`, emit vector table global + per-mechanism
prologue/epilogue, `section(".isr_vector")` on table.
**Files:** `src/backend/llvm/mod.rs`, `emit_toplevel.rs`

### 5f — General Section Placement (scoped)

`section(".name")` on `defn` and `const` only (not `let`/state fields).
Proof obligation: sectioned defn must not allocate.
**Files:** parser, typechecker, backend

### 5g — Tests + Docs

Golden-file test per mechanism, parse round-trip (3 forms), restriction
proofs. Update SPEC §13.2, agent-reference, backend-contracts.

---

## Phase 6 (DONE): noalias + Test Alignment + FFI Audit (1 session)

From `docs/plans/2026-09-07-noalias-benchmarking-and-test-alignment.md`.

### 6a — Delete Dead Tests

4 files: `ffi_comprehensive_tests.rs`, `ffi_stdlib_tests.rs`, `test_rust.rs`, `test_c.rs`.

### 6b — Fix 8 Broken Tests

API alignment: Parser, TypeChecker, ProofEngine constructors.

### 6c — noalias Dispatch Change

Stop inlining `@txn_*` in multi-txn ticks (`dispatch.rs:122-157`).
Restore `noalias nocapture` boundary for LLVM mem2reg/SROA.

### 6d — FFI Audit (read-only)

Checklist: BoundaryOwnership, ResolvedFrgn dispatch, GLUE config loading,
export wrappers, gen.bv escape hatch, metropipe.

### 6e — Benchmark + Document

A/B benchmark, document in `benchmarks/results/`.

---

## Phase 7: PTX Tier (3-5 sessions, high effort)

From `docs/plans/2026-09-04-beyond-coopmat.md` Stage 2. Only if 42.0 TFLOP/s
target matters.

| Sub | Task | Files |
|-----|------|-------|
| S1 | CUDA driver module | `lib/runtime/briev_dev_cuda.c` |
| S2 | PTX emitter | `src/backend/ptx/`, `capabilities.rs` |
| S3 | Tensor GEMM: `mma.sync.aligned.m16n8k16`, `ldmatrix`, `cp.async` | S2 |
| S4 | Correctness gate | Per-op microtests, shape portfolio |
| S5 | Performance gate: match 42.0 TFLOP/s | Ledger row |
| S6 | Auto-tune loop | `derive --stochastic` |
| Docs | backend-contracts, HANDOFF, SPEC §9.8, AGENTS.md | Same commit as S2 |

---

## Phase 8: Housekeeping (parallel, low priority)

| Task | Status | Notes |
|------|--------|-------|
| Protocol proof codec bodies | DONE (archived) | `protocols.bv.archive` removed all active bindings; hard-error gate active at `protocol_graph.rs:176` |
| `brievc run x.abv` native subcommand | DONE (pre-existing) | Native in-process runner at `compile.rs:1510` — no shell wrapper needed |
| Resident-launch policy for `.bv` offload | DONE (pre-existing) | `analyze_resident_safety()` at `accel.rs:1482`, consumed by LLVM backend at `mod.rs:2354` |
| Old plan file review/closure | DONE | Curated status index at `docs/plans/INDEX.md` — live vs closed vs historical |
| HashMap rehash PHI verification warnings | Known, non-blocking |
