# Protocol Model — Architecture Document
## 2026-07-27

## 1. Core Thesis: Operations, Not Layouts

Briev's type system is **protocol-based**, not layout-based. A type is defined by
what operations it supports, not by how many bytes it occupies on the target.

```briev
type Int: Int {
    op Add: add(#Lh, #Rh);
    op Sub: sub(#Lh, #Rh);
    op Mul: mul(#Lh, #Rh);
    ...
};
```

A value of type `Int` is anything that implements the `Int` protocol. The physical
representation (8 bits, 32 bits, 64 bits) is a **property** of the type that the
compiler resolves during codegen, not the defining characteristic.

This separation of protocol from layout is the central architectural insight:
two types can share the `Int` protocol while having different bit widths,
different LLVM types, or different encodings, as long as their arithmetic
behavior matches the protocol contract.

### 1.1 The Implicit Golden Path: The Three Tiers

Every value in Briev can be viewed through three lenses. The compiler resolves
all three at compile time — no runtime overhead:

| Lens | Protocol | What you get | Example: `"1"` |
|------|----------|-------------|----------------|
| **Address** | `#Ptr` | The spatial memory location of the data | `0x7ffee3b...` |
| **Encoding** | `Bit` | The raw physical bit pattern | `0x31` (ASCII `'1'`) |
| **Value** | `Int` | The logical mathematical interpretation | `0x01` (integer 1) |

This is a principled resolution of the classic **value vs. address** ambiguity
that plagues low-level languages. In C, a character literal like `'1'` evaluates
to the integer `49` (ASCII encoding), conflating `Bit` and `Int`. To get the
logical value `1`, the programmer must write `'1' - '0'` — a manual encoding hack.

In Briev, the protocol cast selects the interpretation:

```briev
let addr: Ptr  = (Ptr) "1";      // → address in .rodata
let byte: Bit  = (Bit) "1";      // → 0x31 (raw ASCII byte)
let value: Int = (Int) "1";      // → 1 (logical integer)
```

Each cast is resolved entirely at compile time. The generated code contains the
final constant — no runtime conversion overhead.

## 2. Protocol Hierarchy

### 2.1 Core Protocols

| Protocol | Category | Operations | Base variant |
|----------|----------|-----------|--------------|
| `Bit` | Atomic | `CastTo(Int)`, `CastFrom(Int)` | N/A (atomic) |
| `Int` | Arithmetic | `Add`, `Sub`, `Mul`, `Div`, `Mod`, `Neg`, `Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge` | N/A |
| `UInt` | Arithmetic | Inherits `Int` + unsigned semantics | N/A |
| `Float` | Arithmetic | `Add`, `Sub`, `Mul`, `Div`, `Neg`, `Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge` | `Float<IEEE754>` |
| `Bool` | Logical | `Eq`, `Ne`, `And`, `Or`, `Not` | N/A |
| `Char` | Character | `Parse(Quoted)` | `Char<UTF32>` |
| `String` | Container | `Parse(Quoted)`, `prop Size`, `prop Bytes` | `String<UTF8>` |
| `#Void` | Empty | None | N/A |

### 2.2 Protocol Inheritance

A type declares protocol membership via the colon syntax:

```briev
type Int: Int { ... };
type UInt: Int { ... };         // UInt inherits Int
type Int8: Int { bits <~ 8; };  // Int8 inherits Int with narrowed width
```

`protocol` is a category hashword (`Int`, `Float`, etc.). The normalizer
injects a `Cast.#<Category>` property for each protocol the type declares.

### 2.3 Protocol Variants

Protocols can have named variants for different encodings:

```briev
type String: String<UTF8> { encoding <~ "UTF-8"; ... };
type UTF16String: String<UTF16> { ... };
```

Cross-variant calls require explicit disambiguation. The compiler errors if a
`.bv` file calls a `.ebv` function using `String` without specifying the variant.

### 2.4 The Universal Base: `Bit`

Every type ultimately resolves upward to `Bit`. The BFS in `find_cast_path()`
unconditionally injects `Bit` as a reachable node (layout_optimizer.rs:275-301).
This provides the universal fallback: any type can be cast to `Bit` to access
its raw bit pattern, and back via `Bit` cast to any sufficiently wide type.

```briev
let raw: Bit = (Bit) myValue;       // always works
let back: Int = (Int) raw;          // works if Int is wide enough
```

This is the physical counterpart of the logical protocol system. Protocols
describe *what you can do with a value*. `Bit` describes *what the value is
made of*.

## 3. Protocol Casting

### 3.1 Implicit Casts

A type automatically participates in a protocol if it declares `type Foo: #Protocol`.
The normalizer injects `Cast.#Protocol` as a property:

```rust
// normalizer.rs line 306-311
if let Some(ref proto) = td.protocol {
    let cat = proto.strip_prefix('#').unwrap_or(proto).to_string();
    rt.properties.insert(format!("Cast.#{}", cat), PropertyValue::Bool(true));
}
```

No `CastTo`/`CastFrom` declaration is needed for the base protocol itself —
membership is enough.

### 3.2 Explicit Casts: `CastTo` / `CastFrom`

Cross-protocol conversion requires explicit operator declarations:

```briev
type CustomType: Int {
    op CastTo(String<UTF8>) = my_to_string(#Lh);
    op CastFrom(String<UTF8>) = my_from_string(#Lh);
};
```

These inject additional `Cast.String<UTF8>` properties on the type, creating
edges in the cast BFS graph.

### 3.3 Cast Resolution Pipeline

The `Cast#` intrinsic resolves casts in order (intrinsics.rs:963-1025):

```
Step 1: Direct type-to-type op Cast(Target) implementation
    ↓ if not found
Step 2: Protocol path via CastTo(#Cat) → CastFrom(#Cat) chain
    ↓ if not found  
Step 3: Meld shuffle metadata
    ↓ if not found
Step 4: Implicit Cast(Bit) — raw bitcast (always available)
```

### 3.4 The Cast BFS

`find_cast_path()` in layout_optimizer.rs walks the protocol graph to find
conversion chains:

Source: `source_type` → `Cast.#Cat1` → `Cast.#Cat2` → ... → `Cast.#Target`

Every path starts from the source type's `Cast.#` properties and walks through
shared protocol categories. `Bit` is the universal connector — if a protocol
path exists through shared categories, it's preferred. If not, the BFS falls
through to `Bit`.

## 4. `Bit` vs `Int` — The Key Distinction

| Aspect | `Bit` | `Int` |
|--------|--------|--------|
| Domain | Physical | Logical |
| Operations | `CastTo(Int)` | `Add`, `Sub`, `Mul`, ... |
| Width | Target-dependent | Constrained by `min_bits`/`max_bits` |
| Cast from literal | Raw encoding (e.g., `0x31`) | Parsed value (e.g., `1`) |
| Signedness | None | Signed (Int) or Unsigned (UInt) |

A `1` cast to `Bit` produces the machine word with value `1` in the *current*
bit width. The same `1` cast to `Int` produces the logical integer.

## 5. Operator Dispatch

Operations are dispatched by protocol membership, not by type name. The
compiler never matches on `t == "Int"` in Rust code — it checks
`is_protocol_member(ty, "Int")`.

```rust
// helpers.rs:1631-1641
fn is_protocol_member(&self, ty: &Type, protocol: &str) -> bool {
    let prop_key = format!("Cast.{}", protocol);
    self.ctx.type_universe.as_ref()
        .and_then(|u| ty.universe_key().and_then(|k| u.get(k)))
        .map(|rt| rt.properties.contains_key(&prop_key))
        .unwrap_or(false)
}
```

This ensures that any type implementing `Int` gets `+`, `-`, `*` operators
without needing explicit per-type match arms in the compiler.

### 5.1 Intrinsic Lowering

When an operator like `+` is applied to a type with `Int` protocol, the backend:

1. Checks `operator_defs` for the type's `op Add` binding
2. Falls back to the generic `add(#Lh, #Rh)` template from `config/llvm-ops.toml`
3. If no template exists for the type+width combination, errors at compile time

## 6. Protocol Variants and Backend Support

Each backend declares supported protocols in `config/targets.dbvl`. A function
requiring a protocol the backend doesn't support produces a compile error.

The file extension determines the default variant:
- `.bv` → `String<UTF8>`
- `.ebv` → `String<ASCII>`

Cross-variant calls require explicit protocol disambiguation at the call site.

## 7. Summary: Operations Drive Everything

```
type                declares           protocol
  │                                        │
  ▼                                        ▼
Normalizer injects                   Normalizer injects
properties for layout                 Cast.#Category property
(bytes, bits, fields)                       │
                                           ▼
                                     Casting graph resolves
                                     LLVM type + cast paths
                                           │
                                           ▼
                                     Backend dispatches ops
                                     via protocol membership
```

Layout is a property. Protocol is a capability. The type holds both, but the
compiler makes decisions based on capability, not physical size.
