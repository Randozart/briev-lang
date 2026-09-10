# Briev Capability Frontier

**2026-09-10** · Status: PRINCIPLE + ASSESSMENT (living doc)
**Companion to:** the expressiveness-closure amendments in
`docs/plans/2026-09-09-briev-native-runtime-and-family-realignment.md`

## The principle: expressiveness closure

> The compiler's chosen optimum must never be more powerful than the
> language. If the compiler can do X, a user must be able to build X in
> Briev.

Briev's compiler exists to pick the most optimal path automatically. That
mission has a hidden precondition: whenever computer science invents a new
optimal path — a new allocator discipline, a new calling convention, a new
cache trick — that path must itself be *expressible in Briev*. Otherwise the
compiler's cleverness is a ceiling instead of a floor: users wait for Rust
intrinsic work to reach the technique instead of building it themselves.

Rule 14 ("stdlib is the extension mechanism") is this principle applied to
features. Expressiveness closure is the same principle applied to
*techniques*: no runtime, allocator, or hardware trick may permanently
require a compiler change.

The architecture is already closure-shaped at three levels:

- **Intrinsics are overridable** — the declare-guard lets a stdlib defn
  replace a backend-emitted symbol (print family, cast lanes, allocator
  strategies — proven in the briev-native-runtime work).
- **The pipeline is user-extendable** — `plugins/parsed/*.bv` are compiler
  passes written in Briev (the prelude itself is one).
- **The runtime is replaceable** — `briev_rt.c` is being eliminated family
  by family; the endgame is a Briev-owned runtime with a bootstrap fallback.

## Evidence: the bottom tier is already written in Briev

The briev-native-runtime branch replaced the C runtime's hardest routines
with pure Briev, byte-identical:

- **Correctly-rounded float→decimal conversion** (`%.9g` vs C printf) —
  Ryu/Grisu-class algorithmics: IEEE-754 bit decomposition, exact
  leading-digit extraction via shift-and-scale with an 18-digit headroom
  window, long-division digit generation, round-half-even. This is the
  algorithmic heart of printf.
- **UTF8 decode/encode** with multi-byte width tracking.
- **Raw byte-level memory discipline** — `Load#`/`Store#` over
  `[len][bytes]` layouts, header reads, `Copy#` bulk moves, the
  `#13` full-memory semantics that make int-address dereferencing sound
  under LLVM's optimizer.
- **OS interface without libc** — `SysCall#` inline asm (`syscall`/`svc #0`):
  write(2), exit_group, brk/mmap.

That is precisely the code class LLVM's foundations are made of.

## The tier table

| Tier | Techniques | Status |
|---|---|---|
| **Expressible today** | Arena/bump/pool/free-list allocators, tagged pointers, slab allocation, lock-free structures (atomics), zero-copy parsing, SoA layouts via `spec`, cache-line alignment masking | ✅ primitives proven (`cast_lanes.bv`, `SysCall#`, `#13`) |
| **One primitive away** | Prefetch hints, hand-written SIMD, `rdtsc`/`cpuid`, TLS access, fiber/stack switching, performance counters | 🔜 `Asm#` — one intrinsic (two modes: abstract lowering table + raw escape); `SysCall#` already proves the emitter |
| **Analysis, not expressiveness** | The compiler *optimizing through* user constructs — alias analysis of a user arena, provenance for the vectorizer | The constructs are expressible; inference is the compiler's job |
| **Genuinely future** | Stable addresses (`&local`), intrusive pointer-linked structures at the language surface | Backend has allocas; the language surface needs an address-of form + a stable-address contract class |

## The philosophical inversion

LLVM's deepest lever is **undefined behavior** — poison, dereferenceable
assumptions, `argmem` claims that let the optimizer assume. Briev cannot and
should not play that game; its lever is **proof**. Where LLVM says "trust
me, this function is argmem-only" (and — see the `#13` fix — lies cost
miscompiles), Briev's model says `[pre][post]` prove it.

An LLVM-class system written in Briev would therefore be a different
species: SSA dominance, use-def consistency, and pass-pipeline invariants
expressed as **contracts on the compiler itself**, checked at the
compiler's own compile time. "Contracts as fuel" applies recursively to the
compiler writing itself. That is not a weakness relative to C++ — it is the
one place Briev would be strictly more rigorous.

## Self-hosting endgame

The bootstrap chain today: **rustc → LLVM → machine code** — C++ sits at
the root of every Briev binary. Breaking it needs a native emission tier:
a backend that emits x86-64/aarch64 machine code (or `.s` text) directly,
the same class of work as the existing SPIR-V and CIRCT emitters. QBE or
tcc scale — tens of thousands of lines, not LLVM's engineer-decades.

The embryo exists: `lib/compiler/*.bv` (self-hosted parser, lexer,
typechecker sources), the tamer VM, the `compiler-in-Briv` dogfood passes.
LLVM is explicitly **not** the goal — *theoretical capability* is: given
any future optimal path, Briev must be able to express it, bootstrap
included.

## Fundamentals this principle demands

1. **Allocator ownership** — allocation strategy in `lib/std/alloc.bv` +
   `alloc-strategies.dbvl` (config exists); compiler keeps only the
   `--no-std` bootstrap heap.
2. **`Asm#`** — the two-mode intrinsic (abstract lowering table + raw
   escape) that guarantees no hardware trick ever needs a Rust change.
3. **Stable addresses** (future) — address-of form with a contract class
   for intrusive structures.
