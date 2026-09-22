# .bad Acknowledge Tier (`^`/`^^`/`^^^`) + `bootstrap bad`

**2026-09-22**

Two connected pieces that make `.bad` the right tool for bootstrap/embedded
work:

1. **Part A — the acknowledge tier.** A prefix modifier `^`/`^^`/`^^^`
   that acknowledges (silences) probable-error warnings on a `.bad` line.
   Informative, never restrictive — the compiler predicts, the author
   vetoes loudly. Records every veto; nothing is silently hidden.
2. **Part B — `bootstrap bad`.** Let the authored ARM/riscv QEMU-tested
   bootstrap code (`examples/addr_wired.b.bv`, `examples/kernel_rv64.b.bv`)
   be written as `.bad` bodies where the author owns the machine entry.

## Motivation

We drop to assembly for control, so the compiler must never be
restrictive. But silent assembly is a debugging nightmare. The contract
system already proves **definite** errors (arity, missing imm form,
unmapped register, `addr r5, 1`, unprovable `[rN preserved]`, unbalanced
`[frame: N]`). What is missing is the **probable-error tier** — the
compiler predicting likely bugs and letting the author consciously accept
them, per line, with a marker that can't silently rot.

The second piece is about expressiveness parity: the MPS2 ARM bootstrap
and the rv64 kernel are written today with `bootstrap node` + `Asm#("raw",
...)` one-liners. A `bootstrap bad` lets the whole entry be a `.bad` body
— portable core ISA + `target =>` rows, contracts, aliases, comptime —
with the author owning the full machine entry.

## Part A — the acknowledge tier

### Syntax

A prefix modifier on an instruction line (or `target =>` exception row):

```
^ mov r5, 1               // acknowledge probable warnings on this instruction
^^ mov r5, 1; call f      // acknowledge the whole line (all `;` segments)
^^^ mov r5, 1             // FULL OVERRIDE — predicted errors too (recorded, never silent)
^ W1 mov r5, 1            // acknowledge only W1
^ack W1 mov r5, 1         // explicit keyword form — future: ^seq, ^vol, ^deliberate...
```

- `^` = scope: this instruction. `^^` = scope: whole line. `^^^` = full
  authority (overrides predicted *errors* too, not just probable warnings).
- Bare `^` = default ack (acknowledge all probable warnings on scope).
  Keyword tail (`^ack`, `^seq`, …) is the future-expansion slot — the
  grammar is open after `^`, never a naive one-character lock.
- Specific warning names: `W1`, `W2`, … Optional, space-separated after
  the caret(s) and/or keyword.
- The marker is consumed at parse, never emitted. Stale markers (a named
  warning that never fired) are a loud error — markers can't rot.

### The three-tier boundary

| Tier | Example | Overridable? |
|---|---|---|
| Hardware capability | aarch64 `mul` with immediate (no encoding) | **No** — emitting is garbage; the fix is a `target =>` exception, not an override |
| Author-declared contract | `[r10 preserved]` violated, `[frame: N]` exceeded | **No** — the author chose it; change it or drop it (Rule 1) |
| Analysis prediction | probable W-tier, predicted definite errors | **Yes** — `^`/`^^`/`^^^` |

`^^^` overrides what the *compiler concluded*, never what the *hardware
forbids* and never what the *author declared*.

### W-tier warnings (first set)

| # | Warning | Why probable, not definite |
|---|---|---|
| W1 | Caller-saved register live across a `call` with no preservation claim | Callee may not touch it |
| W2 | Push/pop imbalance on a branch-defn path | Deliberate stack juggle possible |
| W3 | `ret` reached with nonzero sp delta | Hand-rolled frame teardown possible |
| W4 | Disclosed clobber collision (FP-pool scratch r9/r8) with a live value | Intentional reuse possible |
| W5 | Inlined defn containing `ret` (documented footgun) | Sometimes exactly what you want |
| W6 | `.local` label referenced outside its scope | Forward refs may be fine |

(W7, "syscall arg of the wrong class", was removed during implementation:
a syscall immediate is already a HARD capability error — the no-imm-form
guard in `bad-isa.dbvl` owns that case, and it belongs to the untouchable
tier, not the ack-able one.)

All are static-analysis heuristics in the lowerer — never hard errors by
default, always suppressible per line.

### Recording, never hiding

Acknowledged warnings still print under `--trace-lowering` as
`info: ... (acknowledged at line N)`. Nothing is deleted from the output;
the author's conscious disagreement is auditable and greppable.

## Part B — `bootstrap bad`

### Syntax

```briev
// thumbv7m-none-eabi, .b.bv bare profile
bootstrap bad reset() {
    section .isr_vector
    sp_slot:  .word 0x2007C000
    reset_v:  .word reset
    section .text.start
    reset:
    Move r?, 1                       // UART TX on (was VolatileStore#)
    ...
    Call main                        // enter the reactor, or park / jump to a kernel
}
```

- A `bootstrap bad` BadFn whose body IS the authored entry. When present,
  the compiler emits **no** owned `_start` — the author owns sp setup,
  `.bss` zeroing, the vector table, and the handoff.
- Body ends in `call main` (reactor), a park, or a jump to a loaded
  kernel — author's choice.
- Postcondition is **positional and taken on authority**: `[booted == true]`
  after the parens; raw `.bad` stores can't carry typed-store proofs, so
  it's claimed (matches `post_authority`). Reactor-side consumers still
  get the guarantee.

### Prerequisites

1. **thumb/arm rows** in `config/bad-isa.dbvl` + `config/bad-registers.dbvl`
   — registers (`r0`-`r15` tokens), `push_width`, `halt` (wfi), `abi_args`,
   `comment`, `imm`. Nothing exists today; `.word` vector-table data labels
   already work.
2. **Freestanding link fix** — `src/compile.rs:2130` (the `else` branch)
   drops `extra_objects` in the bare path. Bad `.o` files must be linked
   there too.
3. **AST + parser** — `TopLevel::BadFn` gains a `bootstrap: bool` flag (or
   a `TopLevel::BootstrapBad` variant); `bootstrap bad` keyword routing in
   the `.bv` parser; raw body capture (reuse the Phase-4 brace-capture).
4. **Pipeline** — skip owned `_start` when a `bootstrap bad` exists;
   compile body through the bad backend → `.s` → `.o` → link freestanding.
5. **Port + QEMU-verify**: `addr_wired.b.bv` (MPS2-AN385) and
   `kernel_rv64.b.bv`'s bootstrap → `bootstrap bad`.

### Deferred (next phase)

Pure bootloader (no reactor) + raw-binary/objcopy output + boot headers
(multiboot/UEFI/DTB).

## Work order

1. Part A: parser prefix strip → lowerer W-tier analysis → ack recording
   → tests. Self-contained, low risk.
2. Part B: thumb/arm config rows → freestanding link fix → `bootstrap bad`
   AST/parser/pipeline → port + QEMU-verify both examples.

## Undo

Part A: delete the prefix strip, the W-tier pass, and the recording lines;
revert the parser test vectors. Part B: revert the config rows, the link
fix, the BadFn flag, and the examples.

## Doc updates

- `docs/architecture/bad-dialect.md`: the acknowledge tier section (three
  tiers, W-table, recording), `bootstrap bad` section.
- `spec/SPEC.md` §20: `bootstrap bad` declaration + the acknowledge tier.
- `docs/architecture/machine-entry.md`: `bootstrap bad` as an authored-entry
  form beside `bootstrap node`.