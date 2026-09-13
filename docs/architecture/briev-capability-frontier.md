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

## The DSL guardrails

The systems tier (the rv64 arc) grew two constructs — `bootstrap node` and
`node @ wiring` — that touch hardware. The standing test for whether such
constructs make Briev a *domain-specific* language, applied throughout the
arc and recorded here as doctrine:

1. **The domain test** — does the construct only mean something in one
   domain? `bootstrap` = an entry the machine starts (a program beginning
   — kernel, PID 1, daemon: one shape). `@` wiring = eligibility by event
   source (interrupts, polled devices, and — future — sockets, signals,
   GUI events: one shape). Neither is interrupt-specific.
2. **The knowledge test** — does the *compiler* carry domain knowledge?
   The machine facts live in three homes, none of them Rust: target
   profiles (`isr_mechanism`), the mechanism registry (conventions,
   `full_context`), and board files (`interrupts.dbvl`,
   `addresses.dbvl`). A second architecture is data.
3. **The reverse-test** — would another domain reuse the constructs
   naturally? The kernel uses the same `when`/`match`/`defn`/state-field
   syntax as a hosted app; a hosted event program would use `@` wiring
   and a machine entry the same way.

The temptations rejected en route — each resurfaced at a different
layer, each rejected for the same reason (machine knowledge belongs in
config or library, never in compiler branches):

| Temptation | Where it resurfaced | Where it lives instead |
|---|---|---|
| CSR access as compiler intrinsics (`set_mepc` intrinsic) | the register-shim design | the kernel's `.bv` shim library, typed one-liners over `Asm#` |
| Per-arch scaffolds as Rust match arms on arch names | the `full_context` scaffold design | the mechanism registry row (`full_context` field) + one emitter arm |
| `@` semantics keyed to arch/board names in Rust | the `@` wiring design | board-file namespaces (`interrupts.dbvl` vs `addresses.dbvl`) |
| Folding `Asm#` results to known values | the kernel freeze investigation | the three-tier value model: proven / asserted (`:=`)/ runtime-unknown — unknowns never fold |

The keyword-choice corollary: machine-entry syntax stays
`bootstrap` — not `boot` — because `boot` is a natural *user identifier*
(the Phase 3 timer demo had a `node boot`) and the longer form is the
canonical systems term, self-documenting at the declaration site.

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

## PROVEN: bare-metal systems programming (rv64 arc, 2026-09-11 → 14)

The tier table's rows became **gates on real hardware emulation** — each
one a QEMU run, not an argument. Branch `feat/rv64-capability-kernel`,
gates in `tests/bare/`:

| Gate | Output | Proves |
|---|---|---|
| `qemu-rv64-kernel.sh` | `BABABABABABA…` | **A preemptive two-task micro-kernel**: authored machine entry, full-context trap scaffold, mcause dispatch, restart scheduler over a trap frame, ecall syscall boundary, two U-mode tasks — 8,520 bytes, pure Briev, no C, no runtime |
| `qemu-rv64-timer.sh` | `123456789012…` | Machine-timer interrupts serviced from a typed handler; the reactor parked at `wfi` woken by hardware |
| (bootstrap) | `briev` | The authored machine entry — PMP, mscratch/kernel-stack, task contexts, mtvec, MTIE+MIE — in language-level syntax (`bootstrap node`), compiler-owned ISA scaffold only |

Language grown by the arc — each piece *disclosed special treatment*, none
a special case: `bootstrap node` (the authored machine entry; the canned
`_start` becomes the fallback), `node @ vector` (machine-serviced events;
the `isr` keyword dissolved; the mechanism inferred from the target
profile), the `full_context` convention (the preemptive save-all scaffold
as registry data), defn-liveness emission (imports grant capability,
liveness gates emission — 13 defines, not 252).

**The residual gap vs C is ecosystem, not mechanism**: what a C kernel has
that this kernel lacks is *drivers, an RTOS ecosystem, thirty years of
soaked example code* — artifacts of maturity, expressible in Briev by the
same constructs the gates demonstrate. The four real bugs the kernel
exposed (expression-bodied `ret 0`, the callable-txn convergence exit,
the `--no-std` no-op, the Asm# operand check) were each **fixed as
language-layer defects** — exactly the capability-frontier thesis: building
the hard thing finds the gaps; the gaps close as language, not as
workaround. Full record: `2026-09-11-rv64-capability-kernel.md`,
`2026-09-14-bootstrap-kernel.md`, `machine-entry.md`.

**The remaining rung to self-hosting** is unchanged (the native emission
tier) — but the systems-programming tier between "hosted user code" and
"self-hosted compiler" is now *occupied*: Briev runs on the bare metal,
preempts itself, and services its own traps, in language-level syntax with
contracts at the boundary.
