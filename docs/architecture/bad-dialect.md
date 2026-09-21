<!-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception -->
# .bad — Briev Assembly Dialect

**2026-09-21** (plan: `docs/plans/2026-09-21-bad-assembly-dialect.md`). A
portable assembly dialect: a universal core ISA with zero source ceremony,
`target =>` exception/default granularity for architectural optimization,
compile-time-inlined `defn`s, and Briev-style formal contracts.

`.bad` is a **separate dialect** — it never enters the .bv pipeline (no
lexer, typecheck, or reactor). Compile with:

```sh
brievc bad <file.bad> [--target <triple>] [--emit-asm]
```

## The one-page program

```bad
section .text
global _start

_start: [post: r10 preserved]
    addr r4, msg               // rsi = buffer address (universal op)
    mov r5, 1                  // rdi = fd
    mov r2, 16                 // rdx = len
    mov r0, 1                  // rax = sys_write
    syscall
    mov r0, 60
    mov r5, 0
    syscall

// Inline exception: the add below is replaced on x86_64 only.
opt_add: [pre: r0 valid]
    add r0, r0, r1
    x86_64 => lea r0, [r1 + 1]
    ret

// Branch defn: default row = universal core syntax; rows are inlined as-is.
defn store_pair x, addr
    default => store x, addr; add r2, addr, 8; store x, r2
    x86_64 => movq [addr], x
    aarch64 => stp x, x, [addr]

section .data
msg: .asciz "hello from .bad\n"
```

## Grammar (strictly line-oriented, zero braces)

| Line shape | Meaning |
|---|---|
| `mnemonic operands` | instruction — belongs to nearest preceding label/defn |
| `target => instr; instr` | exception — attaches to nearest preceding instruction (or whole defn in branch-defn) |
| `default => ...` | branch-defn default row (universal core syntax) |
| `name: [pre: c] [post: c]` | code label (colon form keeps token-shape disambiguation deterministic) |
| `defn name params` | defn head — owns following lines until next top-level line |
| `section .x` / `global n` / `.dir args` | top-level directives |
| `msg: .asciz "..."` | data label + directive |
| `[expr]` | inline contract for the next instruction |
| `alias x = r0` | register alias |

Disambiguation is pure token shape — no indentation sensitivity, no block
tracking. Blank lines are inert. `//` comments (respecting string literals).

## Core ISA (18 ops)

`mov add sub mul div mod mulhi mulhiu and or xor shl shr sar not neg
slt sltu load store ldb ldub ldh lduh stb sth loadoff storeoff cmp
jmp jz jnz jlt jle jgt jge jlo jls jhi jhs call ret push pop push2
pop2 nop syscall halt addr`

Floating point (double precision, `f0`-`f15`): `fmov fadd fsub fmul
fdiv fneg fabs fcmp fload fstore fjz fjnz fjlt fjle fjgt fjge itof
ftoi` — the fj family mirrors the integer j-family.

Universal lowerings live in `config/bad-isa.dbvl` — one row per op,
per-target `"target:reg-form|imm-form"` fields:

- **No `|`** — the reg form takes immediates too (`movq $1, %rax`,
  `mov x0, #42`).
- **`|form`** — distinct immediate form (`mv|li`, `add|addi`,
  `sub|addi(-)`).
- **`|-`** — the hardware has no immediate form; an immediate operand is
  a loud capability error (`mul` on aarch64/riscv64).
- A trailing **`"sym"`** field marks ops that take a label/symbol operand
  (`jmp jz jnz call addr`). Everywhere else an unresolvable name is a loud
  error — `mov r0, msg` on x86 would load MEMORY, not the address; the
  compiler rejects it and points at `addr`.
- `;` inside a template splits into separate emitted lines (riscv push/pop).
- Templates may name physical registers directly (x86 `div` clobbers
  `%rax`/`%rdx`; riscv branch-imm borrows `t0`) — the row author's
  contract, disclosed here.

`addr d, sym` is the portable address-of op (`leaq sym(%rip), %rdi` /
`adr` / `la`). `cmp` has **no riscv64 row** — riscv has no flags; use the
portable compare-and-branch ops `jz a, b, label` / `jnz a, b, label`.

Offset access is a first-class op: `loadoff`/`storeoff d, base, imm`.
Sub-width fields ride `ldb/ldub/ldh/lduh/stb/sth` via `.w8/.w16/.w32`
width-token register rows (x86 `%al`, aarch64 `w`-regs). Template refs:
`$N.w8` = width-qualified register; `$N!` = raw substitute (no imm
prefix — x86 displacements are bare).

## Registers

`config/bad-registers.dbvl` maps the built-in portable set —
`r0`-`r15`, `sp`, `pc` — to assembler tokens per target with proof
properties (`caller` / `callee` / `ro`):

- A target absent from a row = the register does not exist there
  (`r14`/`r15` have no x86_64 field: 16 GPRs). Referencing it is a loud
  error, never a silent remap.
- Properties feed the contract proofs, never codegen branches.
- `alias result = r0` is source sugar resolved before mapping.
- The `imm` row holds each target's immediate prefix (`$` / `#` / empty);
  the `comment` row its GAS comment prefix.

## Contracts

- `[post: rN preserved]` — **proven** when `rN` is callee-saved on the
  target (config property), or when the body carries balanced
  `push rN`/`pop rN` pairs. Caller-saved without pairing is a loud error
  stating the fix. Proven contracts are emitted as comments into the `.s`
  — the proof trail rides the artifact.
- `[pre: rN valid]` — proven when `rN` maps on the target. (Full pointer
  validity proofs deferred.)
- `[frame: N]` — static sp tracking at the target's `push_width`
  (8 on x86_64, 16 elsewhere): the body may stack at most N bytes, must
  restore sp exactly, and must hold 16-alignment at every `call`.
- `[sp % 16 == 0]`-style compare chains — the lhs register's existence is
  checked; constant folding lands with the comptime pass.
- Label-level (`name: [pre: ...] [post: ...]`) and inline (`[expr]`
  before an instruction) forms.

## defn — two shapes, inlined as-is

1. **Sequence defn** — universal body lines (+ per-instruction
   exceptions). The body IS the default.
2. **Branch defn** — only `default?/target => instr; instr` rows; the
   `default` row re-enters the core pipeline (validates like ordinary
   code), a target row emits raw on match.

Params bind positionally; recursion is cycle-guarded (depth 64).

## ABI boundary

`.export name` marks a C-ABI-visible entry point (emits `.global` and
validates the label exists). Argument registers come from the
`abi_args` row per target (SysV: x86_64 `r5,r4,r2,r1,r6,r7`; aarch64
and riscv64 are `r0..r5`) — the same `.bad` function body serves every
target because the ABI map is data. `brievc bad --with-libc` links
`-lc` via the per-target `dynamic_linker` row so `call malloc` and
friends resolve. The stdlib (`std/bad/string.bad`) uses the documented
internal convention (args `r0`-`r2`, result `r0`, `r3`-`r5` scratch).

## Error doctrine

Every failure is loud with what/why/fix: unknown mnemonic (names the core
set), arity mismatch, missing target lowering (points at the config row or
the `target =>` gate), missing immediate form, unmapped register (lists
the targets that have it), unprovable contract (states the property that
blocks it). No silent remaps, no silent substitutions.

## Implementation map

| Piece | Location |
|---|---|
| AST | `src/ast/bad.rs` |
| Parser | `src/parser/bad.rs` |
| Config registries | `src/backend/bad/registry.rs` + `config/bad-isa.dbvl` + `config/bad-registers.dbvl` |
| Comptime evaluator | `src/backend/bad/comptime.rs` |
| Lowerer | `src/backend/bad/lower.rs` |
| Stdlib | `std/bad/string.bad` |
| Contract proofs | `src/backend/bad/contracts.rs` |
| Entry + assembler path | `src/backend/bad/mod.rs`, `brievc bad` in `src/main.rs` |
| Target routing | `BackendKind::Bad` + `.bad` row in `config/targets.dbvl` |

To undo: delete those files, revert `BackendKind::Bad`, the targets row,
and the doc sections.
