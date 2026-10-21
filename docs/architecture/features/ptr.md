# `Ptr<T>` — Typed Memory Addresses

**Date added:** 2026-06-25  
**Last updated:** 2026-10-20  
**Status:** Active (Phases 1-6 complete)

## Purpose

`Ptr<T>` is a first-class type representing a typed memory address. It carries
the address value at runtime and the pointee type `T` at the type level, enabling
contract-proven safe MMIO and memory access without a borrow checker or GC.

Pointer access is explicit: raw buffers use `p[i]` / `Load#` / `Store#`, and
hardware-volatile access uses `VolatileLoad#` / `VolatileStore#`. All access
is contract-verified; ordinary heap pointers are not volatile.

## Type representation

### Four pointer forms (2026-07-03)

Briev has four forms of pointer, each differing in how much information the
type system has about the pointee:

| Form | Example | Pointee info | Use case |
|------|---------|--------------|----------|
| Typed pointer | `Ptr<Int>` | Full nominal type | MMIO, typed access |
| Layout-constrained | `Ptr64` / `Ptr32` / `Ptr8` / `Ptr128` | Bytes + alignment only | Raw buffers, space |
| Generic | `Ptr` (bare) | Target-pointer-width bytes | Safe void* equivalent |
| Explicit bits | `Ptr<Bits @/0..63>` | Exact bit range | When you need precision |

### PtrN shorthand (Phase 1, 2026-07-03)

All pointer forms map to the same machine representation (`i64` / `u64` in
the backend). The pointee type only affects which operations are permitted:

| Sugar | Desugars to | Pointee bytes | Typed operations |
|-------|-------------|---------------|-----------------|
| `Ptr` | `Ptr<Bits @/0..63>` | 8 | Spatial only (no dereference) |
| `Ptr<T>` | `Ptr<Bits @/0..(T.bytes*8-1)>` | T.bytes | Full T semantics |
| `Ptr8` | `Ptr<Bits @/0..7>` | 1 | Spatial only |
| `Ptr16` | `Ptr<Bits @/0..15>` | 2 | Spatial only |
| `Ptr32` | `Ptr<Bits @/0..31>` | 4 | Spatial only |
| `Ptr64` | `Ptr<Bits @/0..63>` | 8 | Spatial only |
| `Ptr128` | `Ptr<Bits @/0..127>` | 16 | Spatial only |
| `Ptr256` | `Ptr<Bits @/0..255>` | 32 | Spatial only |

A bare `Ptr` without angle brackets is always 8 bytes of pointee (target-pointer
width on 64-bit targets), consistent with `Int` always being 64 bits.

## Operations

### Pointer arithmetic

All arithmetic operations preserve the pointer type:

| Expression | Result | Notes |
|---|---|---|
| `ptr + n` | Same ptr type | Wrapping byte offset |
| `ptr - n` | Same ptr type | Wrapping negative offset |
| `ptr ^ n` | Same ptr type | Bitwise XOR address with mask |
| `ptr & n` | Same ptr type | Bitwise AND address with mask |
| `ptr \| n` | Same ptr type | Bitwise OR address with mask |
| `ptr << n` | Same ptr type | Left shift address |
| `ptr >> n` | Same ptr type | Logical right shift address |
| `ptr_a == ptr_b` | `Bool` | Address equality |
| `ptr_a < ptr_b` | `Bool` | Address less-than |

### Pointer-to-pointer casts (Phase 2, 2026-07-03)

`Ptr<A> as Ptr<B>` is valid when the compiler can verify:
1. `bytes(A) == bytes(B)` — total width match
2. `alignment(A) >= alignment(B)` — source at least as aligned as dest

This means `Ptr<Float> as Ptr<Int32>` is valid (both 4 bytes, align 4),
but `Ptr<Int> as Ptr<Int32>` is NOT valid (different sizes).

```briev
let f: Ptr<Float> = 0x4000 as Ptr<Float>;
let i: Ptr<Int32> = f as Ptr<Int32>;      // ✅ Float.bytes == Int32.bytes
let raw: Ptr32 = f as Ptr32;               // ✅ Layout-compatible
```

No `reinterpret_cast` exists. If the compiler can't prove layout compatibility,
you must go through `Ptr<Bits @/N>` explicitly or use a `meld` declaration.

### Spatial intrinsics (Phase 3, 2026-07-03)

`Ptr<Bits @/N>` supports spatial operations that are valid for ANY type
matching that layout:

| Intrinsic | Signature | LLVM emission | Description |
|-----------|-----------|---------------|-------------|
| `__memcpy#` | `(Ptr, Ptr, Int) -> Bool` | `@llvm.memcpy.p0i8.p0i8.i64` | Non-overlapping copy |
| `__memcmp#` | `(Ptr, Ptr, Int) -> Int` | `@memcmp` (sext i32 to i64) | Byte comparison; 0 = equal |
| `__memset#` | `(Ptr, Int, Int) -> Bool` | `@llvm.memset.p0i8.i64` | Fill with byte value |
| `__hash#` | `(Ptr, Int) -> Int` | Inline FNV-1a loop | 64-bit hash |

These are available via `lib/std/spatial.bv`:

```briev
import { block_copy, block_compare, block_fill, block_hash } from "std/spatial.bv";

let dst: Ptr64 = malloc(8);
let src: Ptr64 = malloc(8);
block_copy(dst, src, 8);
let eq = block_compare(dst, src, 8);
```

### Opaque handles (Phase 4, 2026-07-03)

A library can return `Ptr<Bits @/N>` as an opaque handle. The caller cannot
inspect internals because `Ptr<Bits @/N>` only supports spatial operations:

```briev
// Library: returns opaque 24-byte handle
defn open_db(path: String) -> Ptr128 { ... };

// User: passes handle back, cannot inspect fields
defn query(db: Ptr128, sql: String) -> Result { ... };

// Library internals: re-lens to concrete type (compile-time checked)
inop __db_query#(db: Ptr128, sql: String) -> Result {
    let conn = db as Ptr<DbConnection>;  // internal cast
};
```

The `defining_module` field on `ResolvedType` tracks which module defined
each type. Full boundary enforcement (blocking cross-module `as` casts) is
available for compiler plugins to opt into.

### Function pointers (Phase 5, 2026-07-03)

Functions can be referenced via `.#Ptr` and called indirectly:

```briev
defn my_cmp(a: Int, b: Int) -> Bool { term a == b; };

let cmp_fn = my_cmp .#Ptr;        // function pointer via .#Ptr
let result = cmp_fn(3, 5);          // indirect call through fn pointer
```

The type of `cmp_fn` is `Applied("Fn", vec![Type::Tuple(params), ret])`.
The LLVM backend emits `inttoptr` → `call %fn_ptr()` with proper argument
marshalling (passes `%state`, handles Bool/Float/String types).

### Extract-Operate-Repack (EOR) optimization (Phase 6, 2026-07-03)

When `meld T <:> Int` exists, `(val as Int) * factor as T` is recognized as
an EOR pattern. The backend emits native arithmetic without redundant casts:

```briev
meld Meters <:> Int;
defn scale(val: Meters, factor: Int) -> Meters {
    term (val as Int) * factor as Meters;  // compiled as a single mul i64
};
```

The EOR detection (`try_emit_eor` in `helpers.rs`) checks:
1. Expression is `Cast(BinaryOp(Cast(a, T), Cast(b, T)), U)`
2. `U` has a meld with `T` (e.g., `Meters <:> Int`)
3. If both hold, emits `add`/`sub`/`mul`/`div` directly

## Safety via contracts

Safety is proven by the existing contract system, not by a borrow checker.
A pointer is just an integer with a *provenance*:

```briev
defn read_device(reg: Ptr<Int>) -> Int
    [reg as Int >= UART0_BASE]
    [reg as Int <  UART_END]
{
    term VolatileLoad#(reg);
};
```

The expression `reg as Int` converts the pointer to its raw address for use
in contract comparisons. The proof engine handles `>=` and `<` natively.

## Runtime representation and provenance

All pointer forms are `u64`/`i64` handles in both the interpreter and LLVM
backend — identical in size to `Int`. The pointee type exists at the type level
(`TypedRegister.ty` in the backend). LLVM sees a `Ptr<T>` value as `ptr` at
function/ABI boundaries and as the i64 handle internally.

Raw `Ptr<T>` index/deref accesses lower through `inttoptr`, so pointer
provenance must be preserved explicitly. The backend does this without
volatile and without per-access fences:

- `canonical_ptr(int_reg)` (`src/backend/llvm/mod.rs`) reuses one `ptr` SSA
  value for an address SSA value. Pointer arguments seed it from the entry
  `%argN`; `%State` Ptr fields seed it once per function at entry
  (`preload_state_ptr_fields`). A store to a Ptr field invalidates the field's
  entry for the rest of the function; later loads are re-emitted at their own
  site and are not re-cached.
- Raw `Expr::Index`/`Expr::Deref` count as raw memory in
  `definition_touches_raw_memory`, so such defns get
  `#13 memory(readwrite)`, never the argmemonly `#8` lie.
- Mutable Ptr fields do **not** get `!invariant.load`. A Ptr slot that can be
  reassigned (e.g. a linked-list head) must not claim invariance.
- Ptr-returning defns use the `ptr` ABI; the call site converts once to the
  i64 handle (`ptrtoint`) at the boundary. Ptr parameters convert once from
  `ptr` to i64 at entry.
- `vol` / `VolatileLoad#` / `VolatileStore#` remain the explicit volatile
  surface. Ordinary heap pointers are non-volatile and stay fully
  optimizable.

The canonical cache is dominance-safe: only values seeded at entry (arguments
and preloaded state fields) are reused. Arbitrary block-local i64 values still
receive a fresh conversion at the access site; a block-scoped cache is the
remaining coverage work (see the open-work backlog).

## Intrinsics

| Intrinsic | Signature | Description |
|---|---|---|
| `VolatileLoad#` | `(Ptr<T>) -> T` | Volatile read from an address through a typed pointer (pointee drives width; raw i64 addresses use `Load#`) |
| `VolatileStore#` | `(Ptr<T>, T) -> Bool` | Volatile write through a typed pointer; yields true (implemented 2026-08-27, plan Slice C) |
| `__memcpy#` | `(Ptr, Ptr, Int) -> Bool` | Copy N bytes (non-overlapping) |
| `__memcmp#` | `(Ptr, Ptr, Int) -> Int` | Compare N bytes; 0 = equal |
| `__memset#` | `(Ptr, Int, Int) -> Bool` | Fill N bytes with value |
| `__hash#` | `(Ptr, Int) -> Int` | FNV-1a hash of N bytes |

## Flat control flow mandate

All pointer-related code added in 2026-07-03 follows the max-2-levels nesting
rule. Helper functions use `?` and `else { return None; }` guard clauses.
See `docs/plans/2026-07-03-safe-void-star.md` for the full design.

## Implementation status

| Component | Phase | Status |
|-----------|-------|--------|
| `Type::LayoutPtr(Constraint)` | 1 | ✅ |
| PtrN parser sugar | 1 | ✅ |
| `Ptr<Bits @/N>` normalization | 1 | ✅ |
| Spatial intrinsics | 3 | ✅ |
| `lib/std/spatial.bv` | 3 | ✅ |
| Layout-compatible `Ptr<A> as Ptr<B>` | 2 | ✅ |
| Opaque handle pattern | 4 | ✅ |
| Function pointers (`.#Ptr`) | 5 | ✅ |
| EOR optimization | 6 | ✅ |
| Module boundary enforcement | 4 | ✅ (`defining_module` field) |
| Layout shape caching | 3 | 🚧 (deferred; spatial intrinsics are direct) |
