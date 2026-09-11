# GLUE as ABI Generator — Architecture Insights

**Date:** 2026-07-22
**Status:** Architecture documentation

---

## The Core Insight: Briev Is Layout-Agnostic

A type like `String<UTF8>` does NOT specify a memory layout. It says:

> "I support the UTF-8 string protocol. I know how to decode bytes, measure
>  my length, concatenate with another UTF-8 string, index by code point.
>  What I *am* underneath — SSO inline, heap-allocated, interned, rope,
>  arena-backed — is my own business."

The compiler is free to choose any layout that satisfies the protocol, and
to change that choice per value based on liveness analysis, allocation
pressure, and usage patterns. This is not a future feature — it's baked
into the type system:

- `Int` doesn't say `i64` — it says "I am a machine integer."
- `Float` doesn't say `IEEE754` — it says "I am an IEEE 754 float."
- `Bits` doesn't say `{i8*}` — it says "I am raw memory."

The `Cast(Bits)` fallback exists precisely because any type CAN be treated
as raw memory at the boundary when no higher protocol path exists.

## Hashwords Are Not Types, They Are Protocols

A hashword `#Category` is a **backend directive and a protocol identifier**:
- Backend directive: "use your native implementation of this operation" (`op Add(Int)`)
- Protocol identifier: "I implement the operations of this category"

Two types that both implement `String<UTF8>` can be converted between each
other through the protocol path — even if their memory layouts are completely
different. The transform cost depends on the difference in layout:

| Layout | Transform to SSO | Transform to Heap |
|--------|------------------|-------------------|
| SSO `{i64, i64}` | Identity (0) | Extract + malloc |
| Heap `{ptr, len}` | Pack + inline | Identity (0) |
| Rope | Flatten to buffer | Flatten to buffer |
| Interned(ID) | Lookup + extract | Lookup + copy |

The shortest path (via BFS through the protocol graph) is chosen at compile
time. Identity costs zero and is eliminated at LTO.

## GLUE Is Not an FFI, It Is an ABI Generator

FFI is mechanical: "call this C function with these C types." It prescribes
a fixed ABI (usually C's) that both sides must agree on.

GLUE is generative:

1. **Discover** what protocol each type implements on each side of the
   boundary (via hashword declarations in `.bv` files).
2. **Find** the shortest path between them using `find_cast_path()` BFS.
3. **Emit** the transform chain (CastTo → CastFrom → meld → identity) at
   the boundary.
4. **Optimize away** the chain at LTO time when the path is identity.

For `calling_convention = "lto"` (Rust), there is NO FFI call. The bridge
`.ll` integrates with the host `.ll`, and LLVM inlines the entire boundary.
The "export wrapper" is just a `dso_local` symbol that LLVM resolves at
link time.

For `calling_convention = "c_abi"` (Python, Node), the transform chain is
emitted as explicit code in the C-compatible wrapper. The cost is paid
once per call.

## Layout Inference (Natural Next Step)

Since the type system is layout-agnostic, the compiler can choose layouts
based on how a value is used:

| Usage pattern | Optimal layout |
|---------------|----------------|
| Short-lived, read-only | SSO inline (zero alloc) |
| Frequently concatenated | Rope (O(1) append) |
| HashMap key | Interned (O(1) compare) |
| Crosses FFI boundary | Heap (C-compatible) |
| Large, rarely accessed | Arena-backed (cache-friendly) |

The layout optimizer (Phase 6) was the first step — it proposes layout
changes at the FFI boundary to minimize protocol transform costs. A full
layout inference pass would extend this to ALL values, not just boundary
crossings.

This is not implemented yet, but the foundation exists:
- Protocol system with hashword categories
- `find_cast_path()` BFS for computing transform costs between layouts
- Layout optimizer for boundary proposals
- GLUE bridge for emitting transforms at the boundary

## Key Takeaway

Briev types don't have layouts — they have **protocols**. Layouts are an
implementation detail the compiler chooses per value. GLUE is the mechanism
that finds and emits the transforms between whatever layouts two sides
independently chose, and optimizes the boundary away when they converge.

The system is designed from the ground up so that every language calling
Briev thinks it's calling itself.

---

## Implementation

The GLUE bridge generator has two implementations that produce identical output:

### Rust Pipeline (`src/glue/export.rs`)

The original implementation. Used by `briev export` for standalone bridge
generation. Full pipeline: parse → resolve frgns → compute protocol paths →
LLVM codegen → llc → template rendering → file writing.

### `.bv` Plugin (`lib/glue/generator.bv`)

A Briev-written equivalent using `$` intrinsics. Used by `briev build` with
inline `$(Normalized)` stage blocks. Exercises the full macro system:
`ConfigGet$`, `Tag$`, `TypeInfo$`, `StrReplace$`, `FileWrite$`,
`foreach`, `when` guards, string concatenation, and assignment.

The `.bv` plugin produces the same template-rendered output as the Rust
pipeline, proving that the `$` intrinsic system is complete enough to
implement a cross-language bridge generator entirely in Briev.
