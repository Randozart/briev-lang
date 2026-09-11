# Reflection: `.^` and `.^^`

Briev has two reflection operators that read **compiler-known metadata** about
a value or its type:

| Operator | Kind | Example | Result |
|----------|------|---------|--------|
| `expr.^Meta` | **Runtime** reflection — value-derived | `s.^Len` | a runtime value |
| `expr.^^Meta` | **Compile-time** reflection — type-derived | `x.^^Bytes` | a foldable constant |

The targets are **PascalCase compiler-known identifiers**, explicitly marked by
the operator (this is the language's no-hidden-magic rule: nothing
compiler-resolved is unmarked). Using a compile-time-only target after `.^`
(or a runtime-only target after `.^^`) is an error; an unknown target is an
error.

> The historical `:>` projection and `<:` derivation lens operators were
> removed with the hashword-protocol architecture. The `#` glyph remains for
> protocol hashwords (`op Add(Float)`) and intrinsic names (`Sqrt#`) — it is
> not a reflection operator.

## 1. Compile-time reflection (`.^^`)

Type-derived metadata — known at compile time, foldable into `const`
initializers and contract expressions:

| Target | Meaning |
|--------|---------|
| `Size` | fixed-size element count (`Int[8].^^Size` → 8) |
| `Bytes` | storage size of the type |
| `Alignment` | alignment of the type |
| `Type` | type identity token |

```briev
let items: Int[8];
let n: Int = items.^^Size;         // 8 — compile-time constant
let sz: Int = items.^^Bytes;       // 64 — 8 elements × 8 bytes
let al: Int = items.^^Alignment;   // alignment of Int[8]
```

Because `.^^` results fold, they are safe in contracts and precomputable code.

## 2. Runtime reflection (`.^`)

Value-derived metadata — a runtime value whose *type* is statically known:

| Target | Meaning |
|--------|---------|
| `Len` | runtime length of a String/List value |
| `Ptr` | address-of — `&x` is the primary spelling |

```briev
let s: String = "hello";
let n: Int = s.^Len;               // 5 — runtime length
let p: Ptr<Int> = &x;              // address-of (primary form)
let p2: Ptr<Int> = x.^Ptr;         // reflection form of &x
```

## 3. The static/runtime boundary

- `.^` is strictly runtime; `.^^` is strictly compile-time.
- A compile-time result is usable in `const` and contracts; a runtime result
  is not (it depends on the value).
- Runtime introspection beyond these targets uses **method calls**
  (`s.trim()`, `list.len()`) — never a reflection operator.

## 4. Bit intrinsics (removed from the operator surface)

The old `:> Popcount` / `:> Absolute` operator projections were removed with
the lens system. The LLVM bit intrinsics (`ctpop`, `ctlz`, `cttz`, `abs`,
`bitreverse`) are declared in the backend but have no operator form — they are
reachable as stdlib/`#` intrinsics. Collection queries (`FILTER`/`GROUP`) and
regex-DFA captures are planned, method-syntax features.
