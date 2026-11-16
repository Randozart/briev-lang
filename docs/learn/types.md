# Types in Briev — Learning Guide

**Last updated:** 2026-07-09

## Type Derivation (`<:`)

Briev's type system is built on a small primitive kernel (~13 properties) that the compiler understands natively. Everything else is defined in user-space Briev.

The syntax for defining a type:

```briev
Type Name : Base {
    Property = Value;
    [ Constraint ];
};
```

### Scalars

```briev
Type U8  : Bits { Bytes = 1; Alignment = 1; };
Type U32 : Bits { Bytes = 4; Alignment = 4; };
Type Int : U64;
```

`Bits` is the only truly built-in type. `Bytes` and `Alignment` describe physical layout.

Each type also declares its operations (`op Add`, `op Sub`, etc.) which map to LLVM
instructions. See [Operator Declarations](#operator-declarations) below.

### Collections

Collections are defined with element type and access pattern metadata:

```briev
Type List<T> : Bits {
    ElementType = T;
    FixedSize = false;
    InsertAt = .#Size;
    ExtractFrom = .#Size - 1;
};
```

`ElementType = T` unlocks `[]` brackets. `FixedSize = false` unlocks `<-`/`->` arrows.

### Tuples

Tuples are fixed-size heterogeneous collections. Unlike `List<T>` (which holds zero or more elements of a single type), a Tuple's length and element types are part of its type signature:

```briev
defn pair() -> (Int, String) {
    term (42, "hello");
};
```

### Bracket Indexing

Tuples support the same `[index]` bracket syntax as Lists:

```briev
let t = (10, 20, 30);
let x = t[1];   // 20
let y = t[0];   // 10
```

Indices are zero-based and bounds-checked at runtime. Use bracket syntax (`pair[0]`) instead of the deprecated `:> 0` projection.

### Memory Layout

In the LLVM backend, Tuples share the same memory layout as Lists: `[data_ptr, len, elem0, elem1, ...]`. This means all existing GEP-based indexing code works for both List and Tuple without modification.

## Syntax Gates

Override to restrict access:

```briev
Type Stack<T> : List<T> { AllowIndex = false; };
Type Queue<T> : List<T> { ExtractFrom = 0; AllowIndex = false; };
```

These say: "Stack is like List but you can't index into it." The compiler synthesizes the correct memory operations based on the metadata.

### Codecs

Codecs define how literals are translated to bytes at compile time:

```briev
import { UTF8 } from "std/UTF8.bv";
Type String : List<U8> { Codec = UTF8; };
```

When you write `"Hello"`, the compiler calls `UTF8::encode("Hello")` during compilation and embeds the result directly in the binary.

### Refinement Constraints

```briev
Type PositiveInt : Int {
    [ > 0 ]
};
```

The implicit subject is `_` (the value itself). Constraints are validated against literals at compile time; runtime guards are synthesized for dynamic values.

### Metadata Queries (`:>`)

The projection operator `:>` extracts metadata from any value without mutation:

| Expression | Returns | Works on |
|---|---|---|
| `val .#Size` | `Int` — number of elements/bytes | List, Tuple, String, HashMap, HashSet |
| `val :> IsEmpty` | `Bool` — `true` if zero elements | List, Tuple, String, HashMap, HashSet |
| `val :> Type` | `Int` — type discriminant | Any value |
| `val :> Keys` | `List<K>` — all keys | HashMap |
| `val :> Values` | `List<V>` — all values | HashMap |
| `val :> Contains(k)` | `Bool` — key membership | HashMap, HashSet |
| `val :> Width` | `Int` — bit width of the type | Any scalar type |
| `val :> Endian` | `Int` — endianness (0=little, 1=big) | Any scalar type |
| `val :> Codec` | `Int` — encoding tag | String, Data |
| `val :> Ops` | `Int` — number of declared operators | Any type |

Tuple element access via `:> N` (integer index) is **deprecated** — use `val[N]` bracket syntax instead.

### Operator Declarations

Types declare their operations in the type definition body using `op` annotations.
These tell the compiler which LLVM instruction to emit for each operator:

```briev
type Int : Bits {
    bytes <~ 8;
    storage <~ "Boxed";
    llvm <~ "i64";
    op Add(Int) -> Int = "add nsw";     // nsw = no signed wrap
    op Sub(Int) -> Int = "sub nsw";
    op Mul(Int) -> Int = "mul nsw";
    op Div(Int) -> Int = "sdiv";
    op Mod(Int) -> Int = "srem";
    op Neg() -> Int = "neg";
    op Eq(Int) -> Bool = "icmp eq";
    op Ne(Int) -> Bool = "icmp ne";
    op Lt(Int) -> Bool = "icmp slt";
    op Le(Int) -> Bool = "icmp sle";
    op Gt(Int) -> Bool = "icmp sgt";
    op Ge(Int) -> Bool = "icmp sge";
    default_width <~ 64;               // Int → Int<64>
    commuting <~ true;                 // optimizer: order-independent
};
```

Supported annotations:

| Annotation | Purpose | Example |
|---|---|---|
| `bytes` | Physical size in bytes | `bytes <~ 8` |
| `alignment` | Memory alignment | `alignment <~ 8` |
| `llvm` | LLVM type string | `llvm <~ "i64"` |
| `storage` | `"Native"` (float/double) or `"Boxed"` (i64 register) | `storage <~ "Boxed"` |
| `tbaa` | TBAA type node for alias analysis | `tbaa <~ "Int"` |
| `box` | Boxing transform (Int → i64) | `box <~ "sext.i64.to.i8#"` |
| `unbox` | Unboxing transform (i64 → Int) | `unbox <~ "trunc.i64.to.i8#"` |
| `op Name(T) -> R = "llvm_inst"` | Operator → LLVM instruction mapping | `op Add(Int) -> Int = "add nsw"` |
| `default_width` | Width when none specified | `default_width <~ 64` |
| `commuting` | Optimization hint: operand order irrelevant | `commuting <~ true` |
| `constant_time` | Optimization hint: runtime independent of value | `constant_time <~ true` |

### Type Resolution Pipeline

When you write `let x: Int = 0`, the compiler resolves the type through three stages:

1. **Parser** produces `Type::Custom("Int")` — a named reference
2. **NormalizeTypes pass** looks up `"Int"` in the TypeUniverse, finds `default_width <~ 64`, and produces `Type::Applied("Int", [Width(64)])`
3. **Codegen** queries the universe for `Int`'s storage (`"Boxed"` → `i64`) and operator declarations (`"add nsw"` for Add)

```briev
// All of these produce equivalent code:
let a: Int = 0;          // Custom("Int") → Applied("Int", [Width(64)]) → Bits(64)
let b: Int<64> = 0;      // Applied("Int", [Width(64)]) → Bits(64)
let c: Int<8> = 0;       // Applied("Int", [Width(8)]) → Bits(8) with Int's ops
```

The same pipeline applies to all named types: `Float`, `Bool`, `String`, `Data`,
and user-defined types all resolve through the universe.

### String Layout

`String` is a struct with three fields, defined in the TypeUniverse:

```briev
type String : Bits {
    struct ptr: Ptr<Byte>;      // pointer to UTF-8 data
    struct len: Int;            // byte length
    struct codec: Int;          // encoding tag (0 = UTF-8)
};
```

This means `String` occupies 24 bytes (pointer + length + codec) and supports
field projection via `s :> ptr`, `s :> len`, `s :> codec`. String literals like
`"hello"` are desugared at compile time:

```briev
// Before NormalizeTypes:
let s: String = "hello";

// After NormalizeTypes:
let s: String = String { ptr: &"hello", len: 5, codec: 0 };
```

### InsertAt / ExtractFrom

These two properties define where elements go when pushing and where they come from when popping:

| Expression | Example use |
|---|---|
| `0` | Front (Queue pop) |
| `.#Size` | Append (push to end) |
| `.#Size - 1` | Last (Stack pop) |
| `: { MAX(.k) }` | Max-heap ordered |
| `: { MIN(.k) }` | Min-heap ordered |

#### Custom strategy dispatch

When `InsertAt` or `ExtractFrom` is set to a name that doesn't match any
built-in strategy string, the compiler treats it as a reference to a
user-defined function (typically an `inop` or `defn`). The `<-` arrow
operator dispatches to that function instead of using the default behavior:

```briev
type SkipList<T> : List<T> {
    InsertAt = sl_insert;     // sl <- val → sl_insert#(sl, val)
    ExtractFrom = sl_remove;  // dest <- sl → sl_remove#(sl)
};
```

The function is resolved first as an `inop` (uses fallback for interpreter,
BILD for LLVM), then as a `defn` (executes body). The interpreter tracks
declared types via `let_types` so that `let sl: SkipList<Int> = []` correctly
maps `sl` to `SkipList<Int>` for strategy resolution.

Built-in strategy names:
- `InsertAt`: `append`, `prepend`, `sorted`, `hash`
- `ExtractFrom`: `pop`, `shift`, `head`, `tail`, `hash`

Any other string becomes `Custom(fn_name)`.

### How it works (Three-Pass Pipeline)

1. **Pass 1 (Type-Universe)**: The compiler collects all `Type` declarations, resolves derivation chains, inherits properties, and freezes the type map.
2. **Pass 2 (NormalizeTypes)**: Resolves `Custom("Int")` → `Applied("Int", [Width(64)])` → `Bits(64)` using universe defaults. Desugars string literals to struct instances.
3. **Pass 3 (Executable)**: Uses the frozen type map for type checking, literal encoding, and code generation.

### The Briev philosophy

Most languages hardcode type rules inside the compiler's Rust/C++ source. Briev hardcodes about 13 properties in the Rust compiler and declares the rest in `lib/std/types/bootstrap.bv` (~300 lines of type definitions with operator annotations). Everything — `String`, `Stack`, `Queue`, `HashMap`, even `Int` — is defined in Briev source files, using the same syntax you use to define your own types.

### The `&` address-of operator (2026-07-09)

The `&` operator creates a typed pointer (reference) to a variable or state field:

```briev
let x: Int = 42;
let p = &x;        // p: PtrConst<Int>, points to x
&x = 99;           // write through pointer to state field
```

**Const vs mutable inference:**

| Source | Result type | Description |
|--------|-------------|-------------|
| State field `&field` | `Ptr<T>` | Mutable — can write through it |
| `let` binding `&let_binding` | `PtrConst<T>` | Read-only — cannot modify |
| `&param` | `PtrConst<T>` | Read-only — function parameters |

The `*` operator dereferences a pointer:

```briev
let x: Int = 42;
let p = &x;
let v = *p;       // v = 42 (reads through the reference)
```

**Deref of `Ptr<T>`** produces `T`. Deref of `PtrConst<T>` also produces `T`. Any
attempt to dereference a non-pointer type is a compile-time type error.

**`&` in assignments** (LHS) is sugar for "store through this address":

```briev
&field = value;   // writes value to state field 'field'
*ptr = value;     // writes value through pointer
```

These produce the same LLVM IR: `getelementptr` + `store`.

**Dangling detection:** The compiler warns if you store a pointer to a local
variable into a state field:

```briev
node example [true][true] {
    let tmp: Int = compute();
    &state_field = &tmp;  // warning: pointer to local may dangle
    term;
};
```

Store the value instead:

```briev
&state_field = tmp;       // OK: copies the value, not the pointer
```

### Volatile pointers (2026-07-03)

For hardware register access, Briev has four forms of explicit pointer type,
all sharing the same machine representation (`i64`/`u64` in the backend):

| Form | Example | What it says |
|------|---------|--------------|
| Typed | `Ptr<Int>` | Points to an Int (full nominal type) |
| Bare | `Ptr` | Points to 8 bytes (safe void\*) |
| Fixed | `Ptr32` / `Ptr64` / `Ptr128` | Points to N bytes (known layout) |
| Bits | `Ptr<Bits @/0..63>` | Points to exact bit range |

These pointers **cannot be dereferenced directly** — use `VolatileLoad#` / `VolatileStore#`:

```briev
let reg: Ptr<Int> = 0x40011000 as Ptr<Int>;
let val = VolatileLoad#(reg);
VolatileStore#(reg, val + 1);
```

Layout-compatible casts between pointer types are allowed when the pointee
types have the same bytes and compatible alignment:

```briev
let f: Ptr<Float> = 0x4000 as Ptr<Float>;
let i: Ptr<Int32> = f as Ptr<Int32>;  // both 4 bytes, align 4
```

Spatial operations use `lib/std/spatial.bv`:

```briev
import { block_copy } from "std/spatial.bv";
let dst: Ptr64 = malloc(8);
block_copy(dst, src, 8);
```

Function pointers via `.#Ptr`:

```briev
defn cmp(a: Int, b: Int) -> Bool { term a == b; };
let fn_ptr = cmp .#Ptr;
let eq = fn_ptr(3, 5);
```

No `reinterpret_cast` exists. If the compiler can't prove layout compatibility,
use explicit byte-level operations or a `meld` declaration.

### Two kinds of `as`

Every cast is either a **representation** (the bits stay, the reading changes) or
a **conversion** (a different value entirely). Bit-preserving casts all route
through `Bits`, which is the one edge every type has:

| Cast | Kind | What it means |
|---|---|---|
| `text as Data`, `data as Int` | representation | the handle's **address**, as a word (no call emitted) |
| `bits as Int`, `n as Bit` | representation | the same bits |
| `num as String` | conversion | formats — `1` becomes the text `"1"` (`call @int_to_str`) |
| `text as Int` | conversion | parses the text (`call @str_to_int`) |
| `f as Int` | conversion | a numeric conversion, never the float's bits |

The rule that matters: **the pair decides, not the direction**, and the lowering
shows which it is — a representation emits no call. That is why the stdlib's byte
accessors spell `(s as Data) as Int`: it is the address, so `+ 8 + i` is
meaningful. A cast never reads memory; bytes come from `Load#` / `Store#` (or
`VolatileLoad#` / `VolatileStore#` for `Ptr`). Normative statement: **§15.6** of
the specification.
