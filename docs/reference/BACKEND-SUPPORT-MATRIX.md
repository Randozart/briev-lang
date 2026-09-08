# Backend Feature-Support Matrix

**Date:** 2026-09-08
**Authoritative source:** `src/backend/capabilities.rs` (`BackendCapabilities`),
per-backend `CAPABILITIES` declarations, and the emission sites surveyed in
`src/backend/{llvm,vm,spirv,circt}/`.

Legend: **✅** supported · **⚠️** partial/stub/silent (see note) · **❌**
rejected with compile error (capability gate or `record_unsupported`) · **—
** not applicable.

> **Gate note.** Every backend except Webstack runs `validate_program()`
> pre-codegen (compile.rs:1183-1234). A flag set to `false` = hard compile
> error with what/why/fix — never a silent drop. The hazards below are the
> *remaining* gaps where a flag is true but emission is a stub or no-op, or
> where the gate is skipped.

---

## 1. Expression surface

| Feature | LLVM | VM | SPIR-V | CIRCT | Webstack |
|---|---|---|---|---|---|
| Int literals | ✅ | ✅ | ✅ | ✅ | ✅ |
| Float literals + arith | ✅ | ❌ | ✅ | ⚠️ rejected pre-codegen (`floats:false`) but `hw.constant f64` arm exists | ✅ |
| String literals + concat | ✅ | ❌ | ❌ | ❌ | ✅ |
| Bool/Char literals | ✅ | ✅ | ✅ | ✅ | ✅ |
| Integer ops | ✅ | ✅ | ✅ | ✅ | ✅ |
| Unary ops | ✅ | ✅ | ✅ | ✅ | ✅ |
| Function calls | ✅ | ✅ | ❌ (intrinsic-only) | ⚠️ cells only | ✅ |
| Intrinsics | ✅ | ⚠️ host-call only | ⚠️ small set | ⚠️ 3 only | ⚠️ tiered whitelist |
| if/else expr | ✅ | ✅ | ✅ | ❌ (declared true, no arm → error) | ✅ |
| match expr | ✅ | ⚠️ int patterns only | ⚠️ Bool scrutinee only | ❌ (declared true, no arm) | ✅ |
| block expr | ✅ | ✅ | ❌ | ❌ | ✅ |
| field access | ✅ | ✅ | ❌ | ⚠️ `hw.wire` identity stub | ✅ |
| index `a[i]` | ✅ | ✅ | ⚠️ state fields only | ⚠️ state arrays only; non-state → stub | ✅ |
| slices / ranges | ✅ | ❌ | ⚠️ | ❌ | ✅ |
| tuple/list literals | ⚠️ bare list panics | ❌ | ❌ | ✅ (tuple) | ✅ |
| struct literal | ✅ | ❌ | ❌ | ❌ | ✅ |
| lambda | ✅ | ❌ | ❌ | ❌ | ✅ |
| casts | ✅ | ⚠️ no-op identity | ✅ | ⚠️ extract/passthrough | ✅ |
| is_type | ⚠️ constant-true stub | ❌ | ❌ | ❌ | ⚠️ stub |
| deref / addr-of | ✅ | ⚠️ deref ✅, addr-of no-op | ❌ | ⚠️ passthrough | ✅ |
| spawn | ✅ | ❌ | ❌ | ❌ | ✅ |
| await | ✅ | ❌ | ❌ | ❌ | ✅ |
| method calls | ✅ | ❌ | ❌ | ❌ | ✅ |
| reflect `.^`/`.^^` | ⚠️ partial | ❌ | ❌ | ❌ | ✅ |
| plugin intercept | ✅ | ❌ | ❌ | ❌ | ✅ |
| derivation blocks | ⚠️ constant-0 stub | ❌ | ❌ | ❌ | ⚠️ stub |
| within expr | ⚠️ deadline dropped | ❌ | ❌ | ❌ | ⚠️ |
| obj ports | ✅ | ❌ | ❌ | ❌ | ✅ |
| cells | ❌ (declared false) | ❌ | ❌ | ✅ | ❌ (inherits LLVM false) |
| extern cells | ❌ (declared false) | ❌ | ❌ | ✅ | ❌ |

---

## 2. Statement surface

| Feature | LLVM | VM | SPIR-V | CIRCT | Webstack |
|---|---|---|---|---|---|
| let | ✅ | ✅ | ✅ | ⚠️ cell body: value not stored | ✅ |
| assign | ✅ | ⚠️ unknown target → silent runtime trap | ✅ | ✅ (scalar/regfile/memmacro) | ✅ |
| arrow assign `<-`/`~<-` | ✅ | ❌ | ❌ | ❌ | ✅ |
| guarded `when`/`[cond]` | ✅ | ✅ | ❌ | ✅ (gate stack) | ✅ |
| term / endprogram | ✅ | ✅ | ✅ | ❌ (declared true, no arm) | ✅ |
| break | ✅ | ❌ | ❌ | ❌ | ✅ |
| trap | ✅ | ❌ | ❌ | ❌ (declared true, no arm) | ✅ |
| halt | ✅ | ❌ | ❌ | ❌ | ✅ |
| match stmt | ✅ | ⚠️ int patterns | ❌ | ❌ (declared true, no arm) | ✅ |
| foreach | ✅ | ❌ | ✅ (range only) | ❌ | ✅ |
| inline asm | ⚠️ SILENT DROP (`inline_asm:true`, no arm) | ❌ | ❌ | ❌ | ⚠️ |
| concurrency sections | ✅ (sync/mutex emit inline; FIXED 2026-09-08) | ❌ | ❌ | ❌ | ⚠️ |
| defer | ✅ | ❌ | ❌ | ❌ | ✅ |
| lifetime hints (free/keep) | ✅ | ❌ | ❌ | ❌ | ✅ |
| metadata assign | ⚠️ SILENT DROP (`metadata_assign:true`, no arm) | ❌ | ❌ | ❌ | ⚠️ |
| rollback | ✅ | ❌ | ❌ | ❌ | ✅ |
| gate `[cond];` | ✅ | ❌ | ❌ | ❌ | ✅ |
| trg bindings | ⚠️ SILENT DROP (`trg_bindings:true`, no arm) | ❌ | ❌ | ❌ | ⚠️ |
| yield | ✅ (no-op, eager model) | ❌ | ❌ | ❌ | ✅ |
| check | ⚠️ not emitted (assert deferred) | ❌ | ❌ | ⚠️ obligation ports | ⚠️ |

---

## 3. Intrinsics truly emitted (by backend)

### LLVM
Full registry + templates: `Add# Sub# Mul# Div# Rem# Neg# Abs#` (int+float),
`Eq# Neq# Lt# Gt# Le# Ge#`, `BitAnd# BitOr# BitXor# Shl# Shr# BitNot#`,
`BitReverse# Popcount# LeadingZeros# TrailingZeros#`, `Not#`,
`Deref# Index# Ptr# PtrAdd# PtrSub# PtrDiff# PtrEq# PtrLt#`,
`Capacity# Resize# EnsureCap# TrimCap#`, `Sqrt# Sin# Cos# Fabs# Ceil# Floor#`
(+ `Exp#` external), `Print#` (category dispatch), `Malloc# Alloc# Free# Load#
Store# VolatileLoad# VolatileStore# Copy# Fill#`, `Spawn# SpawnWithOutput#
SetEnv# GetCwd# ChDir#`, `CallPtr#`, `CancelRequested# ClearCancel#`,
`GetGlobalId#`, `AddressOf#`, `SysCall# SysConf#`, full atomics family,
`SimdAdd#/Sub#/Mul#/Fma#`, `Fence#`, `DlOpen# DlSym# DlClose#`,
`Backtrace#`, generative op dispatch (`At# Slice# InsertAt# ExtractFrom#
CopyFrom# Append# Prepend# Count# Iter# Step# IsEnd# Current#`).

**Fall-through → external `@name` call** (link-time failure if no runtime
symbol): `Concat# Length# Get# Insert# GetGlobalSize# GetLocalId# Pow# ToInt#
ToFloat# ToString# StrSplit# EnvGet# SysQuery# TimeNow# HttpFetch# ShellCmd#`
and any unregistered name.

**CPU no-op**: `Barrier#` → `add i64 0, 1` (single thread).

### VM
Only `Print#`/`PrintInt#` (host id 0), `Log#` (host id 1). Everything else →
host-service call; tamer rejects unknown host ids at runtime.

### SPIR-V (kernel-scoped)
`SubgroupFAdd#` (OpGroupNonUniformFAdd), `Exp# Sqrt# Fabs#` (GLSL.std.450),
`GetGlobalId# GetLocalId#` (builtins), `WorkgroupSize#` (constant),
`Load# Store#` (SSBO access-chain). GEMM frontend emits raw
`OpControlBarrier`/`OpImageWrite` internally.

**Normalizer/emitter mismatch**: normalizer allows `Malloc# Free# Print#
Add#…` but emitter rejects them; emitter handles `GetGlobalId# Load# Store#`
that the normalizer doesn't allow (kernel.rs bypasses for builtins).

### CIRCT
`Abs#`, `AddressOf#` (from `address_resolver`), `Size#` (constant 64). All
other intrinsics → recorded error.

**Normalizer/emitter mismatch**: normalizer allows `Add#… SubgroupFAdd#` etc.
but emitter only emits the 3 above.

### Webstack (LLVM wasm32 + tiered whitelist)
Tier 1 (WASM native): `Add# Sub# Mul# Div# Rem# Neg# Abs# Eq# Neq# Lt# Gt#
Le# Ge# BitAnd# BitOr# BitXor# Shl# Shr# BitNot# Not# Fabs# Ceil# Floor#
Sqrt# Sin# Cos# Pow#`.
Tier 2 (WASM runtime): `Ptr# Deref# Index# Cast# AddressOf# Load# Store#
Malloc# Alloc# Free# Copy# Fill# Memcpy# Memset# Len# Length# Concat# Get#
Insert# ToInt# ToFloat# ToString# AtomicLoad# AtomicStore# AtomicCas#
AtomicXchg# AtomicAdd# Fence#`.
Tier 3 (browser API): `PrintInt# PrintFloat# PrintChar# Print# Time# CpuCount#
Hostname# PageSize# Errno# Sleep#`.

Non-whitelisted → hard compile error.

---

## 4. Known gaps / hazards (the "has no implementation" ledger)

### LLVM
| Gap | Type |
|---|---|
| `InlineAsm`, `MetadataAssignment`, `TrgBinding` | **SILENT DROP** — declared true, no emit arm (emit_stmt.rs catch-all) |
| `IsType` | constant-true stub |
| `DerivationBlock`/`FormattingAnnotation` | constant-0 stub |
| `Expr::Within` | deadline/fallback discarded; only inner expr emitted |
| `Check` statement | not emitted (assert deferred) |
| non-string/non-vector `Slice` | returns base array (silent) |
| `Range` as scalar value | panic |
| bare `List` literal | panic (compiler-bug guard) |
| unresolved `PluginIntercept`/`Exists` | panic |
| strided/f64 vector slice, mask-index on f64 vector | panic |
| `dyn Trait` | panic (staged, thunk-table ABI not landed) |
| runtime `Size`/`Len` reflection | panic (deleted/staged) |

### VM
| Gap | Type |
|---|---|
| Unknown assign target / identifier target | bare runtime trap, **no compile error** |
| non-int-literal match patterns | silently fall through to body |
| struct-literal construction | rejected (`struct_literal:false`) — struct *types* work, construction doesn't |
| unknown identifier / fn / intrinsic | `record_unsupported` → compile error |

### SPIR-V
| Gap | Type |
|---|---|
| `Malloc# Free# Print#` | pass normalizer, die in emitter (mismatch) |
| `Barrier#` | no user intrinsic (GEMM emits raw barrier internally) |
| user function calls | error |
| `match` non-Bool, `if` without else | error |
| non-state-field index | error |

### CIRCT
| Gap | Type |
|---|---|
| cell-body `_ => {}` | **SILENT DROP** (circt/mod.rs:1787) |
| `format_init_expr` `_ => "0"` | non-scalar init silently becomes 0 |
| `emit_contract_condition` `_ => true` | unsupported contract treated trivially true |
| `mlir_type` `None => i64` | unresolvable type silently i64 |
| operand fallback `%0` | failed emission silently substituted |
| `Field` / non-state `Index` | `hw.wire` identity stub |
| declared-true-but-no-arm | `if_expr`, `match_expr`, `term_endprogram`, `match_stmt`, `trap_stmt` → recorded error (honest) |

### Webstack
| Gap | Type |
|---|---|
| capability gate **skipped** | `validate_program` never runs (not in the gate list); LLVM full() surface assumed |
| `cells` / `extern_cells` | inherited false from LLVM, but ungated |

---

## 5. Per-backend charter

| Backend | Nature | Entry |
|---|---|---|
| **LLVM** | full native target | `LlvmBackend::generate` |
| **VM** | integer-stack tamer (`.lair`) | `VmBackend::generate` |
| **SPIR-V** | GPU kernels (`.spv`), accel items only | `compile_spirv` (kernel-scoped gate) |
| **CIRCT** | hardware netlist (`.mlir`), cells + watchdog | `CirctBackend` (whole-program + `record_unsupported`) |
| **Webstack** | WASM via LLVM wasm32 + JS shim | `LlvmBackend` with `with_webstack(true)`; intrinsic whitelist only gate |

Sources: `src/backend/capabilities.rs`, `src/backend/llvm/{capabilities,mod,emit_expr,emit_stmt,intrinsics}*.rs`,
`src/backend/vm/{mod,emit_expr,emit_stmt}.rs`, `src/backend/spirv/{mod,lower}.rs`,
`src/backend/circt/mod.rs`, `src/backend/webstack/{mod,normalizer}.rs`.