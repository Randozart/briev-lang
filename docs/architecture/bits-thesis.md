# The Bits Thesis

**Date:** 2026-07-11  
**Updated:** 2026-08-15 (Fundamentals: `Data` root, `Bit<N>` unified, `Blob`)  
**Status:** Foundational  
**Applies to:** Briev compiler core architecture, interpreter, type system, backends

> **2026-09-02 (shape law):** this thesis predates the de-hashtagged
> fundamentals. In every example below, read a type-declaration parent
> `Int`/`String`/`Float` as the plain name (`Int`/`String`/`Float`)
> and `!> bits: N` as `spec Bits: N` (§8.2). The `#` survives only as
> backend directives in op signatures and cast edges
> (`op Add(Float): …`, `axiom CastTo(Float<IEEE754>)`), plus the
> casting graph's internal category keys. The shape law:
> `type Int32: Int { spec Bits: 32; };` — AGENTS.md pillar.

---

## 2026-07-20 Update: Hashword Protocol Architecture

The metadata properties `primitive`, `ctd`, and `alu` are superseded by
the **hashword protocol system**. Types no longer carry `primitive <~ "Int"`
to tell the backend what they are. Instead, they declare ops using `#Category`
hashwords — `op Add(Int, Int)` — which are backend directives.

Hashwords can be parameterized by **protocol variant** using angle brackets:
`String<UTF8>`, `String<ASCII>`, `String<hex>`, `String<base64>`,
`Float<IEEE754>`. The file extension determines the default (`.bv` → UTF8,
`.ebv` → ASCII). Cross-variant calls require explicit protocol.

| Old mechanism | Replaced by |
|---|---|---|
| `primitive <~ "Int"` + `llvm <~ "i64"` | Structure + `op Add(Int)` |
| `ctd <~ "Float"` + `alu <~ "Float"` | Structure + `op Add(Float)` |
| `op Add ~> "int.add"` (string binding) | `op Add(Int, Int)` (hashword directive) |
| TOML config (`llvm-ops.toml`, `ctd-llvm-mappings.toml`) | Removed — hashword backend intrinsics |
| `category` inference (2026-07-19 attempt) | Removed — types don't belong to categories |
| `<~` property syntax | `!> key: value;` — e.g., `!> bits: 8;` |

**The Bits thesis is unaffected.** `Bit` is the protocol; `Bit` is the sole
primitive type (hardcoded anchor in the compiler, not a primordial — primordials
are overrideable by stdlib, Bit is not). They are tightly coupled:
the protocol guarantees the semantics, the type provides the concrete
representation. What
changes is how types express their semantics: through ops with hashword
signatures, not through metadata tags. See `docs/architecture/casting-protocol.md`
and `docs/plans/2026-07-20-extensible-number-types-final.md` for the full
architecture.

> **2026-08-15 (Fundamentals addendum).** The hierarchy above is updated:
> `Data` is now the universal reflective floor (every value observable as raw
> storage via the treat-as-bits view — **NOT a supertype**; no universal
> inheritance edge) and `Bit<N>` is the unified bit type at any declared
> width (`Bit` bare = flexible, resolved later; `Bit<N>` = exact N). There is
> no separate `Bits` type — multiple bits is just `Bit<N>`. The byte-buffer
> type (formerly `Data`) is renamed
> `Blob` (a `[len][bytes]` buffer, a `Data` member like `String` but with no
> encoding interpretation). Category hashwords lose their `#` in fundamental
> positions (`Int`→`Int`, `Bit`→`Bit`, `Data`→`Data`); protocol variants
> (`String<UTF8>`) keep theirs. The bit-thesis core survives: every type is
> composed of bits, and `Bit<N>` is the most direct representation. See
> `docs/plans/2026-08-15-fundamentals-as-types.md`.

## 2026-07-24 Update: Protocol-First Types

Type declarations now use `: Protocol` instead of `: Bits`:

```briev
// Before:
type Int : Bits { maxbits <~ 64; ... op Add(Int, Int); };

// After:
type Int: Int;                       // protocol-only (width inferred)
type i64: Int { !> bits: 64; };       // derives from Int, explicit width
type UInt: Int;                        // derives from Int, inherits Int protocol
```

Key changes:
- **`Bits` is implicit** — no need to declare `: Bits`. Every type has bits.
- **Protocols drive dispatch** — `Int` tells backends how to add, subtract, etc.
  The backend knows the default ops for every protocol. User only writes `op`
  overrides when deviating from the default.
- **Width is inferred** unless `!> bits: N;` is explicit.
- **`x.#Property` replaces `x :> Property`** for accessing type properties
  (Size, Capacity, Ptr, etc.). The `:>` operator is deprecated.
- **`match` in `$defn`** — `match expr { pattern => body; _ => body; };`
  supports integer, string, and wildcard patterns.
- **`=>` token** added for match arm syntax.
- **`<~` removed** in favour of `!> key: value;` for all metadata properties.

```briev
// Protocol default — nothing to override:
type Int: Int;
type String: String;

// Derives from parent, inherits protocol:
type i64: Int { !> bits: 64; };

// Override only what's different from the protocol default:
type MyString: String {
    op Add(String): weird_interaction(#L, #R);
};
```

### Types Have No Fixed Layout

A type `String` does not have a fixed `{i64, i64}` layout. It has whatever
shape the optimizer selects for the program's actual usage. The protocol
contract (`String`) tells the backend what operations are valid; the backend
picks the representation — inline SSO, heap-allocated, rope tree — based on
the program's operation profile. This is what makes the type system
**width-agnostic and layout-agnostic**.

## 2026-07-30 Update: Casting Graph — Protocols Are Guarantees, Types Are Overlays

The `op Cast()` mechanism is removed. Cast dispatch is now a **casting graph**
where every base protocol has a hardcoded direct lane to every other base
protocol. `operator_defs` no longer carries Cast/CastTo/CastFrom — only
non-cast operators (`InsertAt`, `ExtractFrom`).

### Four-Layer Protocol Hierarchy

> **2026-08-15 (Fundamentals as Types).** `Data` is now the universal
> reflective floor (every value observable as raw storage — NOT a supertype;
> no universal inheritance edge); `Bit<N>` is the unified bit type at any
> declared width (`Bit` bare = flexible); the byte-buffer type is renamed
> `Blob`. The category hashwords `Int`/`Float`/`String`/`Bit`/`Data`
> lose their `#` in fundamental positions; protocol variants
> (`String<UTF8>`) keep theirs. See
> `docs/plans/2026-08-15-fundamentals-as-types.md`.

```
Layer 1: Data (universal reflective floor); Blob (the [len][bytes] byte buffer)
  Observe TO  Data = raw storage view (the treat-as-bits material view, never a supertype edge)
  Cast FROM Data = interpret raw bytes as target protocol semantics
  Bit<N> = the unified bit type — every type is composed of bits;
           Bit bare = flexible width (resolved later), Bit<N> = exact N.

Layer 2: Base protocols (hardcoded in compiler)
  Int, UInt, Float, String, Bool, Char, Data, Blob
  Each has a hardcoded direct lane to every other base protocol.
  Each knows its LLVM type representation.
  Each knows its operations (Add, Sub, Mul, etc.).

Layer 3: Sub-protocols / variants (stdlib proto declarations)
  proto ASCII: String { CastTo(String): ascii_to_utf8(#L); };
  proto UTF16: String { CastFrom(String): utf16_from_utf8(#L); };
  Normalizer reads these, feeds edges into the casting graph.

Layer 4: User types (stdlib type declarations)
  type String: Data;   // fundamentals refine through Data
  type Int32: Int { spec Bits: 32; };
  All behavior inherited from parent/protocol. No body needed.
```

### Core Principle: Protocols are Guarantees, Types are Overlays

Every base protocol has a hardcoded direct lane to every other base protocol.
These lanes are **compiler guarantees** — they always exist, they always work
the same way, and they cannot be broken, removed, or overloaded.

Types can extend, override, and customize on top of the protocol guarantees.
A type-level override of `CastTo(Int)` changes what happens when *that
specific type* reaches `Int`, but the `String → Int` lane itself is
unchanged — available for any other `String` type that doesn't override it.

### Primitives vs Primordials: What's Overrideable

| | Primitive | Primordial |
|---|---|---|
| **Examples** | `Data`, `Bit<N>` | `Int`, `Float`, `Bool`, `Char`, `Blob`, `Void` |
| **Where defined** | Hardcoded in compiler (`seed_primordial_types`, before the PRIMORDIALS loop) | Seeded by compiler in PRIMORDIALS loop |
| **Overrideable?** | No — error if any stdlib or user code declares `type Data` / `type Bit` | Yes — bootstrap.bv or user `.bv` files replace the seeded entry silently |
| **Why** | Axiomatic anchors — the whole system rests on them | Useful defaults — stdlib can specialize them |

`Data` is the universal reflective floor: the compiler's root axiom,
non-negotiable, unoverridable — every value can be observed as its raw
storage, but it is NOT a supertype and adds no universal inheritance edge.
`Bit<N>` is the bit type at any width — the direct representation of N bits. Any attempt to declare `type Data` or `type Bit`
in stdlib or user code produces a compiler error. Everything else — `Int`,
`Float`, `String`, `Blob` — is a primordial: a useful default that stdlib or
user code can refine or replace. If bootstrap.bv declares `type Int: Int {
spec Bits: 32; }`, that replaces the primordial `Int` entry.

The **`Data` protocol** is similarly hardcoded in the casting graph — its
lanes to every other protocol are compiler guarantees. But protocols are not
types: a type participates in a protocol (`type Int32: Int`), and that
protocol membership can be set freely by the standard library.

**Using `Data` as a protocol is fully legitimate.** You can create new types
that participate in it:

```briev
type ReorganisedBit: Bit {
    spec Bits: 42;
    op CastTo(String): my_custom_encode(#L);
    op CastFrom(String): my_custom_decode(#L);
};
```

This declares a 42-bit type that uses `Bit` protocol semantics (raw bits,
bitwise operations) but with custom encoding/decoding to strings. What you
cannot do is touch `type Bit` itself — that concrete type is the
compiler's axiom. But using `Bit` protocol membership for your own types
is exactly what the system is designed for.

### The `→ Bit` Ban and the `← Bit` Door

`Bit` is where all protocols meet as equals:

- **`op CastTo(Bit)` is banned at declaration time.** `"CastTo(Bit) is
  hardcoded — use x as Bit or Cast#(x, target) for bitcasts."` Casting TO
  `Bit` is a **representation guarantee**: the compiler always does the
  mechanical job (bitcast, extractvalue, ptrtoint) with zero semantic
  transformation. No type overrides this.

- **`op CastFrom(Bit)` is the sole user-extensible cast edge.** It is the
  **interpretation door** — a type declares how to construct itself from raw
  memory bits. This is the one place where user code gives meaning to `Bit`.

- **`op CastTo/CastFrom(Category)` for non-`Bit` categories** remains
  allowed. `type AutoString: String { op CastTo(Int): my_parse(#L); };`
  registers a type-level lane override. The graph always prefers a type-level
  override over the protocol default.

Three-way priority in the casting graph's `emit_cast()`:
1. **Type-level override** — if the specific src→dst pair has one
2. **Protocol default** — the hardcoded lane between the two base protocols
3. **`CastFrom(Bit)` constructor** — if the target type declares it and the
   path passes through `Bit`

Step 3 never applies when the target IS `Bit`.

### How the Casting Graph Resolves `x as Target`

1. Determine `(src_protocol, src_variant)` and `(dst_protocol, dst_variant)`
   from the types involved.
2. Check for a type-level override on the specific src→dst pair. If found,
   emit it.
3. If src and dst are both base protocols (no variants), use the hardcoded
   direct lane — O(1), no BFS.
4. If variants are involved, BFS through variant edges + base lanes to find
   a path. If found, emit each step.
5. If no path exists through the graph, fall through to LLVM coercion
   (inttoptr, sitofp, bitcast, etc.) for trivial cases.

All `→ Bit` lanes bypass steps 2–5: they always emit the hardcoded
mechanical transformation. `CastFrom(Bit)` overrides are checked only when
the target is a concrete type that declared one.

---

## Preamble

Most compilers hardcode primitive types (`Int`, `Float`, `Bool`, `String`) into
their intermediate representations and interpreter internals. This makes the
compiler simple to write initially, but permanently locks the language's
semantics: a user cannot define their own `Int` with saturating arithmetic,
their own `Float` with a different rounding mode, or their own `Bool` with
three-valued logic without modifying the compiler itself.

The Bits thesis rejects this compromise. It treats the compiler as a pure
execution engine for **uninterpreted bit-vectors**, where all semantic meaning
is injected dynamically from the type universe at compile time. The compiler
frontend knows nothing about integers, floats, strings, or pointers — it only
knows about bits, and everything else is a metadata overlay.

---

## The Three Axioms

The entire Briev language is built from exactly three hardcoded assumptions.
Everything else — every type, every operation, every data structure — follows
from these axioms and is defined in the standard library prelude.

### Axiom 1: `Data` Is the Universal Reflective Floor; `Bit<N>` Is the Bit Type

> **2026-08-15.** Originally `Bit` was the root protocol. Under
> Fundamentals-as-Types, `Data` is the universal reflective floor (every
> value observable as its raw storage — NOT a supertype, no universal
> inheritance edge); `Bit<N>` is the unified bit type at any declared width
> (every type is composed of bits, and `Bit<N>` names a run directly). The
> universal treat-as-bits material membership lives on `Bit`/`Bit<N>`;
> `Data` is the reflective raw-storage floor. The byte-buffer is `Blob`. See
> `docs/plans/2026-08-15-fundamentals-as-types.md`.

`Data` is the universal reflective floor — every value can be observed as
its raw storage, but it is not a supertype: no implicit `Data` edge is added
to the casting graph. `Bit<N>` is a contiguous sequence of N uninterpreted
bits (`Bit` bare = flexible width, resolved later). `Data` and `Bit<N>` are
the only types the compiler knows about axiomatically. Every other type and
protocol is observable as raw storage through the reflective floor.

```
Data — universal reflective floor, hardcoded in compiler
  Observe  Data = raw storage view (treat-as-bits material view, never a supertype edge)
  Cast FROM Data = interpret raw bytes as target semantics (overridable via op CastFrom(Data))
Bit<N> — the bit type at any width; every type is composed of bits
  Bit bare = flexible width (resolved later); Bit<N> = exact N bits
```

`Data` is special-cased in the casting graph: it has no base parent because
it *is* the base. Every other protocol has a direct lane to `Data` and a
direct lane from `Data`:

```
type Int:     Data;       // Int is-a Data → i64 LLVM type
type Float:   Data;       // Float is-a Data → double
type Bool:    Data;       // Bool is-a Data → i8
type Char:    Data;       // Char is-a Data → i32
type String:  Data;       // String is-a Data → ptr to [len][bytes]
type Blob:    Data;       // Blob is-a Data → ptr to [len][bytes] (no encoding)
type Void:    (no width)  // zero-width, no bits
```

The only property the frontend hardcodes at the protocol level is the LLVM
type representation for each base protocol. Width is derived from the `!> bits`
metadata when explicit, or from the protocol default otherwise.

### Axiom 2: Bitwise Operations Are the Laws of Physics

The `Bits` type intrinsically supports the six operations of boolean algebra
without consulting any `op` bindings in the type universe:

| Operation | Symbol | Semantics |
|-----------|--------|-----------|
| Bitwise AND | `&` | Byte-wise AND |
| Bitwise OR  | `\|` | Byte-wise OR  |
| Bitwise XOR | `^` | Byte-wise XOR |
| Bitwise NOT | `~` | Byte-wise complement |
| Shift left  | `<<` | Byte array shift |
| Shift right | `>>` | Byte array shift |

These are the only operations the interpreter applies to `Value::Bits` without
looking up type properties. They represent the native laws of physics of the
underlying hardware — wires and gates. All other operations (addition,
multiplication, comparison, string concatenation, etc.) are **semantic
interpretations** bound to bits via the `op` metadata system.

### Axiom 3: Runes Map to `op` Bindings

The Briev language surface has operator symbols (runes): `+`, `-`, `*`, `/`,
`==`, `<`, `>`, `[]`, `<-`, etc. The frontend hardcodes the mapping from each
rune to an `op` contract name:

| Rune | `op` name | Purpose |
|------|-----------|---------|
| `+`  | `op Add`  | Addition |
| `-`  | `op Sub`  | Subtraction |
| `*`  | `op Mul`  | Multiplication |
| `/`  | `op Div`  | Division |
| `==` | `op Eq`   | Equality |
| `<`  | `op Lt`   | Less-than |
| `[]` | `op ExtractFrom` | Index read |
| `<-` | `op InsertAt` / `op DiscardAt` | Collection mutation |
| `term` | `op Term` | Return/implicit terminal |

The type determines the *binding* for each `op`. The binding is either a
compiler intrinsic (identified by a trailing `#`) or a standard Briev function
(no trailing `#`):

```briev
type Int: Int {
    op Add(Int): add(#L, #R);       // compiler intrinsic — backend emits i64 add
};

type Complex: Int {
    real: Float;
    imag: Float;
    op Add(Int): complex_add(#L, #R);  // user function — no intrinsic needed
};
```

Intrinsics route to the interpreter's `execute_intrinsic` table, which
performs the operation on raw byte arrays. User functions are AST-level
rewrites: `a + b` becomes `complex_add(a, b)` before codegen.

---

## Derived Concepts

From these three axioms, the entire language emerges.

### 1. The Universal Value: `Value::Bits(Vec<u8>)`

The interpreter has a single representational value type. Every value in the
Briev universe — from a boolean to a 10-megabyte database index — is
represented internally as a raw byte sequence:

```rust
pub enum Value {
    /// The ONLY representational storage cell for program data.
    /// 2026-07-11: All scalars, structs, and collections are bits.
    Bits(Vec<u8>),

    // Compiler-internal meta-objects (not representational data)
    Defn(String),
    Void,
    Ref(Box<Value>),
    Expr(Box<Expr>),
    Stmt(Box<Statement>),
    Block(Vec<Statement>),
    Items(Vec<TopLevel>),
    Type(Type),
    Regex(RegexPattern),
    DbvlTable(Arc<DbvlTableInner>),
}
```

No `List`, no `HashMap`, no `Tuple`, no `Instance`, no `Enum`. A `List<T>`
containing a million elements is `Value::Bits(24 bytes)` — the struct layout
`{ ptr: Ptr<T>, len: Int, cap: Int }`, where `ptr` is a **virtual memory
address** into the interpreter's sandboxed heap (see §6).

#### VirtualHeap: The Compile-Time Memory Model

The interpreter maintains a sandboxed virtual memory space for compile-time
execution. This is the same pattern used by Miri (Rust's compile-time
interpreter) and every safe partial evaluator:

```rust
/// Sandboxed heap for compile-time allocation and pointer arithmetic.
/// 2026-07-11: Phase 8A — enables List, HashMap, Box etc. as pure Bits.
pub struct VirtualHeap {
    allocations: HashMap<u64, Vec<u8>>,
    next_address: u64,
}
```

When compile-time code runs `list.push(val)`:
1. The intrinsic `__list_insert#` receives `Value::Bits(24)` representing
   `{ ptr, len, cap }` and `Value::Bits(N)` representing `val`
2. It copies the virtual address from the struct, checks bounds, optionally
   allocates new memory in the VirtualHeap
3. It writes the new element's bytes at `ptr + len * sizeof<T>()` in the heap
4. It returns a new `Value::Bits(24)` with updated `len`

This is a `HashMap::get` + `Vec::extend_from_slice` at compile time —
O(1), no different from the current `Value::List` access path.

#### Prelude Cache

To avoid re-evaluating the standard library on every compilation, the
interpreter state is cached after first evaluation:

```
cache/prelude.bincode:
  - VirtualHeap allocations (type structures, string constants)
  - Type universe (resolved types, operator bindings)
  - FFI registry entries

Invalidation: file timestamps on lib/std/*.bv + compiler version hash
```

On subsequent compilations, the cached prelude is deserialized instead of
re-evaluated. The VirtualHeap allocation cost for prelude types is paid
once, not once per build.

### 2. `Void = Bits(0)`

The `Void` type is not a base type. It is a zero‑width specialization of
`Bits`:

```briev
type Void {
    !> maxbits: 0;
    !> alignment: 1;
};
```

Because `maxbits <~ 0` (zero-width), the type resolver allocates zero bytes for
any slot of type `Void`. The compiler's frontend has zero hardcoded knowledge
of "Void" as a concept. Void has no protocol membership — it is pure zero-width
`Bit`.

### 3. `Box<T>` Is a Struct with `op Drop`

Memory management is not a compiler intrinsic. `Box<T>` is a library type:

```briev
type Box<T>: Bits {
    ptr: Ptr<T>;
    op Drop(self) = __free_heap_allocation#;
};
```

The compiler tracks variable lifetimes. When a value of a type implementing
`op Drop` goes out of scope, the compiler inserts a call to the destructor.
If no `op Drop` exists, the compiler generates zero deallocation code.

This replaces the old `storage <~ "Boxed"` magic string. The compiler does not
need to know what a "box" is — it only needs to know whether `op Drop` exists.

### 4. Collections Are Stdlib Structs

`List`, `HashMap`, `HashSet`, `Stack`, `Queue` are not compiler primitives.
They are structs defined in `lib/std/` that manage pointers to heap memory:

```briev
type List<T>: Bits {
    ptr: Ptr<T>;
    len: Int;
    cap: Int;
    op Drop(self) = free_list_allocation;
    op InsertAt(self, index: Int, val: T) = list_insert;
    op ExtractFrom(self, index: Int) -> T = list_get;
};
```

Indexing a list (`list[i]`) routes through `op ExtractFrom`, which computes
`ptr + i * sizeof<T>()` and dereferences. The interpreter evaluates this as a
regular function call. No special-casing for "list operations."

### 5. Protocol-Level Metadata Resolution

The frontend recognizes a fixed set of metadata properties:

| Property | Purpose | Hardcoded? |
|----------|---------|------------|
| `bits` | Bit width of the type | Yes (Axiom 1 — width is fundamental) |
| `alignment` | Memory alignment | Yes (layout engine) |
| `op X` | Operator binding | Yes (Axiom 3 — rune→op, not op→intrinsic) |
| `llvm_type` | LLVM type override | **No** — derived from protocol+metadata by normalizer |

LLVM types are resolved from `(protocol, metadata)` by the normalizer:

| Protocol | Metadata | LLVM type |
|----------|----------|-----------|
| `Int` | (none) | `i64` (default 64-bit) |
| `Int` | `!> bits: 8` | `i8` |
| `Int` | `!> bits: 32` | `i32` |
| `Float` | (none) | `float` (default 32-bit) |
| `Float` | `!> bits: 64` | `double` |
| `String` | (none) | `{ i64, i64 }` |
| `Bool` | (none) | `i8` |
| `Char` | (none) | `i32` |
| `Bit` | (none) | `i64` (default) |
| `Data` | (none) | `ptr` |

The old `primitive <~ "Int"` / `alu <~ "Int"` / `llvm <~ "i64"` metadata
properties are **removed**. Hashword protocol membership (`Int`, `Float`,
etc.) replaces all three. The frontend matches on protocol membership via
`is_protocol_member()`, not on string values of metadata tags.

Any property the frontend does not recognize is stored, serialized to the
`.bvsa` archive, and ignored. Only backend-specific tooling (e.g.,
`briev-llvm`, `briev-circt`) interprets it.

### 6. The Three Token Forms (Compiler Axioms)

The parser produces exactly three primitive token forms, each representing
raw bytes from source text with no semantic interpretation:

| Form | AST node | Source | `formatting <~` value |
|------|----------|--------|----------------------|
| QuotedValue | `Expr::Quoted(Vec<u8>)` | `"..."` | `Quoted` |
| DecimalValue | `Expr::Decimal(i64)` | `[0-9]+`, `[0-9]+\.[0-9]+` | `Decimal` |
| Bareword | `Expr::Identifier(String)` | `[a-zA-Z][a-zA-Z0-9_]*` | `Bare` |

These are compiler axioms — they must exist because the lexer must produce
something. All semantic meaning is attached by the type's codec via the
`formatting <~` property:

```briev
codec HexColor {
    formatting <~ Bare;     // ← FF00FF is accepted
    parse      <~ parse_hex;  // converts text to Value::Bits
};
```

The `@` prefix modifier converts any token to `QuotedValue(raw_bytes)`,
bypassing lexer interpretation. `@FF00FF`, `@42`, `@"..."` all produce
`Expr::Quoted(bytes)`.

No name-based magic. `String` accepts `"..."` because `String` declares
`op Parse(String)` — the identity parse form, meaning the quoted bytes
are already valid UTF-8. (Legacy: `DefaultQuoted.formatting <~ Quoted`
still works but is deprecated in favour of `op Parse`.)

### 6.1 Parse Protocol — Replacement for `formatting` metadata

**2026-07-20:** The `formatting` metadata property and the `codec`
declaration form are superseded by the `op Parse` protocol:

```briev
// Old (codec + formatting):
codec HexColor {
    formatting <~ Bare;
    parse      <~ parse_hex;
};

// New (op Parse):
type HexColor {
    data: Bits<24>;
    op Parse(Bare): parse_hex(#L);   // Bare literal "FF00FF" → HexColor
};
```

| Old mechanism | Replaced by |
|---|---|
| `formatting <~ Bare` + `parse <~ parse_hex` | `op Parse(Bare): parse_hex(#L)` |
| `formatting <~ Decimal` + `parse <~ parse_fn` | `op Parse(Decimal): fn(#L)` (or `op Parse(Int)` for identity) |
| `formatting <~ Quoted` + `parse <~ identity` | `op Parse(String)` or `op Parse(Quoted): fn(#L)` |
| `DefaultQuoted` codec class | Inline `op Parse` on each type definition |
| `<~` property assignment syntax | `!> key: value;` throughout |

**Why the change:** The `op` system already provides dispatch, `alwaysinline`,
positional markers (`#L`, `#R`), and inheritance through `:`. `op Parse`
integrates literal construction into the same system rather than maintaining
a parallel `codec` mechanism. Additionally, `op Parse(#Category)` provides a
zero-cost identity path: when the target type IS the protocol shape,
parsing is a no-op.

**The three token forms (QuotedValue, DecimalValue, Bareword) remain compiler
axioms** — only the dispatch mechanism changes from `formatting` metadata
to `op Parse` signatures.

### 6.2 Round-Trip Verification of Parse Ops

Every `op Parse(Form) = fn(#L)` declaration triggers compile-time symbolic
execution to verify that parsing is invertible:

1. The compiler applies `fn` to a representative constant literal
2. The compiler applies the corresponding produce op (CastTo or Cast) to
   the result
3. The compiler asserts that step 2 produces the original literal bytes

For a type like `HexColor` with `op Cast(String)`:
```
Parse("FF00FF") → 0xFF00FF  →  Cast(String) → "FF00FF"  ✓  Round-trip OK
```

If the round-trip fails (e.g., a hash function that loses information),
the compiler emits a warning but continues. Non-invertible types must
document this with an explicit annotation.

### 7. The Complete Protocol Graph from First Principles

Every type participates in a protocol. Nothing is special-cased:

```
Bit (root protocol, compiler axiom)
  │
  ├── Int       → i64 LLVM, Add = native i64 add
  │     ├── type Int: Int             (default 64-bit signed)
  │     ├── type UInt: Int            (same bits, unsigned interpretation)
  │     ├── type Int8:  Int { !> bits: 8; }
  │     ├── type Int16: Int { !> bits: 16; }
  │     ├── type Int32: Int { !> bits: 32; }
  │     ├── type Int64: Int { !> bits: 64; }
  │     └── type Data:  Int  { !> bits: 64; }  (pointer-width alias)
  │
  ├── Float     → double (default 64-bit), Add = native fadd
  │     ├── type Float:  Float
  │     ├── type Float32: Float { !> bits: 32; }
  │     ├── type Double:  Float { !> bits: 64; }
  │     └── type Half:    Float { !> bits: 16; }
  │
  ├── String    → {i64, i64}, Add = concat
  │     ├── type String: String
  │     ├── proto ASCII: String { CastTo(String): ascii_to_utf8(#L); };
  │     └── type ASCIIStr: String<ASCII>
  │
  ├── Bool      → i8, Eq = icmp ne
  │     └── type Bool: Bool { !> bits: 8; }
  │
  ├── Char      → i32, Eq = icmp eq
  │     └── type Char: Char { !> bits: 32; }
  │
  └── Data      → ptr, CastTo(Int) = ptrtoint
        └── (implicit on every pointer type)
```

Everything on the right side of `│` is defined in the standard library
prelude (`bootstrap.bv`), not in the compiler's Rust code — except the
base protocols themselves, which are hardcoded in the casting graph.

A user could omit the prelude entirely, define their own `Int` with
saturating arithmetic, their own `List` with arena allocation, their own
`String` with a different encoding — and the compiler would handle them
identically because it only sees protocol membership + metadata. There is
no "stdlib" path and "user" path in the compiler. There is only one path.

### 7. SMT Solver Alignment

#### 7.1 Eliminating the Translation Gap

In traditional solver-aided compilers, the compiler must bridge between
its high-level type representation and the SMT solver's logic. Translating
an algebraic enum, a struct with padding, or a string into SMT-LIB requires
complex lowering rules that are brittle and hard to verify.

Under the Bits thesis, a type's physical representation in the compiler is
identical to its representation in the SMT solver:

| Briev type | Compiler value | SMT-LIB sort |
|-----------|---------------|--------------|
| `Int` | `Value::Bits(8 bytes)` | `(_ BitVec 64)` |
| `Bool` | `Value::Bits(1 byte)` | `(_ BitVec 8)` |
| `String` | `Value::Bits(24 bytes)` | `(_ BitVec 192)` |
| Custom struct | `Value::Bits(N bytes)` | `(_ BitVec N*8)` |

The translation function is a single match arm:

```rust
fn value_to_smt(value: &Value) -> SmtExpr {
    match value {
        Value::Bits(bytes) => SmtExpr::Bitvector(bytes.len() * 8, bytes),
        _ => unreachable!(), // meta-objects never reach the solver
    }
}
```

No type dispatch. No enum-variant branching. The frontend and the solver
speak the same bit-level language. The mathematical model can never diverge
from the runtime behavior because they are the same representation.

#### 7.2 Bit-Blasting Performance

SMT solvers are exceptionally fast at solving bit-vector constraints because
of **bit-blasting**: every bit of a `(_ BitVec N)` variable becomes a boolean
variable, and operations become networks of primitive logic gates. These gate
networks are fed to the solver's CDCL (Conflict-Driven Clause Learning) SAT
engine, which can solve millions of boolean variables in microseconds.

Because Briev values are already raw bytes, the compiler can emit SMT-LIB
bit-vector constraints directly — no type-to-logic lowering step. The solver
receives the same bit-level problem the hardware would execute.

#### 7.3 Elimination of the Modulo Arithmetic Tax

If a solver models machine integers using Linear Integer Arithmetic (LIA),
it must enforce wrapping on every operation:

```
; LIA model of 64-bit addition — expensive for the solver
(assert (= result (mod (+ a b) 18446744073709551616)))
```

In QF_BV, wrapping is native to the representation:

```
; QF_BV — wrapping is free, the bit-vector is finite
(assert (= result (bvadd a b)))
```

The solver's bit-blasting circuit for `bvadd` naturally wraps at 64 bits,
just like physical CPU silicon. Overflow checks, saturating arithmetic, and
bounds verification are all derived from the same native bit-vector
operations — no modulo constraints needed.

#### 7.4 Elimination of Pointer Aliasing Complexity

In SMT solvers, modeling a heap with arbitrary pointer aliasing requires
expensive array theory axioms (select/store with frame conditions). Because
Briev's value semantics and linear ownership guarantee that variables are
unaliased, independent bit-vectors, the solver never needs to reason about
aliasing. Every variable maps to a fresh `(_ BitVec N)` constant. The
solver's state space is flat and localized — no interconnected pointer web.

#### 7.5 Impact on Synthesis and Verification

The synthesis engine (Phase 9) and contract verifier (Phase 10) both
translate function bodies and contracts into SMT-LIB queries. Because the
input is already `Value::Bits`:

- The solver interface is a single function, not a type-directed translator
- Example values from `:=` derivation blocks map directly to solver constants
- Contract pre/post-conditions become bit-vector constraints without lowering
- The solver's counterexample models are trivially convertible back to
  `Value::Bits` — no reification step needed

This alignment is not an accident of implementation. It follows directly from
the Bits thesis: if everything is bits at the compiler level, everything is
bits at the solver level, because the solver is just another consumer of the
same representation.

---

## How This Enables Decoupled Backends

The `.bvsa` (Briev Value Semantic Archive) is a serialized representation of
the typed program. It contains:

- Type definitions with all properties (frontend- and backend-intrinsic)
- Function bodies as AST nodes
- Operator bindings as property references

A decoupled LLVM backend reads the `.bvsa` archive. For each type, it queries
the casting graph to determine the LLVM type representation from the type's
protocol membership and metadata. It never asks the frontend "is this an Int?"
— it only checks protocol membership via `is_protocol_member()`.

A decoupled CIRCT backend reads the same `.bvsa` archive. It ignores LLVM
types and computes its own hardware representation from `!> bits` and
protocol membership. The frontend does not need to know which properties a
backend will use.

---

## Sufficiency Proof

These three axioms are sufficient to build an entire programming language's
behavior because:

1. **All data is bits.** Every value in every program is a sequence of bytes.
   The interpreter can store, copy, and compare any value without knowing
   what it represents.

2. **All computation reduces to bit manipulation.** Addition, subtraction,
   comparison, indexing — every operation is a transformation on byte arrays.
   The intrinsics table maps named operations (`"__add_i64"`) to concrete
   byte-array logic. Backends map the same names to native instructions.

3. **All semantics are protocol membership.** The meaning of a value — whether
   it is an integer, a float, a pointer, or a color — is not in its bits. It
   is in the protocol the type participates in. Changing protocol membership
   changes the semantics without changing the bits. The casting graph
   guarantees every protocol can reach every other protocol — no type is
   ever stranded.

4. **All extension is user-defined.** A new type, new operator, new backend
    target — none require compiler changes. Types are declared, operators
    bound, backends subscribe to properties they understand.

---

## FAQ

### Q1: If everything is Bit (the Bits thesis), why does the compiler still have `Int`, `Float`, `Bool` as separate protocols?

**They are not separate primitives — they are protocol contracts defined in
the casting graph.** The compiler's casting graph has hardcoded lanes between
base protocols, but the type checker never matches on protocol names. It
checks protocol membership via `is_protocol_member()`:

```briev
type Int: Int;     // Int participates in Int protocol → backend knows to use i64 ALU
type Float: Float; // Float participates in Float protocol → backend uses float ALU
type Bool: Bool;   // Bool participates in Bool protocol → backend uses i1 compare
```

The compiler frontend never matches on the name `"Int"`. It checks
`is_protocol_member(ty, "Int")` which queries the casting graph — a
reachability check, not a name match.

The only place protocol names appear as hardcoded concepts is the
`ReturnKind` enum, which is a **compiler-to-backend contract**, not a type
system feature:

```rust
ReturnKind::Native("Int")   // backend: "emit 64-bit integer ops"
ReturnKind::Native("Float") // backend: "emit 64-bit float ops"
ReturnKind::Native("Bool")  // backend: "emit 1-bit bool ops"
```

These tell the backend what LLVM IR to emit. They are not type judgments.

### Q2: What does each consumer in the pipeline see?

| Consumer | Sees | Action |
|----------|------|--------|
| Parser | raw bytes + token forms | No type knowledge |
| Type checker | protocol membership + metadata | Structural comparison on width |
| Casting graph | (protocol, variant) pairs | BFS for cast path between protocols |
| SMT solver | `(_ BitVec N)` | Ignores all metadata |
| CIRCT backend | protocol + `!> bits` | Uses bits for hardware width |
| LLVM backend | protocol + metadata | Resolves LLVM type from `(protocol, metadata)` |
| Meld/FFI | protocol + shape/mapping | Enforces C layout compatibility |

The SMT solver and CIRCT backend **never need to know about `Int` or `Float`**.
They operate on pure bit-vectors. The casting graph is only consumed by the
LLVM backend to resolve cast paths between types.

### Q3: Does the type checker force me to coerce types at every boundary?

**No.** The type checker compares types structurally by width. Protocol
membership (`Int`, `Float`, etc.) is **not part of the comparison key**.
An `Int` and a `Float` both have 64-bit width, so the type checker sees
them as compatible at the structural level. Coercion only matters at the
LLVM codegen level, where `is_protocol_member(ty, "Int")` vs
`is_protocol_member(ty, "Float")` determines which ALU instruction to emit.

The exception is explicit `meld` (FFI) declarations, where C's type system
requires specific layout guarantees. That's an opt-in mechanism, not the
default path.

### Q4: Are CPU types (`Int`, `Float`) real, or are they just convenience?

**Both.** CPU architectures have evolved to the point where specific bit
patterns trigger specific hardware units:
- `i64` → general-purpose ALU (integer add, mul, etc.)
- `double` → float ALU (fadd, fmul, etc.)
- `i1` → branch condition (je, jne, etc.)

These are genuine physical realities of the hardware. `Int` protocol
membership routes a 64-bit value to the integer ALU. `Float` routes
it to the float ALU. The same 64 bits go into different silicon, but
they're still bits.

The Bits thesis does not deny this. It says: **the bits are the true
representation. The protocol tag is a routing hint for backends that
have multiple ALUs.**

### Q5: Does this mean `ReturnKind::Native("Int")` could map to `i32` on an embedded target?

**Yes, exactly.** On x86_64, `Int` → `i64`. On a 32-bit ARM target, the
same intrinsic could map to `i32`. The `.bv` source doesn't change — only
the backend's interpretation of `Int` changes. The `!> bits` metadata on
`Int` would be set per-target:

```briev
// x86_64 backend: !> bits: 64 → i64
// ARM32 backend:  !> bits: 32 → i32
type Int: Int { !> bits: TARGET_PTR_SIZE; };
```

All algebraic operations on `Int` automatically use the right width because
they operate on the protocol's native integer width for the target.

### Q6: If protocol membership is ignored by the type checker, how does `is_protocol_member()` affect anything?

**It doesn't affect the type checker.** It only affects the LLVM backend.
The pipeline is:

1. Type checker: verifies structural width compatibility → **protocol membership ignored**
2. Casting graph: resolves `(protocol, variant)` to cast path → emits LLVM IR
3. LLVM backend: checks `is_protocol_member()` → emits `add i64` or `fadd double`

Protocol membership travels through the pipeline **opaque to the type
checker**. It is only consumed by the casting graph and the LLVM backend,
never by structural type comparison.

### Q7: Is the Bits thesis hogwash? Should we reintroduce primitives as first-class compiler concepts?

**No — the Bits thesis is correct, and primitives should NOT be reintroduced
as first-class compiler concepts.**

The Bits thesis is the deepest correct description of computation: everything
is `Bit`. The protocol layer (`Int`, `Float`, etc.) is a thin routing
surface that only the LLVM backend consumes. Reintroducing primitives as
first-class AST types would:
- Duplicate the width information already present in protocol metadata
- Require special-casing in the type checker, SMT solver, and CIRCT backend
- Break the axiom that every type participates in a protocol
- Force users to learn about primitives when defining custom types

The current architecture — `Bit` at the core, protocol membership for
dispatch, `ReturnKind` as compiler-to-backend contract, casting graph for
cross-protocol conversion — is the right balance.

### Q8: How does FFI fit into this? C expects specific types.

The `meld` keyword bridges Briev's type system to foreign (C) ABIs. It
explicitly maps Briev types to C types with layout guarantees:

```briev
meld type FileHandle: C "int" {
    !> bits: 32;
    !> signed: true;
};
```

The FFI path is **opt-in** — you explicitly declare when a type needs C
compatibility. The `meld` declaration provides shape and mapping metadata
that the backend uses to emit the correct C ABI. Outside of `meld`,
everything is `Bits(N)`.

### Q9: Why does the compiler have `ReturnKind::Inferred` if everything is supposed to be explicit about `Bits(N)`?

`Inferred` is not about **type inference at the language level**. It's about
**argument-to-return type propagation at the intrinsic level**. A concrete
example:

```
Add#(a: Bits(64), b: Bits(64)) → Bits(64)  // return = same width as args
Add#(a: Bits(32), b: Bits(32)) → Bits(32)  // return = same width as args
```

`Inferred` says "the return type is derived from the argument types, not
declared independently." This is only used for polymorphic intrinsics like
`Add#`, `Sub#`, `Mul#`. Most intrinsics use `Native("Int")` or
`Exact(Type)`.

### Q10: If CIRCT and SMT ignore metadata, how does CIRCT know the width of a type?

CIRCT reads the bit width from `!> bits` metadata, which is Axiom 1.
`!> bits: 64` → hardware `uint64_t` or equivalent. Protocol membership
(`Int` vs `Float`) is irrelevant for hardware synthesis — CIRCT only
needs bit widths and dataflow connections. This is by design: **hardware
doesn't have separate integer and float ALUs at the RTL level, it has
wires and gates.**
