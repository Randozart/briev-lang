# Hash-Prefixed Compiler Words (`#words`)

Compiler-internal tokens prefixed with `#` that carry special meaning.
They are lexed as distinct tokens, never as identifiers.

## 2026-09-11 (FUNDAMENTALS DOCTRINE — category hashwords retired)

The fundamentals stand alone: **the fundamental name is both the base
type AND the protocol.** One name, two roles.

- `Float` is a type, a parent (`type Float32: Float`), a protocol
  category (`proto Posit32: Float`), and an op parameter
  (`op Add(Float)`). All one spelling.
- Protocol variants ride on the type: `Float<Posit>`, `String<UTF8>` —
  parsed as `Applied(fundamental, [variant])` and peeled by the casting
  graph to `(category, variant)`.
- The category hashwords — `#Float`, `#Int`, `#UInt`, `#String`,
  `#Bool`, `#Char`, `#Blob`, `#Bit`, `#Data`, `#Bits` — are RETIRED.
  Any of them in a type, parent, protocol-category, op-parameter, or
  cast-target position is a compile ERROR naming the fix
  ("#Float is retired — write Float"). No deprecation alias. Their
  `Type::HashWord` / `Type::HashWordVariant` AST variants are deleted.
- Target/protocol hashwords are a DIFFERENT mechanism and remain:
  `#System` (link), `#Web`/`#Web`-style bridge routing, `#Link<name>`
  (linker flags). They never named types or categories.
- Operand markers remain: `#Lh` / `#Rh` / `#T` / `#Self` in op bindings.
- Field markers remain: `#Stack` / `#Heap` / `#Scalar` on declarations.

Consumers removed in the same sweep: the parser hashword arms and
default-variant tables, `type_to_protocol` HashWord peeling (replaced
by the Applied-fundamental peel), op-coverage shape-matching
(`param_covers` now treats a bare fundamental param as its category),
`validate_constraints`, GLUE ABI keys (`.dbv` protocol/conversion keys
and the `native_key` lookups are bare), `derive_type_protocols`
(emits bare categories), and the interpreter/bit-lane `#Bit`/`#Bits`
special cases (flexible `Bit`/`Bits` is the content-view cast target,
routed through the casting graph's Data root; concrete `Bits<N>` is the
width assertion).

Historical sections below are preserved verbatim.

---

## 2026-07-20: Hashword Categories (`#Int`, `#Float`, `#String`, etc.)

> **2026-08-15 (Fundamentals as Types).** The fundamental types
> (`Data`, `Bit<N>`, `Int`, `UInt`, `Float`, `String`, `Bool`, `Char`,
> `Blob`, `Ptr`, `Void`) are now compiler-native primordials and appear in
> op signatures **without** the `#` — `op Add(Int)`, not `op Add(#Int)`.
> `Data` is the universal reflective floor (observable raw storage — not a
> supertype); `Bit<N>` is the unified bit type (`Bit` bare = flexible
> width); `Blob` is the `[len][bytes]` byte buffer.
> Parameterized protocol **variants** (`#String<UTF8>`, `#Float<IEEE754>`)
> keep their `#`. Non-category `#` roles below (`#Lh`/`#Rh`/`#T`, `#Link`,
> `#System`) are unchanged. See
> `docs/plans/2026-08-15-fundamentals-as-types.md`.

> **2026-09-02 (Fundamental-Parent Membership).** A type whose parent chain
> reaches a fundamental DERIVES that category's membership — `type Float16 :
> Float { }` is a `#Float` member with no `#Float` restatement, no declared
> arithmetic ops, and no width-suffixed intrinsics. Category resolution
> walks the parent chain at every hop (typechecker `declared_category_of`,
> casting graph `type_to_protocol`, the AST-level
> `derive_type_protocols` for glue/FFI), and all scalar lowering is
> shape-driven: the operand's `(category, bits)` picks half/float/double
> and the instruction (`fadd half`, OpFAdd 16, …). Explicit `#Cat`
> declarations keep precedence for variants (`#String<C_String>`) and
> non-fundamental protocols. See
> `docs/plans/2026-09-02-fundamental-parent-membership.md`.

The fundamental types in op signatures are **backend category directives**:

```briev
type Int : Bits {
    op Add(Int);       // "backend, emit whatever i64 add means to you"
    op Sub(Int);
    op Mul(Int);
};

type Bfloat16 { data: Bit<16>;
    op Add(Float) = bfloat_add(#Lh, #Rh);  // override with custom fn
};
```

A fundamental in an op signature tells the backend: **handle this operation
using your intrinsic knowledge of that category's protocol.** The backend
decides what `Int` addition means in its own terms (LLVM → `add i64`,
CIRCT → hardware adder, SPIR-V → `OpIAdd`).

**Protocol variants** parameterize the fundamentals: `#String<UTF8>`,
`#String<ASCII>`, `#Float<IEEE754>`. The file extension determines the
default (`.bv` → UTF8, `.ebv` → ASCII). Cross-variant calls require explicit
protocol disambiguation — the compiler errors if a `.bv` file calls an
`.ebv` function using `String` without specifying the variant:

```briev
fn cross(a: #String<UTF8>, b: #String<ASCII>) { ... };
```

Each backend declares supported protocols in `config/targets.dbvl`. A function
requiring a protocol the backend doesn't support produces a compile error.

### `Cast#()` — Cast intrinsick

`Cast#()` is a compiler internal intrinsick — not user-declarable. When the
programmer writes `(TargetType)expr`, the compiler emits `Cast#(expr, TargetType)`,
which runs the resolution pipeline:

1. `meld Source <-> Target` — structural equivalence
2. `op Cast(Target)` on Source — direct type-to-type
3. `CastTo(Fundamental)` → `CastFrom(Fundamental)` — parent/protocol path
4. Implicit `CastTo(Bit<N>)` + `CastFrom(Bit<N>)` — raw bytes (always)

User-declarable ops: `CastTo(Fundamental)`, `CastFrom(Fundamental)`, and
`Cast(ConcreteType)`. See `docs/architecture/casting-protocol.md`.

See `docs/architecture/casting-protocol.md` for the full protocol system.

### `op Parse(Fundamental)` — Compile-time identity parse

`op Parse(Fundamental)` uses the same mechanism as `op Add(Int)`:

| Fundamental | Tells the compiler |
|---|---|
| `Int` | Parse literals as native integer — no conversion needed |
| `String` | Parse quoted literals as UTF-8 — no conversion needed |
| `Float` | Parse numeric literals as IEEE 754 float |

`op Parse(Bare)`, `op Parse(Decimal)`, and `op Parse(Quoted)` are NOT
fundamental ops — they are concrete-form ops that always require a conversion
function. Only `op Parse(Fundamental)` is an identity parse.

```briev
type Int {
    op Add(Int);              // backend directive: integer add
    op Parse(Int);            // identity parse: literal IS an Int
    op Parse(Decimal);        // concrete form: numeric literal → conversion fn
};
```

## Current Words

| Token | Meaning | Used In |
|-------|---------|---------|
| `#Lh` | Left operand of `<-` | Strategy op bindings: `op InsertAt: fn(#Lh, #Rh)` |
| `#Rh` | Right operand of `<-` | Strategy op bindings: `op ExtractFrom: fn(#Rh)` |
| `#T` | Type parameter of generic collection | Strategy op bindings: `pop as #T` |
| `#StdIn` / `#StdOut` / `#StdErr` | Stream symbols (Phase 4) | `#StdOut <- value` writes (→ `Print#`); `#StdErr <- <String>` writes to stderr (→ `__eprint_str`); `#StdIn` is a `Ptr<Int>` stream-handle value |

> **2026-09-06 (plan 2026-09-06-cpp-expressiveness.md).** The atomic
> ordering vocabulary (`relaxed`, `acquire`, `release`, `bartered`, `seq`)
> is deliberately NOT a hashword set: the words are CONTEXT-SENSITIVE
> strategy keywords — bare lowercase, valid only (a) before `atomic` in a
> field declaration (`relaxed atomic count: Int;`) or (b) as the trailing
> argument of an atomic intrinsic (`AtomicLoad#(p, relaxed)`). No `#`
> marker: they are disclosed strategy words per SPEC §8.1, not
> compiler-internal tokens. `seq` is the existing strategy keyword reused
> (sequential consistency IS sequentialism); `bartered` names the
> acquire+release RMW exchange (each side gives visibility AND takes it).
> Outside the two positions they parse as expressions and surface as
> unknown-identifier errors.

## Semantics

### In strategy property bindings (`<~`)

Resolved at codegen time by substituting the concrete operand:

| Marker | `collection <- value` (InsertAt) | `dest <- collection` (ExtractFrom) | `<- collection` (Discard) |
|--------|----------------------------------|------------------------------------|---------------------------|
| `#Lh` | handle register for `collection` | pop target register for `dest` | void (no target) |
| `#Rh` | value register for `value` | handle register for `collection` | handle register for `collection` |
| `#T` | element type of collection | element type of collection | element type of collection |

The handle register is computed via `emit_addr_of` on the collection variable,
which produces a GEP into `%State` (for state fields) or an alloca address
(for let bindings). The `#Rh`/`#Lh` substitution is a register name pass-through
— the compiler resolves the expression to a register first, then substitutes.

### Stream symbols (`#StdIn`/`#StdOut`/`#StdErr`, Phase 4)

These are compiler-known intrinsic-pointer symbols, not protocol hashwords.
They lex as ordinary identifiers (`#` is an identifier character) and are
recognized by name in the arrow typechecker and codegen:

- `#StdOut <- value` — any value; lowered to the generic `Print#` intrinsic.
- `#StdErr <- <String>` — a String only; lowered to `__eprint_str` (stderr).
- `#StdIn` — usable as a value (`let h: Ptr<Int> = #StdIn;`); a stream handle
  for the trg read composition.

### Rule

No `#`-prefixed word is ever a user-defined identifier. They are reserved
compiler vocabulary. Adding a new `#word` requires:
1. A new token in `src/lexer.rs`
2. Parser handling in the relevant context
3. Codegen resolution in the backend
4. Entry in this document

### Reserved

- `#Self` — reserved for future use (self-reference to the type definition)

## Non-word Hash Tokens

These `#` tokens exist but are NOT compiler words — they are syntax:

| Token | Purpose |
|-------|---------|
| `#` suffix on identifiers | Intrinsic marker: `Malloc#`, `SysCall#`, `Sqrt#` |
