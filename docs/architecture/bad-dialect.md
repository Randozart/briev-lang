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

_start: [r10 preserved]
    addr r4, msg               // rsi = buffer address (universal op)
    mov r5, 1                  // rdi = fd
    mov r2, 16                 // rdx = len
    mov r0, 1                  // rax = sys_write
    syscall
    mov r0, 60
    mov r5, 0
    syscall

// Inline exception: the add below is replaced on x86_64 only.
opt_add: [r0 valid]
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
| `mnemonic a, b; mnemonic c` | `;` is the universal instruction separator — any instruction line is a sequence (label bodies, defn bodies, branch rows, exception rows, `.bv` bad bodies alike) |
| `target => instr; instr` | exception — attaches to nearest preceding instruction (or whole defn in branch-defn) |
| `default => ...` | branch-defn default row (universal core syntax) |
| `name: [c] [c]` | code label — contract groups are POSITIONAL: one group = postcondition, two groups = pre then post. No `pre:`/`post:` keywords (see Contracts) |
| `defn name params` | defn head — owns following lines until next top-level line |
| `section .x` / `global n` / `.dir args` | top-level directives |
| `msg: .asciz "..."` | data label + directive |
| `[expr]` | inline contract for the next instruction |
| `^` / `^^` / `^^^` + instruction | acknowledge prefix — silences W-tier warnings for its scope; `^^^` overrides predicted errors (see the acknowledge tier) |
| `raw <target>` ... `end` | verbatim assembly block for ONE target — lines pass through unparsed; emitted only when the active family matches (see Raw blocks) |
| `raw <target> <name>` ... `end` | NAMED raw block — also emits a callable `<name>:` label on the matching family (per-arch stdlib entries) |
| `alias x = r0` | register, mnemonic, or label alias — resolved in that order |

Friendly mnemonic aliases (`Move`, `Add`, `JumpIfGreaterOrEqual`, …) load
by default from `std/bad/friendly.bad` (the prelude); `brievc bad --raw`
opts out. Raw core names always work — aliases are additive and
self-describing. `_start` is the one genuinely universal name and has NO
alias.

Disambiguation is pure token shape — no indentation sensitivity, no block
tracking. Blank lines are inert. `//` comments (respecting string literals).

## Core ISA (18 ops)

`mov add sub mul div mod mulhi mulhiu and or xor shl shr sar not neg
slt sltu load store ldb ldub ldh lduh stb sth loadoff storeoff cmp
jmp jz jnz jlt jle jgt jge jlo jls jhi jhs call ret push pop push2
pop2 nop syscall halt addr`

Floating point (double precision, `f0`-`f15`): `fmov fadd fsub fmul
fdiv fneg fabs fcmp fload fstore fjz fjnz fjlt fjle fjgt fjge itof
ftoi` — the fj family mirrors the integer j-family. Float literals
(`fmov f0, 1.5`) ride a deduped `.rodata` literal pool (x86_64
rip-relative, aarch64 adrp+ldr via x9, riscv64 la+fld via t0 —
clobbers disclosed).

`syscall num, a1, a2, a3` is portable end to end: the kernel call is
named (`syscall write, r5, r4, r2`) — numbers differ per target
(x86_64 write=1, aarch64/riscv64 write=64) and resolve through the
`syscall_nums` rows; the templates route the number and args into each
target's syscall ABI. Register operands only.

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

- `[rN preserved]` — **proven** when `rN` is callee-saved on the
  target (config property), or when the body carries balanced
  `push rN`/`pop rN` pairs. Caller-saved without pairing is a loud error
  stating the fix. Proven contracts are emitted as comments into the `.s`
  — the proof trail rides the artifact.
- `[rN valid]` — proven when `rN` maps on the target. (Full pointer
  validity proofs deferred.)
- **Positional groups**: a single bracket group is the postcondition
  (implied — matching `.bv` function contracts); two groups are pre then
  post: `name: [r0 valid] [r10 preserved]`. The `pre:`/`post:` keywords
  are gone — using them is a loud error that names the positional rule.
  `[frame: N]` keeps its keyword (a different proof kind).
- `[frame: N]` — static sp tracking at the target's `push_width`
  (8 on x86_64, 16 elsewhere): the body may stack at most N bytes, must
  restore sp exactly, and must hold 16-alignment at every `call`.
- `[sp % 16 == 0]`-style compare chains — the lhs register's existence is
  checked; constant folding lands with the comptime pass.
- `[frame: N]` also tracks direct `sub sp, sp, imm` / `add sp, sp, imm`
  displacement, not only push/pop discipline.
- Label-level (`name: [c] [c]`) and inline (`[expr]`
  before an instruction) forms. Inline contracts reject `frame:`/
  `pre:`/`post:` keywords — they are plain expressions.

## defn — two shapes, inlined as-is

1. **Sequence defn** — universal body lines (+ per-instruction
   exceptions). The body IS the default.
2. **Branch defn** — only `default?/target => instr; instr` rows; the
   `default` row re-enters the core pipeline (validates like ordinary
   code), a target row emits raw on match.

Params bind positionally; recursion is cycle-guarded (depth 64). Local
labels (`.name:`) are legal inside defn bodies with **hygienic per-call-
site gensym** (`L<defn>__<local>__<n>`) — double invocation never
collides. Invocation is at the instruction position (`ChargeGuest r1`);
a `Return` inside an inlined defn exits the ENCLOSING function (a
documented footgun — keep returns at top level).

## ABI boundary

`.export name` marks a C-ABI-visible entry point (emits `.global` and
validates the label exists). Argument registers come from the
`abi_args` row per target (SysV: x86_64 `r5,r4,r2,r1,r6,r7`; aarch64
and riscv64 are `r0..r5`) — the same `.bad` function body serves every
target because the ABI map is data. Args 7+ pass on the stack: the
first stack arg sits at `abi_stack_arg_base` bytes above `sp` at entry
(x86_64 = 8 past the return address; arm/riscv = 0) with 8-byte stride
— read them with `loadoff r?, sp, <off>`. `brievc bad --with-libc`
links `-lc` via the per-target `dynamic_linker` row so `call malloc`
and friends resolve. The stdlib (`std/bad/string.bad`) uses the
documented internal convention (args `r0`-`r2`, result `r0`,
`r3`-`r5` scratch).

`Arg dst, n` materializes the Nth C-ABI argument per target — register
move when `n <= abi_reg_args`, stack `loadoff` at `abi_stack_arg_base +
(n - reg_args - 1) * 8` beyond. Fully config-driven; no target knowledge
in the compiler.

## `bad` fns in .bv

`.bv` programs can declare a portable assembly function inline:

```briev
bad add(a: Int, b: Int) -> Int [result == a + b] {
    Add r0, r5, r4
}
```

Body is real `.bad` grammar (aliases active, `;` universal, contracts
positional). The trailing bracket is an implied **postcondition** — a
full Briev expression over the params and `result`; a leading group (if
present) is the precondition. Params bind PER TARGET through `abi_args`
(Int → r-regs) and `abi_args_fp` (Float → f-regs); the body references
the LOGICAL param names (`a`, `b`), which the pipeline resolves to the
target's C-ABI registers. The pipeline wraps the body in an entry label,
appends `ret` if absent, compiles it through the bad backend to `.s`,
assembles to `.o`, and links it into the binary; the LLVM IR references
the symbol via `declare` (call-site contracts are checked by the normal
Briev machinery). `bad fn` replaces `asm<Target>` (AsmFn is retained for
backward compatibility and deprecated).

## `bootstrap bad` — the authored machine entry

`bootstrap bad name() [post] { body }` (a plain `.bv` file) is the
**authored machine entry**: the body IS the reset vector / `.text.start`
routine. The compiler emits NO owned `_start` when one is present — the
author owns sp setup, `.bss` zeroing, the vector table, and the handoff
(`call main` / park / jump). The body is parsed VERBATIM (no `_entry:`
wrap, no auto-`ret`); the entry symbol is auto-exported so the linker's
`ENTRY(...)` resolves. The postcondition is **positional and taken on
authority** — raw `.bad` stores cannot carry typed-store proofs (matches
`post_authority`); reactor-side consumers still get the guarantee.

```briev
bootstrap bad Reset_Handler() [true] {
    section .isr_vector
    sp_slot:  .word 0x2007C000
    reset_v:  .word Reset_Handler + 1   // thumb bit set for Cortex-M
    section .text.start
    Reset_Handler:
    addr r0, msg
    ...                                   // MMIO, loops, handoff
    halt
    section .rodata
    msg: .asciz "Briev boot\n"
}
```

## Raw blocks — verbatim assembly for one target

`raw <target>` ... `end` emits its lines VERBATIM (no mnemonic
classification — directives like `.code32` work) for the target whose
family prefix matches, and skips them for every other target. The
ergonomic escape hatch for text the portable core ISA cannot express:

```bad
raw x86_64
    .code32
    cli
    movl $(gdt_end - gdt - 1), %eax
    lgdt gdt
    ...
    ljmp $0x08, $_start64
    .code64
end
_start64:
    // portable 64-bit core ops
```

The alternative — one `x86_64 => <line>` exception per line — cannot
carry directives (`.code32` in an exception row is an unknown-mnemonic
error) and is unergonomic for whole preambles. Raw blocks fix both.
Used by the x86 real-mode MBR body (`examples/bad/boot_mbr.bad`, with
the `int` core op for BIOS software interrupts) and the multiboot2 32-bit
prologue (`examples/bad/boot_multiboot.bad`). A bare `raw` with no target
or an unterminated block before EOF is a loud error.

## Per-arch stdlib boot entries (named raw blocks)

A NAMED raw block — `raw <target> <name>` ... `end` — emits a callable
`<name>:` label on the matching family (and nothing elsewhere), so the
portable core can `call uart_init` / `jmp uart_init` and the matching
family's block runs. Same name across families is fine: one build = one
target, so the label registers only for the active family (no namespace
collision). This makes per-arch boot prologues stdlib data:

```bad
// std/bad/arch.bad
raw riscv64 uart_init      // PMP grant + li a2, 0x10000000, then j core
    ...
end
raw thumbv7m uart_init     // vector table + ldr r2, =0x40004000, then b core
    ...
end
```

A `.bv` file imports them at TOP LEVEL — `import "std/bad/arch.bad";` —
the resolver records the `.bad` path (never parsed as Briev) and the bad
backend inlines it when compiling `bootstrap bad` bodies. The bootstrap
body is then a portable core that calls the imported primitives:

```briev
import "std/bad/arch.bad";
bootstrap bad Reset_Handler() [true] {
    Reset_Handler:
    jmp uart_init
core: [r2 valid]
    // portable: banner via call putc, handoff to a .bv defn, halt
}
```

`examples/bad/bootloader.bv` is this pattern: one source booting riscv64
(QEMU virt), thumbv7m (MPS2-AN385), and x86_64 (multiboot2) with only the
prologues in `arch.bad`. A `bootstrap bad` body can also CALL a real `.bv`
defn (`call kernel_bv`) — `defn_liveness` roots symbols referenced from
bootstrap bodies, so the handoff target is emitted. The console write
goes through a per-arch `putc` named raw block (store width differs per
UART: MPS2 wants byte stores, the virt 16550 a full-width store).

## Boot sectors (`--raw-bin` + `int`)

`int N` is the portable BIOS software-interrupt op (`int $N` on x86_64;
other targets get the loud capability error). A flat boot image is
`brievc bad file.bad --target x86_64 --raw-bin --no-link`: objcopy flattens
the object (no link — 16-bit relocs cannot link in a 64-bit ELF). A
512-byte MBR with the 0x55AA signature boots under SeaBIOS. For images
that need section merging (a multiboot header in `.text`), plain
`--raw-bin` links first then flattens.

QEMU-verified: `examples/bad/boot_mps2.bv` boots the MPS2-AN385
(Cortex-M3) with no `startup.S` and no compiler `_start`, printing through
the CMSDK APB UART. `examples/bad/boot_rv64.bv` boots QEMU virt the same
way on riscv64 (PMP grant + UART). thumb/arm assembly uses clang's
integrated assembler and ld.lld when the `arm-none-eabi` binutils are
absent (documented fallback, never a silent pass).

## CSR access (riscv64 M-mode)

`csrr d, csr` / `csrw csr, s` / `csrs csr, s` / `csrc csr, s` read/write/
set/clear a named CSR (`mcause`, `mepc`, `mtvec`, `mscratch`, `mie`,
`mstatus`, `pmpaddr0`, `pmpcfg0`, …); `mret` returns from M-mode trap.
The CSR name is a symbol operand — it substitutes literally. riscv64-only
rows: other targets get a loud capability error (no such registers),
never a silent pass. These are the ops the rv64 kernel bootstrap's `Asm#`
one-liners lower to.

## Raw binary output (`--raw-bin`)

`brievc bad file.bad --raw-bin` and `brievc build ... --raw-bin` extract
the flat loadable image (`objcopy -O binary`) from the linked ELF — the
boot-sector / firmware blob a bootloader would load. `boot_rv64.bin`
loads directly in QEMU `-kernel` and boots standalone. riscv64 bare-metal
objects assemble `-mabi=lp64` soft-float to match the `.bv` side's ABI.

The two-stage pattern (`examples/bad/boot_stage1.bv`) shows a bootloader
that does machine setup then CALLS a `kernel` routine in the same image
— the load-and-handoff bootstrapper, verified from the flat `.bin`.

## The acknowledge tier — predicting, not blocking

`.bad` **predicts** probable errors and lets the author **veto loudly**.
A `^` / `^^` / `^^^` prefix on an instruction line silences W-tier
warnings for its scope; `^^^` also overrides predicted errors. Nothing
blocks — a prediction is always surfaced, acknowledged ones print as info
under `--trace-lowering` (never silent).

```
^ mov r5, 1               // ack probable warnings on this instruction
^^ mov r5, 1; call f      // ack the whole `;`-separated line
^^^ mov r5, 1             // full authority: predicted errors too
^ W1 mov r5, 1            // ack only W1 (explicit name)
^ack W1 mov r5, 1         // keyword tail — future: ^seq, ^vol, ...
```

- Caret count = scope: 1 Instr, 2 Line, 3 Override. The keyword tail is
  the future-expansion slot — the grammar is open after `^`.
- **Three tiers**: hardware capability (no imm form, unmapped register)
  and author-declared contracts (`[rN preserved]`, `[frame: N]`) are
  NEVER ack-able — `^^^` overrides only what the *compiler concluded*,
  never what the *hardware forbids* and never what the *author declared*.
- **W1** caller-saved live across `call` · **W2** branch-path push/pop
  imbalance · **W3** `ret` with sp delta · **W4** FP-pool scratch
  collision (r9/r8) · **W5** defn-inlined `ret` · **W6** unresolved local
  label.
- **Stale markers can't rot**: an ack naming a warning that never fired
  is a loud error.
- Recorded, never silent: `--trace-lowering` prints acknowledged warnings
  as info with the line noted — the author's conscious disagreement is
  auditable and greppable.

## Cross-target verification

`brievc bad --run` executes the linked binary — natively on the host
family, under `qemu-<family>` (with the gnu sysroot when present)
otherwise. The cross toolchains come from the `cross_as`/`cross_ld`
rows; tests probe availability and skip with a printed note when a
toolchain is absent — never a silent pass. aarch64 `addr` rides
`adrp + add :lo12:` (full range, PIC-safe).

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
