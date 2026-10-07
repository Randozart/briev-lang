# Webstack Intrinsic Policy

**Date:** 2026-07-26
**Status:** Active — applies to WASM-first webstack v2 (LlvmBackend wasm32 path)

## Overview

The webstack backend (LlvmBackend wasm32 + GlueWebGenerator) inherits the LLVM
backend's full intrinsic system (`src/backend/llvm/intrinsics.rs`, 1144 lines).
Every `#` intrinsic is emitted as LLVM IR and lowered to WASM by `llc`. However,
not all intrinsics make sense in the browser environment. This document
categorizes every intrinsic into one of four tiers.

## Tier 1: WASM Native — Fully Supported

These intrinsics lower directly to WASM instructions via LLVM. No JS shim
involvement. Zero overhead.

| Intrinsic | LLVM emission | WASM lowering |
|-----------|---------------|---------------|
| `Add#` | `add i{32,64}` | `i32.add` / `i64.add` |
| `Sub#` | `sub i{32,64}` | `i32.sub` / `i64.sub` |
| `Mul#` | `mul i{32,64}` | `i32.mul` / `i64.mul` |
| `Div#` | `sdiv i{32,64}` | `i32.div_s` / `i64.div_s` |
| `Rem#` | `srem i{32,64}` | `i32.rem_s` / `i64.rem_s` |
| `Neg#` | `sub 0, x` | `i32.sub` / `i64.sub` |
| `Abs#` | `llvm.abs` | WASM select pattern |
| `Eq#` | `icmp eq` | `i32.eq` / `i64.eq` |
| `Neq#` | `icmp ne` | `i32.ne` / `i64.ne` |
| `Lt#` | `icmp slt` | `i32.lt_s` / `i64.lt_s` |
| `Gt#` | `icmp sgt` | `i32.gt_s` / `i64.gt_s` |
| `Le#` | `icmp sle` | `i32.le_s` / `i64.le_s` |
| `Ge#` | `icmp sge` | `i32.ge_s` / `i64.ge_s` |
| `BitAnd#` | `and i{32,64}` | `i32.and` / `i64.and` |
| `BitOr#` | `or i{32,64}` | `i32.or` / `i64.or` |
| `BitXor#` | `xor i{32,64}` | `i32.xor` / `i64.xor` |
| `Shl#` | `shl i{32,64}` | `i32.shl` / `i64.shl` |
| `Shr#` | `ashr i{32,64}` | `i32.shr_s` / `i64.shr_s` |
| `BitNot#` | `xor -1, x` | `i32.xor` / `i64.xor` |
| `Not#` | `xor 1, x` | `i32.xor` / `i64.xor` |
| `Fabs#` | `llvm.fabs` | `f32.abs` / `f64.abs` |
| `Ceil#` | `llvm.ceil` | `f32.ceil` / `f64.ceil` |
| `Floor#` | `llvm.floor` | `f32.floor` / `f64.floor` |
| `Sqrt#` | `llvm.sqrt` | `f32.sqrt` / `f64.sqrt` |
| `Sin#` | `llvm.sin` | External call (libm) |
| `Cos#` | `llvm.cos` | External call (libm) |
| `Pow#` | `llvm.pow` | External call (libm) |

**Sin/Cos/Pow** lower to libm calls (`sin`, `cos`, `pow`) which WASI provides.
The WASI polyfill in the browser provides these via `Math.sin` etc.

## Tier 2: WASM LLVM Runtime — Supported via Runtime

These use LLVM builtins that WASM provides or are lowered to WASM ops by `llc`.
Some need linear memory management in the JS shim.

| Intrinsic | LLVM emission | WASM mechanism |
|-----------|---------------|----------------|
| `Ptr#` | `inttoptr` | WASM opaque reference type or i32 reinterpret |
| `Deref#` | `load` | Linear memory `i32.load` |
| `Index#` | `getelementptr` + `load` | Linear memory arithmetic + `i32.load` |
| `Cast#` | `bitcast` / `ptrtoint` / `inttoptr` | WASM reinterpret or no-op |
| `AddressOf#` | `ptrtoint` | Compile-time resolved or linear memory address |
| `Load#` | `inttoptr` + `load` | Linear memory read |
| `Store#` | `inttoptr` + `store` | Linear memory write |
| `Malloc#` | `call @malloc` | Requires WASM import — JS shim provides `malloc` |
| `Alloc#` | Alloc strategy (arena/alloca/malloc) | Requires stack or heap allocator |
| `Free#` | `call @free` | Requires WASM import — JS shim provides `free` |
| `Copy#` | `call @llvm.memcpy` | Lowered to `memory.copy` by `llc` |
| `Fill#` | `call @llvm.memset` | Lowered to `memory.fill` by `llc` |
| `AtomicLoad#` | `load atomic` | `i32.atomic.load` |
| `AtomicStore#` | `store atomic` | `i32.atomic.store` |
| `AtomicCas#` | `cmpxchg` | `i32.atomic.rmw.cmpxchg` |
| `AtomicXchg#` | `atomicrmw xchg` | `i32.atomic.rmw.xchg` |
| `AtomicAdd#` | `atomicrmw add` | `i32.atomic.rmw.add` |
| `Fence#` | `fence seq_cst` | `atomic.fence` |
| `Len#` / `Length#` | Load from struct header | Linear memory load |
| `Concat#` | External call | Requires WASM import — JS shim provides |
| `Get#` | External call | Requires WASM import — JS shim provides |
| `Insert#` | External call | Requires WASM import — JS shim provides |

### Malloc/Free requirements

The JS shim (`dom-shim.mjs`) must provide `malloc` and `free` as WASM imports.
Without these, `Malloc#` and `Free#` crash at runtime. The shim can use:

```javascript
// Minimal bump allocator in _buildImports()
malloc: (size) => {
  const ptr = this._heapPtr;
  this._heapPtr += Number(size);
  return ptr;
},
free: (ptr) => { /* no-op for bump allocator */ },
```

A production WASM allocator (dlmalloc, wee_alloc) can be compiled separately
and linked at the WASM level via `wasm-ld` — no JS changes needed.

## Tier 3: Browser API — Supported via JS Shim Imports

These intrinsics don't exist as WASM instructions but have direct browser
JavaScript equivalents. The LLVM backend emits them as external calls
(`call @IntrinsicName`), which become WASM imports. The JS shim provides
the implementation.

| Intrinsic | LLVM emission | JS shim implementation |
|-----------|---------------|----------------------|
| `Print#` (generic; type-dispatched) | `call @PrintInt` / `@PrintStr` / `@PrintFloat` / `@PrintChar` / `@PrintBool` | `(n) => console.log(n)` — the 2026-08-01 audit folded the four special-cased print intrinsics into one `Print#` |
| `Time#` | `call @Time` | `() => Date.now()` |
| `CpuCount#` | `call @CpuCount` | `() => navigator.hardwareConcurrency || 4` |
| `Hostname#` | `call @Hostname` | `() => window.location.hostname` |
| `PageSize#` | Constant 4096 | Hardcoded |
| `Errno#` | `call @Errno` | `() => 0` (no errno in JS) |
| `Sleep#` | `call @Sleep` | `(ms) => new Promise(r => setTimeout(r, Number(ms)))` |

**Migration path:** These intrinsics are superseded by `frgn from "glue/web/web.js"` + stdlib.
`PrintInt#` → `import "web/console.bv"` → `log(n)`.
`Time#` → `import "web/time.bv"` → `now()`.
The intrinsic forms are kept for backward compat during the migration period.

## Tier 4: Unsupported — Compile-Time Error

These intrinsics have no meaningful WASM or browser equivalent. Using them
in code compiled with the webstack backend produces:

```
Intrinsic '<name>' is not supported by the webstack/WebAssembly backend.
```

| Intrinsic | Reason |
|-----------|--------|
| `GetEnv#` | No environment variables in browser. Use `frgn from "glue/web/web.js"` + JS impl. |
| `GetEnvInt#` | Same — no environment variables. |
| `SysCall#` | No OS syscalls in WASM. No Linux `syscall` instruction. |
| `SysConf#` | No OS sysconf in WASM. |
| `DlOpen#` | No dynamic linking in WASM MVP. |
| `DlSym#` | No dynamic symbol lookup. |
| `DlClose#` | No dynamic library unloading. |
| `Backtrace#` | No stack trace intrinsic in WASM. |
| `ReadFile#` | No filesystem in browser WASM. Use `fetch()` via `frgn from "glue/web/web.js"`. |
| `HttpFetch#` | No HTTP intrinsic. Use `fetch()` via `frgn from "glue/web/web.js"`. |
| `ShellCmd#` | No shell in browser. |
| `SetStdoutBuf#` | No stdout buffer concept in browser. |
| `GetGlobalId#` | No GPU compute shader mapping in standard WASM. Stub would return 0. |
| `GetGlobalSize#` | Same — no GPU compute context. |
| `GetLocalId#` | Same. |
| `GetGroupId#` | Same. |
| `GetNumGroups#` | Same. |
| `WorkgroupSize#` | Same. |
| `Dims#` | Same. |
| `StrSplit#` | No WASM string model. Use `import "string.bv"` at the Briev level. |
| `EnvGet#` | No environment. Use `frgn from "glue/web/web.js"` to expose JS env vars if needed. |
| `SysQuery#` | No system query mechanism. |
| `TimeNow#` | No nanosecond clock. Use `time.bv` `now()` for milliseconds. |

### GPU intrinsics special note

GPU compute intrinsics (`GetGlobalId#`, etc.) COULD be supported in the future
when targeting WebGPU compute shaders via WASM. This would require the CIRCT
backend or a SPIR-V pipeline, not the LLVM wasm32 target. They are unsupported
in the current webstack v2 but not architecturally impossible.

## Implementation in Compiler

The LLVM backend already handles all intrinsics — Tier 1-3 continue to work
as-is. Tier 4 requires an additional check in the webstack codegen path.

### Where to add the check

In `src/backend/llvm/intrinsics.rs`, the `emit_intrinsic_call()` function is
the single dispatch point. Add a WASM-specific rejection gate early:

```rust
// 2026-07-26: Webstack/WASM intrinsic policy.
// Tier 4 intrinsics produce a compile error when targeting WASM.
if backend.ctx.webstack_enabled {
    if let Some(msg) = wasm_unsupported_intrinsic(name) {
        return BTypedRegister::error(format!(
            "Intrinsic '{}' is not supported by the webstack/WebAssembly backend. {}",
            name, msg
        ));
    }
}
```

Where `wasm_unsupported_intrinsic()` returns `Some(reason)` for Tier 4
intrinsics and `None` for Tiers 1-3.

### Error format

```
error: Intrinsic 'SysCall#' is not supported by the webstack/WebAssembly backend.
  No OS syscalls in WASM. Use frgn from "glue/web/web.js" to call browser APIs.
  --> my_file.bv:12:5
```

### When `webstack_enabled` is false (native LLVM target)

No restriction — all intrinsics work as before.

## Summary Table

| Tier | Count | Mechanism | Example |
|------|-------|-----------|---------|
| 1 — WASM native | 29 | WASM instruction lowering | `Add#`, `Sqrt#`, `Eq#` |
| 2 — Runtime | 19 | Linear memory + WASM imports | `Malloc#`, `Copy#`, `AtomicLoad#` |
| 3 — Browser API | 9 | JS shim import stubs | `PrintInt#`, `Time#`, `CpuCount#` |
| 4 — Unsupported | 18 | Compile-time error | `SysCall#`, `DlOpen#`, `GetEnv#` |

Total: 75 intrinsic variants across all tiers. All 63 registered intrinsics
plus 12 unregistered-but-referenced variants are categorized.
