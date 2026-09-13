# Machine Entry — bootstrap, event vectors, and the ABI layers

**2026-09-14.** How Briev programs meet the machine: who enters where, who
owns which register protocol, and where each piece of machine knowledge
lives. Companion to `briev-execution-model.md` (the reactor) and the plan
`2026-09-14-bootstrap-kernel.md`.

One sentence: **entries are declared, machines wire them, compilers own ABI,
users own policy.**

## The three entry classes

| Form | Fired by | Scaffold (compiler-owned) | Contracts |
|---|---|---|---|
| `bootstrap node <n> [<post>] { … }` | the reset vector | `sp ← _stack_top`, `.bss` zero, body, hand off to the reactor | handoff postcondition, proven from body stores |
| `node <n> @ <vector> [pre][post] { … }` | the machine event (mtvec) | convention scaffold — see below | ordinary state obligations |
| `node <n> [pre][post] { … }` | the reactor | none | ordinary state obligations |

All three are nodes in the type system's eyes; they differ only in who
dispatches them and which ABI the compiler wraps around the body.

## `bootstrap node` — the authored program entry

The program's machine beginning. Without one, the compiler emits the canned
`_start` (set sp, zero .bss, call the reactor) — the fallback. With one, the
author's body replaces the fallback's *policy*: the compiler still emits the
ISA scaffold (sp, .bss — architecture facts, never board facts), then the
typed body, then hands off to the reactor.

```briev
bootstrap node reset [armed == false && ticks == 0] {
    wire_trap_vector();      // kernel shim (typed)
    interrupts_enable();     // kernel shim
    armed = true;            // typed store — proves the handoff contract
};
```

- The handoff postcondition is over **program state at reactor handoff** and
  is proven from the body's typed stores plus the state initializer
  (constant evaluation). Machine behavior of the scaffold is compiler-owned
  ABI — the same trust class as every prologue.
- One bracket group (postcondition) — nothing fires a bootstrap but the
  machine. A `[pre][post]` form is a compile error.
- Placed in `.text.start` (the entry symbol — QEMU virt `-bios none` enters
  at the start of RAM, not `e_entry`). First-declared bootstrap is the
  reset entry.
- Always a liveness root; never inlined; excluded from reactor dispatch and
  the concurrency gate.

## `node @ vector` — machine-serviced events

`@` is Briev's hardware-association delimiter: it already binds MMIO
addresses (`trg` pins, `@0x10000000`) and now binds **interrupt vectors**.
Which namespace (addresses vs interrupts) is board-file scoped.

```briev
node trap_service @ timer_irq [ticks >= 0] { … };
```

- Vector names resolve from the board's `interrupts.dbvl` (`timer_irq = 7`);
  literal numbers are accepted.
- **Mechanism inference**: the active target profile's `isr_mechanism` field
  names the mechanism row (`config/isr-targets.dbvl`). No profile default
  and no explicit row → compile error with the profile key to set; the
  compiler never invents a layout.
- The mechanism's **convention** supplies the entry scaffold:
  - default: the interrupt calling convention (the machine's own partial
    save);
  - `full_context`: save x1–x31 + sp → kernel stack → body → restore →
    `mret` — the preemptive-service sequence.
- Machine-fired: excluded from reactor dispatch and the concurrency gate;
  always a liveness root; ordinary state contracts (delegation obligations
  live on the called txns).

## The ABI layers

| Layer | Owner | Contents |
|---|---|---|
| ISA scaffold | compiler | sp/bss (reset); save-all/restore/`mret` (`full_context`); prologues |
| Frame layout | compiler | `@__briev_trap_frame`: x1–x31, sp, pc — pinned, documented |
| Mechanism selection | target profile | `isr_mechanism` field → registry row |
| Vector names + addresses | board files | `interrupts.dbvl`, `addresses.dbvl` |
| Register access | kernel shim library | `mcause()`, `set_mepc()`, `ecall()`, … — typed one-liners over `Asm#` |
| Kernel policy | kernel `.bv` | dispatch, schedule, syscall table, task table |

The context switch is typed memory: the scheduler writes the next task's
registers into `@__briev_trap_frame` + `mepc`; the scaffold's restore path
resumes whoever the frame names. U-mode entry is the same path — `mepc` +
`mstatus.MPP = U` set in the typed body; the scaffold's `mret` drops
privilege as a CSR side effect.

## The register shim

Kernel logic never sees asm. The shim library (e.g.
`lib/kernel/rv64-machine.bv`) carries the machine facts as typed functions —
the `riscv::register` pattern:

```briev
defn mcause() -> Int     { (Asm#("raw", "csrrs $0, mcause, zero", 0)) & 15; }
defn ecall()             { Asm#("raw", "ecall", 0); }
defn wire_trap_vector()  { Asm#("raw", "la $0, trap_service; csrw mtvec, $0", 0); }
```

Disclosed necessity: `wire_trap_vector` names a symbol inside its template —
the one place asm text must reference a program symbol. Inline-asm
discipline: every register flows through constraint registers (`$0`/`$1`);
a template that touches t0/t1 behind the compiler's back corrupts whatever
LLVM hoisted into them (recorded 2026-09-13, plan
`2026-09-11-rv64-capability-kernel.md` Addendum D).

## Pinned symbol conventions

- `@txn_<name>` — txn wrapper symbols (reactor dispatch + shim `call`/`la`).
- `@__isr_body_<name>` — the typed body behind a machine entry.
- `@__briev_state`, `@__briev_trap_frame` — compiler-owned globals.
- `_stack_top`, `_bss_start`, `_bss_end` — linker-provided (scaffold reads).

These are stable ABI for kernel authors; changes are breaking and must be
documented here.

## What retired, and why

- **`naked`** (proposed, never added): its two jobs — authored reset, trap
  stubs — are subsumed by `bootstrap node` and the mechanism convention.
- **`isr` keyword**: dissolved into `node @ vector` + profile inference; the
  registry remains the ABI layer it always was.
- **`beginprogram` as mechanism**: optional sugar — the bootstrap's handoff
  state makes the first node eligible; `beginprogram` nodes keep working.
- **asm in kernel logic**: replaced by shim + scaffolds; asm survives only as
  compiler-owned ABI text and six typed shim one-liners.
