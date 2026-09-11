# Intrinsics vs Stdlib — The Dividing Line

> **2026-07-20:** The TOML config layer described in this document
> (`config/llvm-ops.toml`, `config/ctd-llvm-mappings.toml`) is removed.
> Hashword categories (`Int`, `Float`, `String`) in op signatures are the
> replacement — the backend has intrinsic knowledge of `#Category` operations.
> Custom types use explicit `op Add(MyType) = fn(#L, #R)` bindings with
> auto-`alwaysinline`. See `docs/architecture/casting-protocol.md`.

## The Rule

**Everything that MUST hold with no stdlib loaded is an intrinsic.**
**Everything else is stdlib.**

If `rm -rf lib/std && compiler --no-stdlib` still type-checks and compiles
`let x: Int = 5`, it's an intrinsic. If a user could write a `.bv` file that
achieves the same thing, it belongs in stdlib.

## Observability: Two Enforcement Points, One Concept

Intrinsics and stdlib both need "the compiler must not eliminate this call."
Intrinsics are pinned **by design** (`observable: true` in
`intrinsic_signatures.rs`); stdlib functions claim the same contract with the
**`out` keyword** (2026-08-04, `docs/plans/2026-08-04-out-observability-and-
native-stdlib.md`):

| Where | Mechanism | Semantics |
|---|---|---|
| Intrinsics | `observable: true` in `intrinsic_signatures.rs` | Pinned by design — an intrinsic IS its behavior (`Print#`, `Malloc#`, `Copy#` …) |
| Stdlib (`defn`/`node`/`txn`/`let`) | `out` keyword | Calls are liveness roots; the body is still fully optimized; only the call boundary survives. `out let` pins reads/writes without volatile. |

`out` is a **pin, never an acceleration**: the compiler always maximizes
optimization (even to a LUT); `out` says "I need this specific call done, you
cannot optimize it out." This is the never-faster contract (AGENTS.md Golden
Rule 2 "MAXIMUM EFFICIENT DEFAULT": keywords express intent, never speed).

## Three-Layer Architecture

| Layer | File | Role | Validated |
|-------|------|------|-----------|
| **Contract** | `src/intrinsic_signatures.rs` | Declares what `#` intrinsics exist and their type signatures | Frontend (typechecker) rejects calls to unknown intrinsics |
| **Implementation** | `config/llvm-ops.toml` | Maps (op, primitive, bytes) → LLVM IR template | Backend finds template or falls through to `emit_external_call` |
| **Binding** | `lib/std/types/bootstrap.bv` | Maps operator symbols (`+`, `<`) to per-type op bindings | Typechecker resolves operator via `get_operator_intrinsic` |

The frontend can optionally cross-check the config to give early errors when
the backend has no template for a given type+width combination. Currently this
check does not exist — the backend silently falls through to external call.

## What Lives Where

### Intrinsics (`intrinsic_signatures.rs` — compiler must have)

| Category | Examples | Reason |
|----------|----------|--------|
| Primitive types | `Int`, `Float`, `Bool`, `Void` | Compiler needs these to type-check anything |
| Arithmetic | `Add#`, `Sub#`, `Mul#`, `Div#`, `Rem#`, `Neg#`, `Abs#` | Operator sugar desugars to these; no stdlib needed. 2026-09-02 (fundamental-parent-membership): these are SHAPE-INFERRED — one intrinsic per operation serves every width; the operand's `(category, bits)` picks the lowering (`fadd half` / `fadd float` / `add i64`). No width-suffixed arithmetic intrinsics exist or may be added (FAddF64#/AddI64#-style protocol-table labels are typecheck-era names, never consumed by codegen). |
| Comparison | `Eq#`, `Neq#`, `Lt#`, `Gt#`, `Le#`, `Ge#` | Same — operators desugar to these |
| Bitwise | `BitAnd#`, `BitOr#`, `BitXor#`, `Shl#`, `Shr#`, `BitNot#` | Same |
| Logical | `Not#` | Unary, no short-circuit concern |
| Pointers | `Deref#`, `AddrOf#` | `*ptr` and `&x` desugar to these; needed for memory model |
| Index | `Index#` | `a[b]` desugars to this |
| Memory | `Malloc#`, `Free#`, `Load#`, `Store#`, `Copy#`, `Fill#` | Heap operations are language primitives |
| Math hardware | `Sqrt#`, `Sin#`, `Cos#`, `Fabs#`, `Ceil#`, `Floor#`, `Exp#`, `Pow#` | Cannot be expressed in Briev without hardware access. `Exp#` (2026-09-02): `GLSL.std.450 Exp` on SPIR-V, `expf` on native. |
| I/O | — | Migrated to stdlib. `!Print`/`!PrintLn` dispatched via Front plugin; `!GetEnv`/`!GetEnvInt` resolved to pure-Briev environ scan. All use `SysCall#(Write, ...)` or `Load#` underneath. |
| Atomic | `AtomicLoad#`, `AtomicStore#`, `AtomicCas#`, etc. | Hardware memory model primitives |

### Stdlib (`lib/std/` — can be absent, feels native)

| Category | Examples | How it hooks in |
|----------|----------|-----------------|
| Collection types | `RingBuffer<T>`, `List<T>`, `HashMap<K,V>` | `InsertAt <~ fn(#L, #R)` property |
| Strategy functions | `ring_push`, `ring_pop` | Briev `defn` with Ptr arithmetic |
| Derived collections | `Stack<T>`, `Queue<T>` | Wraps RingBuffer/List |
| Terminal I/O | `print_int`, `print_str`, `print_char`, `print_float` | `!Print`/`!PrintLn` dispatched by Front plugin → typed stdlib functions using `SysCall#(Write, ...)` |
| Environment | `getenv`, `get_env_int` | `!GetEnv`/`!GetEnvInt` dispatched by Front plugin → pure-Briev environ scan via `Load#` |
| File I/O | `FileRead#`, `FileWrite#` | Already intrinsics (observable) |
| Threading | | Uses atomic intrinsics |

## The Test

```
// This must work with --no-stdlib:
let x: Int = 5;
let y: Int = x + 3;
let p: Ptr<Int> = &x;
let v: Int = *p;
```

```
// This is stdlib — compiler doesn't know about it:
let rb: RingBuffer<Int> = init_ring_buf(1024);
rb <- 42;              // Works because InsertAt property resolves
let v: Int = rb.pop();  // ExtractFrom resolves via the arrow (dest <- rb)
```

## Why Not Make Everything Stdlib?

Some operations cannot be expressed in Briev at all:
- `a + b` on hardware integers maps to a single `add` instruction — no Briev
  function can emit that without compiler help.
- `Sqrt#(x)` maps to `@llvm.sqrt.f32` — no Briev function can call LLVM
  intrinsics directly.
- `Malloc#(size)` maps to `@malloc` — the ABI convention for heap allocation
  is platform-specific and baked into the compiler.

These must be intrinsics. Everything else is negotiable.

## Why Make Everything Else Stdlib?

The `ring_push` case proves the pattern: 15 lines of Briev with Ptr arithmetic
replaces a compiler-intrinsic + Rust match arm. A user writing `MyQueue<T>`
with `InsertAt <~ my_push(#L, #R)` gets the same `<-` syntax without touching
the compiler. The only hardcoded Rust code is the strategy dispatch reading the
property value — and even that is generic.
