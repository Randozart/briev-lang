# Primordial Types — Pragmatic Companion to the Bits Thesis

**Date:** 2026-07-16
**Updated:** 2026-07-20 (hashword protocol architecture)
**Status:** Active
**Applies to:** TypeUniverse, normalizer, all backends

> **2026-07-20:** The `primitive`, `ctd`, and `alu` metadata properties
> described in this document are superseded. Primordial types now carry
> hashword op signatures instead of metadata tags:
>
> ```briev
> type Int: Int {
>     op Add(Int, Int);      // not primitive <~ "Int" + llvm <~ "i64"
>     op Parse(Int);           // identity literal construction
>     op Parse(Decimal);        // numeric literal construction
> };
> ```
>
> **2026-07-24:** The `<:` syntax is replaced by `: [Parent] [Protocol]`.
> `type Int: Int` declares Int as implementing the Int protocol. Width is
> inferred unless `bits <~ N` is explicit. The `.#` operator replaces `:>`
> for property access: `x.#Size` not `x .#Size`.
>
> **2026-07-31:** LLVM-type resolution is the **casting graph's** job, not
> the normalizer's. The normalizer only registers types in the universe
> (`register_typedefs`); the backend calls
> `casting_graph.resolve_llvm_type(universe, ty, int_bits)`. Any mention of
> `llvm_type` as a `ResolvedType` property in this doc is historical.
>
> The primordial seed table still exists as a convenience — it pre-populates
> the universe with well-known type names so `--disable-plugin prelude` works. But the
> semantic identity of a type is now determined by its op signatures, not
> by `primitive`/`ctd`/`alu` metadata.
>
> The `formatting <~` codec property described below is also superseded
> by `op Parse`. See the Parse Ops for Primordial Types section below.
>
> See `docs/architecture/casting-protocol.md` and
> `docs/plans/2026-07-20-protocol-and-meld-architecture.md#layer-10-parse`.

---

## Motivation

The Bits thesis (see [bits-thesis.md](bits-thesis.md)) defines `Bits` as the
sole compiler primitive. Every other type is a user-defined `type Foo : Bits`
with metadata. This is the ideal: a compiler that knows nothing about
integers, floats, or strings — only about uninterpreted bit vectors.

In practice, the strict interpretation creates a bootstrap problem. The
stdlib file `lib/std/types/bootstrap.bv` defines `Int`, `Float`, `Bool`,
`String`, etc. as `type X : Bits { ... }` declarations with metadata. These
arrive in the compilation pipeline via the prelude plugin + import resolver.
If either step fails or is absent, a "bare" type reference like
`meld Int -> Float` panics: the `TypeUniverse` has no `Int` entry.

**Primordial types** seed the `TypeUniverse` with well-known type names on
construction, each carrying default metadata. They make `Int`, `Float`, etc.
available without stdlib import, while preserving the Bits thesis's core
insight: **the compiler never hardcodes type semantics**. It merely seeds
the metadata that backends already read.

---

## What Primordial Types Are Not

Primordial types are **not** a return to `Token::TypeInt` or hardcoded type
dispatch. The following remain true:

| Aspect | Before (pre-Bits-thesis) | After primordial types |
|--------|------------------------|----------------------|
| Lexer | 33 `Token::Type*` variants | All identifiers |
| Parser `parse_type()` | Token variant match | Identifier string dispatch |
| Backend type dispatch | Name string matching (`"Int" → i64`) | Metadata-driven (`primitive`, `llvm_type`) |
| User override | Not possible without stdlib | `type Int : Bits { ... }` replaces primordial |
| Semantics | Hardcoded in compiler | Defined by metadata properties |

The primordial table is a **convenience seed**, not a privileged type system.

---

## Relationship to the Bits Thesis

The Bits thesis (Appendix A of `docs/architecture/bits-thesis.md`) defines
three axioms:

1. **`Bits` is the sole primitive** — upheld. All primordial types have
   `base = "Bits"`.
2. **All semantic meaning is metadata** — upheld. Primordial types are
   metadata bundles (`bytes`, `alignment`, `primitive`, `llvm_type`).
3. **Any type can be redefined** — upheld. A user `type Int : Bits { ... }`
   replaces the primordial entry via `HashMap::insert`.

Primordial types are an appendix to the Bits thesis, not a contradiction.
They acknowledge that the compiler needs a self-hosting bootstrap to be
useful without stdlib import, while keeping the thesis's architectural
commitments intact.

---

## The Primordial Table

| Name | bytes | alignment | primitive | llvm_type | Notes |
|------|-------|-----------|-----------|-----------|-------|
| `Int` | 8 | 8 | signed | i64 | Default integer |
| `UInt` | 8 | 8 | unsigned | i64 | Default unsigned |
| `Int8` | 1 | 1 | signed | i8 | |
| `UInt8` | 1 | 1 | unsigned | i8 | |
| `Int16` | 2 | 2 | signed | i16 | |
| `UInt16` | 2 | 2 | unsigned | i16 | |
| `Int32` | 4 | 4 | signed | i32 | |
| `UInt32` | 4 | 4 | unsigned | i32 | |
| `Int64` | 8 | 8 | signed | i64 | |
| `UInt64` | 8 | 8 | unsigned | i64 | |
| `Float` | 4 | 4 | float | float | Alias `Float32` |
| `Float64` | 8 | 8 | float | double | Alias `Double` |
| `Bool` | 1 | 1 | unsigned | i8 | |
| `Char` | 4 | 4 | unsigned | i32 | |
| `String` | 16 | 8 | struct | %String | 2 fields: data, len |
| `Data` | 8 | 8 | pointer | i8* | Typed pointer |
| `Void` | 0 | 0 | void | void | Zero-width |
| `UTF8View` | 16 | 8 | struct | `{ i64, i64 }` | Borrowed UTF-8 view (fat ptr: data, len) |
| `StaticString` | 16 | 8 | struct | `{ i64, i64 }` | ROM string (ptr + len) |
| `SmallString64` | 72 | 8 | struct | `{ i64 x 9 }` | 64-byte inline buffer, zero heap |

**String** is a 2-field struct (`data: Int, len: Int`) with `encoding <~ "UTF-8"`
property. With SSO (feature flag `feature_sso_strings`), the handle is
`{ i64, i64 }` where handle[0] packs ≤6 bytes inline (tag bit 0 = 1) or
holds a heap pointer (tag bit 0 = 0). Handle[1] is the byte length.
Codegen identifies string-like types via `is_string_like()` which checks
shape (2 Int fields) + encoding property, not the name `"String"`.

**UTF8View** is a borrowed, zero-allocation UTF-8 view. Same `{i64, i64}`
fat pointer format as String but never owns its buffer. Excluded from
`type_is_heap_allocated`. Always `encoding <~ "UTF-8"` (guaranteed by
construction). Cannot be stored in state (borrow — would dangle across ticks).

**StaticString** is a ROM-resident string literal. Points to `.rodata`.
Created automatically by the compiler for string literals. No allocation,
no free.

**SmallString64** is an embedded-friendly inline buffer string. 64 bytes
of storage packed into 8 × Int fields + 1 × Int length. Zero heap allocation.
Ideal for microcontroller/bare-metal targets. Operations read/write bytes
directly from the struct fields via `when`-chained slot selection.

The `fields` vector on `ResolvedType` is populated from `TypeDef.body.slots`
by the normalizer's `register_typedefs`. For primordial types, fields
are seeded in `seed_primordial_types()`. Backends use `fields` for LLVM
struct type lowering, state slot width, and `is_string_like` detection.

---

## Override Semantics

When a source file defines `type Int : Bits { data: Bits<32>; ... }`, the
normalizer's `register_typedefs` function calls `universe.register(rt)`,
which does `self.types.insert("Int", rt)`. This **replaces** the primordial
`Int` entry. The replacement is complete — all primordial ops and layout
are lost, and the user's definition is authoritative.

This means:

- `type Int { data: Bits<32>; op Add(Float, Float) = int_add_float(#L,#R); }`
  → Int is now a 32-bit type that adds like a float. The name "Int" is
  irrelevant to codegen — only the layout and ops matter.
- `type String { data: Bits<32>; }` → String is now a 32-bit scalar.
  Operations that expected `String` protocol ops (`Extract(Char)`,
  `InsertAt(Char)`, `Concat(String)`) will fail at the typechecker
  because the user's definition doesn't declare them.

The "deals with it" contract: once you declare a type with a given name, you
own its semantics. The compiler will not warn you that `String` is now a
float-like type — it will emit whatever the layout and ops dictate.

---

## Parse Ops for Primordial Types

Each primordial type implicitly carries `op Parse(#Category)` for its
protocol category, providing zero-cost identity literal construction:

| Primordial | Protocol | Implicit Parse op | Also accepts |
|---|---|---|---|
| `Int` | `Int` | `op Parse(Int)` | `op Parse(Decimal)` — numeric literals |
| `Float` | `Float` | `op Parse(Float)` | `op Parse(Decimal)` — numeric literals |
| `Bool` | `Bool` | `op Parse(Bool)` | `op Parse(Bare)` — `true`/`false` |
| `Char` | `Char` | `op Parse(Char)` | `op Parse(Decimal)` — code point value |
| `String` | `String` | `op Parse(String)` | `op Parse(Quoted)` — string literals |
| `UInt` | `Int` | `op Parse(Int)` | `op Parse(Decimal)` |
| *(all others)* | derived | derived from protocol | derived from structure |

When a user overrides a primordial type via `type Int { ... }`, the Parse ops
are also overridden. If the user's definition includes `op Parse(Int)` or
`op Parse(Decimal)`, those replace the primordial defaults. If neither is
declared, the type has NO implicit Parse ops — it cannot be constructed
from literals without an explicit conversion function.

The implicit `op Parse(#Category)` for primordial types is an optimization
of the primordial seed logic in `TypeUniverse::new()`, not a special case in
the type system. A user-defined type with the same ops and protocol gets
identical behavior.

---

## Implementation

The seed table lives in `TypeUniverse::new()` via a helper
`fn seed_primordial_types(&mut self)`. Each entry is constructed as a
`ResolvedType` with properties matching the table above.

The `seed_primordial_types()` helper must also seed the implicit
`op Parse(#Category)` and `op Parse(Form)` entries for each primordial type.
These are added to the `ResolvedType.operators` vec just like any other op.

The normalizer's annotation loop checks `rt.properties.contains_key("llvm_type")`
before deriving it, so primordial types with explicit `llvm_type` keep their
annotation.

---

## See Also

- [bits-thesis.md](bits-thesis.md) — the three axioms this document companions
- [backend-type-dispatch.md](backend-type-dispatch.md) — how backends read metadata
- `src/type_universe/mod.rs` — TypeUniverse + primordial seed
- `src/backend/llvm/normalizer.rs` — normalizer annotation loop
- `lib/std/types/bootstrap.bv` — stdlib type definitions (still loads; its
  TypeDef inserts are no-ops since primordial already exists)
