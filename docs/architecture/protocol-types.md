# Protocol Types — Operation-First Compilation

**Date:** 2026-07-23
**Status:** Architecture documentation

---

## The Core Idea

A protocol category (written as `String`, `Int`, `Float`, `Bits`) is not
a type. It's a **compile-time operational assumption** — a promise that "I
support the operations of this category."

**A type has no fixed layout.** It has whatever shape the optimizer selects
for the program's actual usage. The protocol contract guarantees *behavior*,
not *bytes*.

```briev
type String: String;   // "I can concatenate, compare, slice"
                        // layout: whatever the optimizer picks
```

## Protocols Are a Frontend-Only Abstraction

The compiler resolves all protocol assumptions to concrete types and operations
before LLVM ever sees the IR. LLVM never knows about `String`, `CastTo`,
or protocol variants.

```
Source:  String, op Add, op Length
             │
             ▼
    Protocol Graph ─── BFS CastTo/CastFrom edges
    (frontend only)    (bindings = transformation functions)
             │
             ▼
    Concrete types + ops chosen per target
    (backend: struct { ptr, i64 }, native add)
             │
             ▼
    LLVM IR: concrete types, concrete ops, never knows protocols
```

## Protocol Declarations: `proto`

A protocol variant is declared with `proto` — a new top-level form:

```briev
proto ASCII: String {
    CastTo(String<UTF8>) = ASCII_to_UTF8(#L);
    CastFrom(String<UTF8>) = UTF8_to_ASCII(#L);
};
```

### Rules

| Item | Requirement | Why |
|---|---|---|
| `CastTo`/`CastFrom` | MUST have a binding `= fn(#L)` | The binding defines HOW the layouts differ |
| Round-trip parity | Compiler proves `inverse(forward(x)) == x` | Inconsistent transforms are bugs |
| Cross-op equivalence | Proved equivalent to CastTo→default→CastFrom | Custom path must match round-trip |
| `#L`, `#R` | `#L` = self, `#R` = target | Convention, enforced by type checker |

The body is **difference-only** — you write only what differs from the default.

### Defaults Are Primordial

`String` resolves to `String<UTF8>` by default. The defaults are hardcoded
in the parser and always available, even with `--no-stdlib`. Non-default
variants (ASCII, UTF16, Posit32) are provided by the prelude plugin.

| Bare hashword | Resolves to | Source |
|:---|---:|---:|
| `String` | `String<UTF8>` | Primordial (parser) |
| `Float` | `Float<IEEE754>` | Primordial (parser) |
| `Char` | `Char<unicode>` | Primordial (parser) |
| `Int` | `Int` (no variant) | Primordial |

### Three-Layer Model

| Layer | Declares | Example | Purpose |
|---|---|---|---|
| Protocol edge | Compatibility direction + transform | `CastTo(String<UTF8>) = fn(#L);` | Defines HOW two variants relate |
| Protocol op | Optimization hint | `op Add(String<UTF8>) = fn(#L, #R);` | Skip CastTo→default→CastFrom round-trip |
| Type override | Different method | `op CastTo(String<UTF8>) = my_way(#L);` | Override protocol-level default |

### Prelude Declarations

Non-default variants are declared in `lib/std/protocols.bv`, auto-loaded by
the prelude:

```briev
proto ASCII: String {
    CastTo(String<UTF8>) = ASCII_to_UTF8(#L);
    CastFrom(String<UTF8>) = UTF8_to_ASCII(#L);
};
proto UTF16: String {
    CastTo(String<UTF8>) = UTF16_to_UTF8(#L);
    CastFrom(String<UTF8>) = UTF8_to_UTF16(#L);
};
```

## Protocol Graph

Every `CastTo`/`CastFrom` with a binding is a directed edge. The binding
IS the transformation. The compiler finds the shortest path from source
to target at compile time via BFS.

### Root: `Bits`

`Cast(Bits)` is implicit on every type. Because `Bits` is the implicit
base of all types, every type can reinterpret itself as raw bytes. This
guarantees the protocol graph is always connected.

```
SourceType --[implicit Cast(Bits)]--> Bits --[CastFrom(TargetType)]--> TargetType
```

### Fewer Hops = Less Conversion

Declaring a variant closer to the target means fewer BFS hops:
- `String<UTF16>` on Windows → identity path, zero conversion code
- `String` → resolves through `UTF8 → UTF16` → one conversion edge
- Both compile correctly; the pinned variant gives the optimizer less work

## Inheritance

When a type declares `type MyString: String`, it automatically inherits
all edges from the `String` category's protocol graph. A type only writes
`op CastTo(...) = fn(...)` to **override** the protocol-level default.

```briev
type MyString: String;   // inherits CastTo(String<UTF8>), CastFrom, all ops

type MySpecialString: String {
    op CastTo(String<UTF8>) = my_custom_way(#L);  // override
};
```

## Compiler Proofs

For every protocol declaration, the compiler runs two proofs:

### Proof 1: Round-Trip Identity

```briev
CastTo(String<UTF8>) = ASCII_to_UTF8(#L);
CastFrom(String<UTF8>) = UTF8_to_ASCII(#L);
// Proved: UTF8_to_ASCII(ASCII_to_UTF8(x)) == x
```

This uses the existing symbolic evaluation and SMT solver pipeline
(`src/analysis/meld_validation.rs`). If the proof fails, compilation is
denied — inconsistent transformations are bugs.

### Proof 2: Cross-Op Equivalence

```briev
op Add(String<UTF8>) = ASCII_add_with_UTF8(#L, #R);
// Proved: ASCII_add_with_UTF8(x, y) == UTF8_to_ASCII(ASCII_to_UTF8(x) + y)
```

The cross-op is an optimization hint — it says "skip the round-trip, I
already know how to do this directly." The compiler proves the hint is
correct.

### Proof 3: Protocol Contract (Optional)

If a protocol declares a contract `[expr]`, the compiler proves it at
every boundary crossing via SMT:

```briev
proto ASCII: String [#Self[i] < 128] {
    CastTo(String<UTF8>) = ASCII_to_UTF8(#L);
};
// Every value entering or exiting this protocol must satisfy the contract
```

## `#L` and `#R` Convention

In protocol declarations, `#L` is always the protocol's own variant (self)
and `#R` is the target variant parameter:

```briev
// #L = self (ASCII), #R = target (UTF8)
CastTo(String<UTF8>) = ASCII_to_UTF8(#L);
op Add(String<UTF8>) = ASCII_add_with_UTF8(#L, #R);
```

This follows the same `#L`/`#R` convention used in type-level `op` bindings,
where Add maps to `+` and `#L + #R` is natural.

## Protocol Categories Are Fictional, Not Abstract

A protocol category has **no implementation**. There is no "string struct"
for `String`. It's a hypothetical — "this is what a thing WOULD look like
if it were a string, but I don't care what it needs to be."

The compiler uses the protocol graph to discover CastTo/CastFrom relationships.
The graph edges are the edges from `proto` declarations AND from type-level
`op CastTo`/`op CastFrom` declarations. Both feed the same BFS.
