<!-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception -->
# The .bld dialect — BILLD (Briev Intermediate Low-Level Dialect)

**2026-10-08** (plan `docs/plans/2026-10-08-billd-intermediate-dialect.md`,
milestones M2–M7 same day). BILLD is the execution-recipe tier between
`.bv` (bounded, proven, reactor) and `.bad` (physical registers,
line-oriented):

| Tier | Dialect | Nature | Guarantee |
|---|---|---|---|
| High | `.bv` Briev | expressive, proven | bounded loops, reactor, contracts |
| **Mid** | **`.bld` BILLD** | **execution recipe / engine manual** | **unbounded loops, symbolic registers, naked code** |
| Low | `.bad` Briev Assembly | exact instructions | physical registers r0–r15, per-target rows |

The gap it fills: assembly is too granular (the author micromanages
scratch registers for values with no architectural meaning); C is too
detached (stack, frames, ABI, virtual memory — you cannot write the
first fifty instructions of a boot sequence in it). BILLD is **the
architecture of the machine without the chore of the scratchpad**:
symbolic dataflow + PascalCase engine verbs.

## Pipeline

```
.bld source
  → parser   (src/parser/bld.rs — braced Briev-style expressions)
  → AST      (src/backend/../ast/bld.rs)
  → lower    (src/backend/bld/lower.rs)   → BadProgram
  → allocate (src/backend/bld/alloc.rs)   → virtuals resolve
  → emit     (src/backend/bad/*)          → .s (UNCHANGED .bad backend)
```

The ladder is strict: BILLD never enters the `.bv` pipeline (no reactor,
no termination gates); the `.bad` backend consumes an ordinary
`BadProgram` — same registries, same targets (x86_64, aarch64, riscv64,
thumbv7m), same `.s`/assemble/link/flatten/run tooling.

## Grammar

```
item     := import | const | defn | bootstrap
import   := 'import' STRING ';'
const    := 'const' NAME '=' expr ';'
defn     := 'defn' NAME '(' params ')' ( '->' type )? block
bootstrap:= 'bootstrap' NAME '(' ')' block          (exactly one per image)

stmt     := let | assign/compound | call | when | loop | while
          | break | continue | return | block | bad
let      := 'let' NAME (':' type)? '=' expr ';'
assign   := expr ('=' | compound) expr ';'          (target: name in v1)
call     := NAME '(' args ')' ';'                   (verb or defn)
when     := 'when' expr block ('else' ('when' … | block))?
loop     := 'loop' block                            (unbounded; exits via break)
while    := 'while' expr block
bad      := 'bad' '{' <verbatim .bad grammar> '}'
return   := 'return' expr? ';'

expr     := shared Briev `Expr` grammar (precedence, calls, arithmetic)
```

- **`when`, never `if`** — Briev's conditional is `when` in every
  dialect; a statement-head `if` errors with a `when` fix.
- **Compound assignment** (`x |= 1`) desugars to `x = x | 1`.
- Expressions share the Briev expression parser: `&`/`|` bind tighter
  than `==`/`!=`.
- Contracts: none in the v1 `.bld` grammar — `bad { }` blocks carry
  `.bad` positional contracts where they matter.
- Naked semantics: no prologue, no auto-`ret`. What you write is the
  sequence. (A compiler-opened frame is the one documented exception —
  see the allocator below.)

## Values and the tier ladder of storage

Every value is one of four classes — **Int, Float, Bool, Ptr** — derived
from annotations (`i: Int`, `p: Ptr`, Bits widths) or propagation.
Int/Bool/Ptr interchange as the same machine word; Int↔Float converts
automatically (itof/ftoi; a fractional Float → Int constant is loud).

Storage resolves in tiers:

1. **Constants fold** at compile time (loud on overflow, division by
   zero, out-of-range shifts; `>>` is the LOGICAL shift — `shrq`/`lsr`/
   `srl`; Float division by zero folds to inf, the hardware behavior).
2. **Leaf recipes keep parameters in their ABI registers** — zero cost.
3. **Fresh single-def temps adopt their register** — `let b = a + 1`
   emits `add v0, …` with no second copy.
4. **Everything else names a virtual** (`vN` integer-class, `fvN`
   float — the class rides the name) that the **M4 allocator**
   resolves: linear scan over registry-derived pools, loop-carried
   intervals extended across backedges, call-crossing values forced
   into callee-saved registers or frame slots, spilled values
   reloaded through reserved scratches, frames push/pop the
   callee-saved registers they use and restore before every `ret`.
5. **Physical registers** (`r0`–`r15`, `f0`–`f15`) appear only inside
   `bad { }` blocks and the ABI binding of leaf parameters.

A recipe that touches `sp` cannot take a compiler frame — spill needs
there are loud, naming the values.

## Calls

- **`defn` calls** use the C-ABI: integer-class arguments take the
  `abi_args` order, floats the `abi_args_fp` order, results return in
  r0/f0. Arguments stage through fresh virtuals before the ABI copies
  (parallel-move safety: `f(b, a)` with both already in argument
  registers must not clobber a source mid-copy). A recipe whose body
  calls anything stashes its parameters out of the caller-saved ABI
  registers at entry (the callee would clobber them).
- **Engine verbs** (below) inline at the call site — no staging, no
  convention.
- **Imported `.bad` labels** (and named raw blocks) are external C-ABI
  calls; `.bad` sequence `defn`s are NOT callable from `.bld` (they are
  inline material for `bad { }` blocks — no label exists to call).

## Engine verbs — `config/bld-intrinsics.dbvl`

Registry rows, not Rust matches (Rules 3/15/23). Each row: arity,
`ret`/`void`, per-target sequences of `.bad` core instructions. `$N`
splices the call's argument operand; `$N!` demands a compile-time
constant (opcode-encoded numbers — CR/CSR numbers live in the
instruction itself). A `"ret"` verb's result arrives in r0. A verb
without a row for the target is a loud capability error naming the
available targets.

| Verb | Arity | Result | Notes |
|---|---|---|---|
| `ReadControlReg(n)` | 1 | Int | CR/CSR number is a constant; x86 `movq %crN`, riscv `csrr` |
| `WriteControlReg(n, v)` | 2 | void | value staged through a register (CR writes need one) |
| `DisableInterrupts` / `EnableInterrupts` | 0 | void | x86 `cli`/`sti`, riscv `csrrc/csrrs` mstatus (768); aarch64 = capability error until verified DAIF rows land |
| `Halt` | 0 | void | x86 `cli; halt`, other targets `halt` (`wfi`) |
| `WaitForInterrupt` | 0 | void | `wfi` everywhere (x86 renders `hlt`) |
| `MemoryBarrier` | 0 | void | `mfence` / `dsb sy` / `fence iorw, iorw` |
| `InvalidateTlb` | 0 | void | CR3 reload (clobbers %rax — the row's contract) / `tlbi vmalle1` / `sfence.vma` |
| `LoadDescriptorTable(m)` | 1 | void | x86 `lgdt` only |
| `FarJump(seg, off)` | 2 | void | x86 only (segmentation is x86; others refuse rather than drop the segment) |
| `Store(v, a)` / `Load(a)` | 2/1 | void/Int | the tier's memory statements over the universal `store`/`load` core ops |

Deliberate v1 gaps are LOUD, never guessed: aarch64 CR/DAIF rows need
verified `S3_x` encodings; guessed encodings are worse than honest
refusals (zero-tolerance beats table completeness). An engine verb that
cannot express its sequence falls to a `bad { }` raw block.

## `bad { }` blocks

Verbatim `.bad` grammar in the dialect itself. The text parses inside a
`_bldwrap:` label so instruction lines can never orphan: they splice
into the recipe body at any position. **Ownership items** — sections,
data labels, `defn`s, raw blocks — attach only at the recipe's FIRST or
LAST statement (`.bad` ownership is positional; a mid-recipe owner
would steal the instructions after it). Every recipe re-states
`section .text` before its label, because an ownership block may have
switched sections. `bad { }` blocks work in physical registers; naming
a compiler-managed `vN` there is loud.

## Imports

`import "x.bld"` merges modules recursively (canonical-path dedup;
cycles terminate); `import "x.bad"` passes through as a directive for
the .bad backend (absolutized paths) with its labels harvested as
external call signatures. Duplicate defn/const/bootstrap names across
the merged stream — and names shadowing engine verbs or the compiler's
`vN`/`fvN` value namespace — are loud at collect.

## The registry rows BILLD adds to `config/bad-isa.dbvl`

`readcr`, `writecr` (opcode-encoded CR/CSR numbers spliced bare),
`cli`, `sti`, `wfi`, `lgdt`, `invlpg`, `tlbflush`, `fence`, `ljmp`.
Row templates contain **final assembly** — they are never re-mapped
(`t1`, not the canonical `r9`; `mv`/`li`, not `mov`). Template-only
physical-register clobbers are the row author's contract
(`tlbflush` clobbers %rax — precedent: asm-lowering's `csrrs $0, time,
zero`).

## Entry and examples

`bootstrap Name() { … }` is the authored machine entry (established
terminology: `machine-entry.md`) — exported with `.global`, entered by
the linker. Exactly one per image.

- `examples/bld/boot_protected_x86.bld` — multiboot2 kernel: the
  32-bit prologue rides the FIRST-statement `bad { }`; the 64-bit body
  is BILLD statements (value loop, MMIO stores, engine verbs).
- `examples/bld/boot_rv64.bld` — QEMU virt: PMP grant via
  `WriteControlReg`, UART banner via `bad { }` MMIO loop, `Halt`.
- `examples/bld/boot_aarch64.bld` — PL011 banner entirely through the
  `Store` verb.
- Gates: `tests/bare/qemu-bld-{x86,rv64,aarch64}.sh` (toolchain absent
  = printed SKIP, never silent).

## To undo

Delete `src/backend/bld/`, `src/parser/bld.rs`, `src/ast/bld.rs`,
`config/bld-intrinsics.dbvl`, `lib/std/bld/`, `examples/bld/`,
`tests/bare/qemu-bld-*.sh`, this doc, SPEC §20.2, and revert:
`BackendKind::Bld` (+ resolve + targets.dbvl/golden row),
`SourceKind::Bld` (+ classify + sweep arm), `brievc bld` + `.bld`
default-path route + `run_bld`/`parse_bld_cli`, the compile.rs arms,
`emit_asm_artifacts`/`route_by_extension` extraction, the `pub` on
`density::is_float_type`, the five compound-assign lexer tokens, the
`abi_args_fp` scalar-row listing, the store/load/mov/writecr/readcr/
cli/sti/wfi/lgdt/invlpg/tlbflush/fence/ljmp ISA rows, and the
harvesting narrowing for `.bad` defns.
