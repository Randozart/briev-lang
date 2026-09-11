# Defining Custom Types

Briev lets you define types that behave like built-in `Int`, `Float`, and
`String`. The fundamentals are compiler-native primordials — your custom
types inherit from them (`type MyInt: Int`) and gain the protocol family
for free, no special compiler support needed.

## 1. `type MyType : Fundamental { ... }`

Use the `type` keyword with a parent fundamental to define a new type. Layout
comes from fields; the parent's protocol provides its own self-arithmetic;
overloads are declared **RHS-only** (the LHS is the declaring type):

```briev
type MyInt : Int {
    data: Bit<64>;               // layout: 8 bytes
    op Add: func(#L, #R);        // binding form — the generic self-add
    op Add(Float): func(#L, #R); // RHS-only overload: MyInt + Float
    op Parse(Decimal): parse_num(#L);  // literal construction: 42 → MyInt
};
```

- **Layout**: determined by fields (`data: Bit<N>`, `field: Type`)
- **Self-arithmetic is implicit**: an `Int`-refining member already knows how
  to add to itself — you only declare overloads whose RHS differs from the
  declaring type (`op Add(Float)`, declared on the LHS type)
- **The two-variant form is removed**: `op Add(Int)` never lists the
  LHS — the declaring type/parent IS the left operand
- **Parse ops**: `op Parse(Decimal): fn(#L)` — custom literal construction

No `!> maxbits:`, `!> alignment:`, `!> llvm:`, `!> storage:`, `default_width`,
`commuting`, or LLVM opcode strings needed. When a physical layout must be
pinned, declare it with `spec` (`spec Bits: 12;`, `spec Alignment: 2;`, `spec
Endian: Big;`, §8.2 of the spec) or a `pack`/`union` struct modifier — never
the `!>` spelling.

## 2. Protocol-Centric Ops

The fundamental in an op signature tells the backend to dispatch to its
intrinsic handler for that category:

```briev
type Bfloat16 : Float {
    data: Bit<16>;
    op Add(Float): bfloat_add(#L, #R);   // protocol-family RHS overload
    op Mul(Float): bfloat_mul(#L, #R);
    op Parse(Float): identity(#L);       // identity — literal IS float
};
```

| Op form | Meaning |
|---|---|
| `op Add: func(#L, #R);` | Binding form — operands are `#L`/`#R` placeholders |
| `op Add(Float): func(#L, #R);` | RHS-only overload for the concrete type `Float` |
| `op Add(Float): func(#L, #R);` | RHS-only overload for the whole `Float` family (a `Float`-refining member) |
| `op Parse(Decimal): fn(#L);` | Custom literal construction |

Functions bound to ops via `: fn(...)` are emitted with LLVM's `alwaysinline`.

### `prop` — Metaproperty Declarations

Alongside `op`, types declare metaproperties via `prop`:

```briev
type MyString: String {
    op CastTo(Bit) = my_encode(#L);
    prop Size = my_chars(#L);     // .^Len → character count
    prop Bytes = my_byte_len(#L); // .^^Bytes → encoded byte length
};
```

A `prop` declares a metaproperty accessible via reflection (`expr.^Name` / `expr.^^Name`). The compiler
resolves it through the parent chain — `String` provides `Size` and `Bytes`,
but a custom type can override them. Same resolution mechanism as `op`.

**Built-in metaproperties by fundamental:**

| Fundamental | Metaproperties |
|----------|---------------|
| `Bit<N>` | `.^^Bits`, `.^^Alignment` |
| `String` | `.^Len`, `.^^Bytes` |
| Any type | User-defined via `prop` |

## 3. Protocol Variants

Protocol variants parameterize the fundamental categories; the variant keeps
its `#` (it is a non-category role):

```briev
type ASCIIString {
    data: Bit<64>;
    len: Bit<64>;
    op CastTo(String<UTF8>) = ASCII_to_UTF8(#L);   // produce UTF-8
    op CastFrom(String<UTF8>) = UTF8_to_ASCII(#L);  // consume UTF-8
};
```

Bare fundamentals resolve to their default variant at parse time:

| Fundamental | Default variant | Also writable as |
|---|---|---|
| `String` | `UTF8` (for all files) | `String<UTF8>`, `String<ASCII>` |
| `Float` | `IEEE754` | `Float<IEEE754>` |
| `Char` | `unicode` | `Char<unicode>`, `Char<ASCII>` |

Cross-variant calls require explicit protocol. A `.bv` file calling an `.ebv`
function using `String` produces a compile error if the default variants
differ.

## 4. Literal Construction via Parse Ops

Types declare how they are constructed from source text:

```briev
type HexColor {
    data: Bit<24>;
    op Parse(Bare) = parse_hex(#L);              // FF00FF → HexColor
    op Cast(Bit);
};

type RomanNumeral {
    data: Bit<16>;
    op Parse(Bare) = roman_from_identifier(#L);  // XIV → RomanNumeral
    op CastTo(Int) = roman_to_int(#L);           // RomanNumeral → Int
};

type BFloat16 {
    data: Bit<16>;
    op Parse(Decimal, pre: "0x") = hex_to_bfloat(#L);  // 0x3F80 → 1.0
    op Parse(Decimal, suf: "bf") = suffix_bf(#L);       // 1.5bf → bfloat
    op Parse(Float);                                     // identity
};
```

When the compiler encounters a literal, it checks the target type's Parse ops:

1. `op Parse(Fundamental)` — identity, zero-cost, no conversion
2. `op Parse(Form, pre: "prefix")` — discriminator match, most specific
3. `op Parse(Form, suf: "suffix")` — discriminator match, most specific
4. `op Parse(Form)` — fallback, matches any literal of that form

## 5. Protocol Conversion (CastTo / CastFrom)

The `CastTo`/`CastFrom` pair handles conversion between types via the
fundamental category:

```briev
type Latin1String {
    data: Bit<64>;
    len: Bit<64>;
    op CastTo(String) = latin1_to_UTF8(#L);       // Latin1 → UTF-8
    op CastFrom(String) = UTF8_to_latin1(#L);      // UTF-8 → Latin1
};
```

At compile time, the compiler inlines both functions and LLVM's `InstCombine`
removes redundant operations. A Latin1→UTF-8→Latin1 round-trip for ASCII data
(0–127) collapses to a no-op.

Cast resolution priority:
1. `meld Source <-> Target  (legacy: use protocol casts)` — structural equivalence
2. `op Cast(Target)` on source — direct type-to-type
3. `CastTo(Fundamental)` → `CastFrom(Fundamental)` — parent/protocol path
4. Implicit `Cast(Bit<N>)` — raw bytes, always available

## 6. Protocol Satisfaction (Compile-Time Verification)

For every type with both a Parse op and a Cast/CastTo op, the compiler
performs symbolic execution at compile time to verify round-trip fidelity:

```
Parse("FF00FF") → 0xFF00FF → Cast(Bit) → "FF00FF"  ✓
```

If the round-trip fails, a warning with the exact input and output values
is reported.

## 7. Type Parameter Constraints

```briev
type HashMap<K: String, V> {
    data: Bit<64>;
    len: Bit<64>;
    cap: Bit<64>;
    op Insert(V) = hashmap_insert(#L, #R1, #R2);
    op Get(K) -> V = hashmap_get(#L, #R);
};
```

`K: String` checks that `K` refines the `String` fundamental's ops
(`CastTo(String)`, `CastFrom(String)`, `Extract(Char)`, `InsertAt(Char)`,
`Concat(String)`, `.^Len`). At instantiation, the concrete type is
verified against the parent.

## 8. Protocol Participation and GLUE Bridge

Types that declare `op CastTo(Fundamental)` gain automatic FFI support through
the GLUE bridge. When a function with a custom type parameter is exported:

1. The protocol BFS (`find_cast_path`) discovers the `Cast.Fundamental` property
2. The fundamental category is looked up in `lib/glue.toml`
3. The language-appropriate native type and C ABI type are selected
4. The `CastTo` function generates the conversion code at the boundary

```briev
// A custom type that refines the Int fundamental
type MyFixedInt {
    data: Bit<32>;
    op CastTo(Int);
    op CastFrom(Int);
};

// Exported to Rust: the bridge automatically maps Int → i64
export defn process(n: MyFixedInt) -> MyFixedInt {
    ...
};
```

No `lib/glue.toml` changes needed — the fundamental system discovers
`CastTo(Int)` from the type declaration. The TOML only needs to know
about the fundamental categories, not every custom type that refines them.

See `docs/architecture/protocol-types.md` for the full architecture doc.

