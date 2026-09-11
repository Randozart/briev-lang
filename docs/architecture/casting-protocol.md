# Casting Protocol — Type Conversion via Hashword Categories

**Date:** 2026-07-20
**Status:** Foundational
**Applies to:** TypeUniverse, normalizer, all backends, typechecker

---

## Overview

Briev has no built-in `as` cast operator. Type conversion is triggered by
the `Cast#()` compiler intrinsic (emitted when the programmer writes
`(TargetType)expr`). The intrinsick runs a resolution pipeline that checks,
in order:

1. `meld Source <-> Target` — structural equivalence
2. `op Cast(Target)` on Source — direct type-to-type
3. `CastTo(#Category)` → `CastFrom(#Category)` — protocol path
4. Implicit `CastTo(Bits)` + `CastFrom(Bits)` — raw bytes (always)

### User-declarable ops

| Op | Direction | Purpose |
|---|---|---|
| `op CastTo(String)` | Source **→** protocol | Produce UTF-8 bytes for `String` |
| `op CastFrom(String)` | Protocol **→** Source | Consume UTF-8 bytes from `String` |
| `op Cast(ConcreteType)` | Source **→** Target | Direct conversion between two concrete types |

`CastTo` and `CastFrom` are always oriented toward the `#Category` protocol.
`Cast(ConcreteType)` is for direct paths between concrete types — no `To`/`From`
needed because both sides are concrete and the direction is unambiguous.

### Example

```briev
type Latin1String {
    op CastTo(String) = latin1_to_UTF8(#L);      // Latin1 → UTF-8
    op CastFrom(String) = UTF8_to_latin1(#L);     // UTF-8 → Latin1
};

type Posit32 {
    op Cast(Int) = Posit32_to_int(#L);             // Posit32 → Int (direct)
};
```

---

## Protocol Parameterization

Each hashword category has one or more **protocol variants** — concrete
representations of the same semantic concept:

| Hashword | Protocol variants | Default (`.bv`) | Default (`.ebv`) |
|---|---|---|---|
| `String` | `UTF8`, `ASCII`, `hex`, `base64` | `UTF8` | `ASCII` |
| `Float` | `IEEE754`, `bin32`, `bin64` | `IEEE754` (backends choose width) | *(same)* |
| `Int` | (no variants — width is target-dependent) | (intrinsic) | (intrinsic) |
| `Bool` | (no variants) | (intrinsic) | (intrinsic) |
| `Char` | `unicode`, `ASCII` | `unicode` | `ASCII` |
| `Bits` | (no variants — raw bits) | (intrinsic) | (intrinsic) |

**The file extension determines the default protocol:**

```briev
// foo.bv — String<UTF8> by default
op Add(String, String);   // resolves to String<UTF8>

// bar.ebv — String<ASCII> by default
op Add(String, String);   // resolves to String<ASCII>
```

**Cross-variant calls require explicit protocol:**

```briev
fn cross(a: String<UTF8>, b: String<ASCII>) { ... };
                               ^^^^^ explicit
```

Without the angle bracket, the compiler uses the source file's default.
If a function from a different file extension (e.g., `.ebv`) is called
from `.bv` where the protocol differs, the compiler errors:

```
error: protocol declaration for String not explicitly defined.
  Called from .bv (default: UTF8) into .ebv (default: ASCII).
  Use String<UTF8> or String<ASCII> to disambiguate.
```

### Why explicit protocols matter

A bit-shifting function written against `String` bytes produces different
results depending on the encoding. UTF-8 has multi-byte sequences where
the high bit is set. ASCII rejects bytes ≥ 0x80. If the protocol variant
changed silently between file extensions, the same function would produce
different results on different targets.

Explicit protocols at crossing boundaries prevent this. The programmer
must acknowledge the encoding difference and declare the transformation
via a `proto` declaration:

```briev
proto ASCII: String {
    CastTo(String<UTF8>) = ASCII_to_UTF8(#L);
    CastFrom(String<UTF8>) = UTF8_to_ASCII(#L);
};
```

The binding defines HOW the layouts differ. The compiler proves round-trip
identity to ensure the transformation is consistent.

---

## The Protocol Graph

Every `Cast(#Category)` declaration is a directed edge. The compiler finds
the shortest path from source to target at compile time via BFS.

### Edges from `proto` Declarations

Protocol variants are declared via `proto` with required bindings:

```briev
proto ASCII: String {
    CastTo(String<UTF8>) = ASCII_to_UTF8(#L);      // edge: ASCII → UTF8
    CastFrom(String<UTF8>) = UTF8_to_ASCII(#L);     // edge: UTF8 → ASCII
};
```

The binding IS the transformation function. Without a binding, the edge
cannot be compiled — the compiler doesn't know HOW to convert between
the two layouts.

### Edges from Type Declarations

Types can also declare edges via `op CastTo`/`op CastFrom`:

```briev
type Latin1String {
    op CastTo(String) = latin1_to_UTF8(#L);      // edge: Latin1String → String
    op CastFrom(String) = UTF8_to_latin1(#L);     // edge: String → Latin1String
};
```

Both feed the same BFS. Protocol-level edges are inherited by participating
types; type-level edges override them.

---

## Protocol Shapes (Backend Contract)

Every hashword category variant has a concrete representation that ALL
backends must be able to produce and consume:

| Hashword | Variant | Representation | Required ops |
|---|---|---|---|
| `Int` | *(none)* | `i{target_width}` | Add, Sub, Mul, Div, And, Or, Xor, Not, Shl, Shr |
| `Float` | `IEEE754` | binary32 or binary64 | Add, Sub, Mul, Div, Sqrt, FMA, `Cast(Float64)` |
| `Float` | `bin32` | binary32 | Same as IEEE754, fixed at 32-bit |
| `Float` | `bin64` | binary64 | Same as IEEE754, fixed at 64-bit |
| `Bool` | *(none)* | `i1` (stored i8) | And, Or, Not |
| `Char` | `unicode` | `i32` code point 0–0x10FFFF | `Cast(Int)`, Eq, Lt |
| `Char` | `ASCII` | `i8` code point 0–127 | `Cast(Int)`, Eq, Lt |
| `String` | `UTF8` | UTF-8 byte sequence | Extract(`Char`), InsertAt(`Char`), Concat(`String`), `.#Size`, `Cast(Bits)` |
| `String` | `ASCII` | ASCII byte sequence (0–127 per byte) | Same as UTF8 — protocol ops are encoding-agnostic |
| `String` | `hex` | Hex-encoded bytes (`0-9a-f` pairs) | Same as UTF8 |
| `String` | `base64` | Base64-encoded bytes | Same as UTF8 |
| `Bits` | *(none)* | Raw `iN` | And, Or, Xor, Not, Shl, Shr |

The protocol ops (`Extract(Char)`, `InsertAt(Char)`, etc.) are the same
regardless of variant. The backend's protocol handler translates between
the variant's internal representation and the protocol shape (UTF-8 bytes).

---

## Conversion Path Resolution

When the compiler encounters a cross-type operation (e.g., `ASCII_str + string`),
it resolves the conversion path:

1. **Exact match**: Type defines `op Add(String)` → use it.
2. **Direct cast**: Type A defines `op Cast(#Category<variant>)` → use it once.
3. **Protocol chain**: Find shortest path through the protocol graph.
4. **Implicit fallback**: `Bits → Bits` (raw bytes).

### BFS Search

```
Input:  source_type, target_category<variant>
Output: sequence of Cast ops, or error

1. If source_type has op Cast(target_category<variant>):
       return [source_type → target_category<variant>]
2. BFS over the protocol graph from source_type to target_category<variant>:
   - Each node is a category variant (e.g. String<UTF8>)
   - Each edge is a Cast(Category<variant>) declaration
   - Bits is always reachable from every type
3. If path found: return sequence of Cast ops
4. If no path: compiler error with available alternatives
```

### Example: `String<ASCII> → String<UTF8>`

If no direct `op Cast(String<UTF8>)` exists, the path is:
`Source.CastTo(Bits)` → `Target.CastFrom(Bits)`

1. `String<ASCII> :> CastTo(Bits)` → raw bytes
2. `Bits :> CastFrom(String<UTF8>)` → construct UTF-8 from raw bytes

The backend implements step 2 in its protocol handler. An optimizing backend
that uses ASCII internally for both would skip both casts.

### Example: `Latin1String → ASCIIString` via protocol

`Source.CastTo(String)` → `Target.CastFrom(String)`

1. `Latin1String :> CastTo(String)` — decodes Latin-1 bytes to `Char` (Unicode scalar)
2. `ASCIIString :> CastFrom(String)` — encodes `Char` to ASCII bytes

For the ASCII range (0–127): the `zext i8 to i32` (Latin-1 → Char) and
`trunc i32 to i8` (Char → ASCII) are both inlined. LLVM's `InstCombine`
eliminates them to a raw byte copy. The protocol overhead is zero for the
common case.

---

## Backend Protocol Handlers

Each backend declares which hashword categories and protocol variants it
supports in `config/targets.toml`:

```toml
[target.desktop]
backend = "llvm"
protocols = [
    "String<UTF8>",
    "String<ASCII>",
    "Float<IEEE754>",
    "Int",
    "Bool",
    "Char<unicode>",
    "Char<ASCII>",
    "Bits",
]

[target.embedded-riscv]
backend = "llvm"
protocols = [
    "String<ASCII>",
    "Int",
    "Bool",
    "Char<ASCII>",
    "Bits",
]
```

The backend implements a protocol handler — a `match` on the variant:

```rust
impl LlvmBackend {
    fn emit_string_concat(&mut self, a: &TypedRegister, b: &TypedRegister, protocol: &str) {
        // 2026-08-01 (B4): String values are ptrs to [len][bytes] — the concat
        // returns a ptr, not a {i64,i64} fat pointer.
        match protocol {
            "UTF8" => {
                writeln!(out, "%r = call ptr @__UTF8_concat(ptr {}, ptr {})",
                    a, b);
            }
            "ASCII" => {
                writeln!(out, "%r = call ptr @__ASCII_concat(ptr {}, ptr {})",
                    a, b);
            }
            "hex" | "base64" => {
                // No hardware support — stdlib fallback
                writeln!(out, "%r = call ptr @__string_transform(ptr {}, ptr {}, \"{}\")",
                    a, b, protocol);
            }
            _ => compile_error!("protocol '{}' not supported by LLVM backend", protocol),
        }
    }
}
```

A function requiring a protocol the backend does not implement:

```
error: target 'embedded-riscv' does not support protocol 'String<UTF8>'.
  Required by function 'generic_concat' in foo.bv.
  Available protocols on this target: String<ASCII>, Int, Bool, ...
```

### Cross-variant detection

The typechecker treats `String<UTF8>` and `String<ASCII>` as distinct types.
Passing one where the other is expected produces:

```
type mismatch: expected String<ASCII> for parameter 1, found String<UTF8>
```

The file extension determines the default variant at parse time:
- `.bv` files: bare `String` → `String<UTF8>`
- `.ebv` files: bare `String` → `String<ASCII>`

When a `.bv` file calls an `.ebv` function using `String`, the default
variants differ (`UTF8` vs `ASCII`), and the typechecker's existing
mismatch detection catches it automatically. The programmer adds the
explicit variant at the call site:

```briev
fn cross(a: String<UTF8>, b: String<ASCII>) { ... };
```

### Adding new protocols

Adding a new protocol variant is additive:

1. Declare the variant via `proto` with CastTo/CastFrom bindings
2. Optionally add a match arm in the backend's protocol handler for optimizations
3. Optionally add it to `config/targets.toml` for GLUE export

The `proto` declaration is always the minimal requirement — it defines the
transformation functions that let the compiler discover paths through the
protocol graph without backend changes.

---

## `disamb` — Disambiguation Hint (superseded)

> **2026-07-31:** `disamb` is superseded by the hardcoded well-known protocol
> variants in the casting graph (`Float<BFloat>` → `bfloat`,
> `Float<Half>` → `half`, …). See `docs/architecture/agent-reference.md` §1.0
> "Protocol variants". The section below is retained as historical reference
> for the pre-variant mechanism.

The `disamb <~ "value"` metadata property disambiguates representations that
structure + bytes + protocol ops cannot distinguish. Currently only needed
for 2-byte floats (`half` vs `bfloat`):

```briev
type Bfloat16 {
    data: Bits<16>;
    disamb <~ "bfloat";
    op Add(Float, Float);
};
```

The normalizer reads `disamb` when deriving `llvm_type` for `Float`-category
types at 2 bytes:

| `disamb` absent | `disamb <~ "bfloat"` |
|---|---|
| `llvm_type = "half"` (IEEE 754) | `llvm_type = "bfloat"` |

`disamb` is a **hint, not a directive** — the normalizer ignores it when
structure alone is sufficient (e.g., 4-byte `Float` is always `"float"`,
8-byte is always `"double"`). It only matters when the combinatorics of
bytes + category ops produce multiple valid representations.

---

## Protocol Ops (Required Per Category)

### `String` protocol

```
op CastTo(String) = fn(#L);         // emit UTF-8 bytes
op CastFrom(String) = fn(#L);       // consume UTF-8 bytes
op Extract(Char) = fn(#L, #R);     // extract char at index
op InsertAt(Char) = fn(#L, #R);    // insert char at index
op Concat(String) = fn(#L, #R);    // append another string-type
.#Size                              // get length in characters
CastTo(Bits)                        // raw bytes
CastFrom(Bits)                      // from raw bytes
```

These ops let ANY two `String` types communicate through the `CastTo`/`CastFrom`
pair — they negotiate the UTF-8 protocol shape without an intermediate type.

```briev
inline defn any_string_to_ASCII(source: String) -> String<ASCII> {
    let bytes = source :> CastTo(Bits);
    // bytes are UTF-8 — validate, then construct ASCIIString
    // ...
};
```

### `Float` width resolution (FloatWidth)

The `Float` protocol owns the width semantics: a Float-category type's LLVM
type is derived from its `bits` metadata, not from any type name.

| `bits` | LLVM type |
|--------|-----------|
| 16 | `half` / `bfloat` (via the `disamb` metadata value) |
| 32 | `float` |
| 64 | `double` |
| 80 | `x86_fp80` |
| 128 | `fp128` |
| default | `float` |

Casting between any two Float representations (variants or the base) is a
`FloatWidth` lane — emitted as `fpext`/`fptrunc` (identity when the widths
match). This is how a boundary type (`CDouble: Float<C_Double>`) gets
`double` and how `2.0 as Float64` widens cleanly.

### `Float` protocol

```
Add(Float)
Mul(Float)
Sub(Float)
Div(Float)
Sqrt(Float)
CastTo(Float)     // produce IEEE 754 bytes
CastFrom(Float)   // consume IEEE 754 bytes
CastTo(Bits)      // raw bits
CastFrom(Bits)    // from raw bits
```

The `CastTo`/`CastFrom` pair handles float conversion directly — no intermediate
type. A Posit32 backend implements `CastTo(Float)` to produce IEEE 754 bytes
and `CastFrom(Float)` to consume them.

### `Int` protocol

```
Add(Int)
Sub(Int)
Mul(Int)
Div(Int)
And(Bits)
Or(Bits)
Xor(Bits)
Not(Bits)
Shl(Bits)
Shr(Bits)
```

Int ops are backend-intrinsic — every backend knows how to add integers.
The protocol shape for `Int` is `i64`. Conversion goes through `CastTo(Bits)` / `CastFrom(Bits)`.

---

## Type Parameter Constraints

```briev
type HashMap<K: String, V> {
    buckets: Bits<64>;
    len: Bits<64>;
    capacity: Bits<64>;
    op Insert(V) = hashmap_insert(#L, #R1, #R2);
    op Get(K) -> V = hashmap_get(#L, #R);
};
```

`K: String` is a **protocol satisfaction check**. The compiler verifies:
does `K` implement the `String` protocol ops (`Extract(Char)`,
`InsertAt(Char)`, `Concat(String)`, `.#Size`, `Cast(Bits)`)?
If yes, `K` satisfies `String` regardless of its concrete name or layout.

Protocol variant constraints are also valid:
```briev
type AscHashMap<K: String<ASCII>, V> { ... };
```

---

---

## Parse Protocol — Compile-Time Literal Construction

### Distinction from Cast

| Operation | Purpose | When it fires |
|---|---|---|
| `Cast#(source, TargetType)` | Convert existing value to different type | Assignment, function call, explicit cast |
| `op Parse(Form)` | Construct value from source text | Literal in source code: `42`, `"..."`, `FF00FF` |

Parse is NOT a subtype of Cast. Parse happens during early typechecking;
Cast happens during codegen. However, Parse ops may invoke Cast internally
(e.g., `op Parse(Decimal) = int_parse_and_cast(#L)`).

### Parse forms

| Form | Implementation? | Meaning |
|---|---|---|
| `op Parse(#Category)` | No — identity | "This type IS the protocol shape for parsing" |
| `op Parse(Bare) = fn(#L)` | Yes — required | Construct from bareword identifier |
| `op Parse(Decimal) = fn(#L)` | Yes — required | Construct from numeric literal `42` |
| `op Parse(Decimal, pre: "0x") = fn(#L)` | Yes — required | Construct from hex literal `0xFF00FF` |
| `op Parse(Decimal, suf: "h") = fn(#L)` | Yes — required | Construct from hex suffix literal `FF00FFh` |
| `op Parse(Quoted) = fn(#L)` | Yes — required | Construct from quoted string `"..."` |

### Parse resolution pipeline

When the compiler encounters a literal `42` assigned to type `T`:

1. Determine the literal's syntactic form (Bare/Decimal/Quoted) and any
   prefix/suffix discriminator (0x, h, bf, etc.)
2. Does `T` declare `op Parse(#Category)` where the category matches the
   literal's protocol? → Use identity (zero-cost, no emission)
3. Does `T` declare `op Parse(Form, pre: "0x")` or `op Parse(Form, suf: "h")`
   matching the literal's discriminator? → Most specific match wins
4. Does `T` declare `op Parse(Form)` without discriminator?
   → Call the inlined defn at compile time (fallback)
5. Does `T`'s parent (via `<:`) have a Parse op? → Check inheritance
6. No match → compile error: "type T does not accept Decimal literals"

### Discriminator qualifiers: pre: and suf:

Parse ops can declare optional prefix and suffix qualifiers:

| Declaration | Matches | Priority |
|---|---|---|
| `op Parse(Decimal, pre: "0x")` | `0xFF00FF` | Highest — exact discriminator match |
| `op Parse(Decimal, suf: "h")` | `FF00FFh` | Highest — exact discriminator match |
| `op Parse(Decimal)` | `42` | Lower — fallback for unprefixed literals |

Resolution: discriminator match always wins over unqualified match.
The most specific qualifying Parse op is chosen before the fallback.

### Discriminator validation

The parser validates that `pre:`/`suf:` values contain no symbols that
conflict with language operators. Forbidden symbols:
`# ! @ & $ ( ) [ ] < > * , ; : = ~ % { } " ' | \`.

```briev
op Parse(Decimal, pre: "@hex") = parse_hex(#L);  // ERROR: '@' reserved
```

### TaggedLiteral AST variant

When the lexer encounters a prefixed literal (`0xFF00FF`) or suffixed
literal (`FF00FFh`), the parser produces `Expr::TaggedLiteral(i64, String)`
with the discriminator tag. The typechecker matches discriminator tags
against `pre:`/`suf:` qualifiers on Parse ops.

### Replacement of `formatting <~`

The `formatting <~` metadata property and the `codec { ... }` declaration form
are superseded by `op Parse`:

| Old mechanism | Replaced by |
|---|---|
| `formatting <~ Bare` + `parse <~ parse_hex` | `op Parse(Bare) = parse_hex(#L)` |
| `formatting <~ Decimal` + `parse <~ parse_fn` | `op Parse(Decimal) = fn(#L)` |
| `formatting <~ Quoted` + `parse <~ identity` | `op Parse(String)` or `op Parse(Quoted) = fn(#L)` |
| `DefaultQuoted` codec class | Inline `op Parse` on each type |

---

## Round-Trip Verification

For every protocol that declares matching `CastTo`/`CastFrom` pairs, the
compiler proves round-trip identity via symbolic execution and SMT:

```briev
proto ASCII: String {
    CastTo(String<UTF8>) = ASCII_to_UTF8(#L);
    CastFrom(String<UTF8>) = UTF8_to_ASCII(#L);
};

// Proved: UTF8_to_ASCII(ASCII_to_UTF8(x)) == x
```

For every cross-variant `op` declaration, the compiler proves equivalence
to the default round-trip path:

```briev
protocol ASCII: String {
    CastTo(String<UTF8>) = ASCII_to_UTF8(#L);
    CastFrom(String<UTF8>) = UTF8_to_ASCII(#L);
    op Add(String<UTF8>) = ASCII_add_with_UTF8(#L, #R);
};

// Proved: ASCII_add_with_UTF8(x, y) == UTF8_to_ASCII(ASCII_to_UTF8(x) + y)
```

If either proof fails, compilation is denied. The existing pipeline in
`src/analysis/meld_validation.rs` handles both cases — Layer 4 (symbolic)
for value-level proofs and Layer 5 (SMT) for full formal verification.

---

## Implementation Phases

### Phase 1: Protocol Variant Syntax

- Parse `#Category<variant>` in op signatures
- Cross-variant call detection and error reporting

### Phase 2: Protocol Graph Skeleton

- `Cast(#Category<variant>)` as a valid op signature
- Implicit `Cast(Bits)` on every type
- BFS path resolution in the typechecker

### Phase 3: Protocol Shape Validation

- Per-category variant validation in typechecker
- `K: String` constraint satisfaction checking
- Missing protocol ops → compile error with available alternatives

### Phase 4: Backend Protocol Handlers

- `config/targets.toml` → protocol list per target
- LLVM backend protocol handler match arms
- Unsupported protocol → compile error listing available protocols

### Phase 5: Parse Protocol

- `op Parse(#Category)` as identity parse (no conversion function)
- `op Parse(Bare/Decimal/Quoted)` with conversion function
- Parse resolution in literal construction (typechecker)
- Round-trip verification in symbolic execution engine
- Replace `formatting <~` codec property with Parse ops

## Melds and the Composite ABI (GLUE boundaries)

2026-08-03 (plan `2026-08-03-native-python-meld-composite.md`). A meld
declaration (`meld CStr -> String`) is a *composite*: one memory region under
two native views, not a conversion recipe. The composite ABI contract every
shim honors:

1. **Handle ABI.** A String/Data composite crosses the boundary as an i64
   handle. Every shim dereferences it to a state-owned, stable region
   `(ptr, len)` with a **NUL invariant** (`bytes[len] == '\0'`).
2. **Lifetime = the state's life.** The arena owns the memory; the state is the
   arena's handle. A composite view is valid from creation until
   `__glue_release`; hosts borrow read-only; each shim *pins* the state so the
   borrow is safe (Python's `memoryview` holds the state ref, Rust borrows the
   state, C is a documented don't-release contract).
3. **Mutability is declared by the meld**, never per language. Default
   read-only (a Briev String is immutable — every language must see it
   read-only); a meld may declare a mutable region.
4. **Interchangeability is declared by the meld.** `meld CStr -> String`
   admits the pair without `as` at assignment, let-init, call args, constructor
   slots, and term/return. The boundary marshalling inserts the delta
   (`cstr_to_briev`/`str_to_c`) at those sites; the typechecker only admits
   the pair — the conversion stays a casting-graph / marshalling decision.
5. **The composite is asymmetric.** A Briev String's data region IS a
   nul-terminated C string, so String → CStr is zero-copy (`str_to_c` returns
   the in-place `(handle + 8)` pointer). CStr → String wraps (a bare C string
   has no length prefix — `cstr_to_briev` allocates the `[len][bytes][\0]`
   form).
6. **No language knowledge in the compiler.** Every language's shim is a
   config-driven recipe (`config/glue.dbvl` templates + per-category
   parse/build snippets). The compiler carries only the composite ABI contract
   + the meld declarations. Adding a language (Lisp, COBOL, …) is a config
   section, never a compiler change.
