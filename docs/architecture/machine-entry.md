# Machine Entry — bootstrap, `@` wiring, and the ABI layers

**2026-09-14.** How Briev programs meet the machine: who enters where, who
owns which register protocol, and where each piece of machine knowledge
lives. Companion to `briev-execution-model.md` (the reactor) and the plan
`2026-09-14-bootstrap-kernel.md` (+ Addendum A).

One sentence: **entries are declared, machines wire them, compilers own ABI,
users own policy.** The DSL guardrails doctrine — the test for whether
machine-entry constructs stay general, and the temptations rejected en
route — lives in `briev-capability-frontier.md` ("The DSL guardrails").

## The entry classes

All of these are nodes in the type system's eyes; they differ only in who
dispatches them and which ABI the compiler wraps around the body.

| Form | Fired by | Scaffold (compiler-owned) | Contracts |
|---|---|---|---|
| `bootstrap node <n> [<post>] { … }` | the reset vector | `sp ← _stack_top`, `.bss` zero, body, hand off to the reactor | handoff postcondition, proven from body stores |
| `node <n> @ <vector> [pre][post] { … }` | the machine event (mtvec) | convention scaffold — see below | ordinary state obligations |
| `node <n> @ <address> [post]? { … }` | the reactor's pass | none (existing trigger machinery) | postcondition; the wiring is the eligibility |
| `node <n> [pre][post] { … }` | the reactor | none | ordinary state obligations |

## `@` wiring — one pattern, two dispatch classes

`@` is Briev's hardware-association delimiter: it binds MMIO addresses
(`trg` pins, `@0x10000000`), and on a node header it binds the node to
hardware. **The board-file namespace of what follows decides the dispatch
class:**

- **interrupts namespace** (`@ timer_irq`, `@ 7` — from
  `interrupts.dbvl`): machine-vectored. The machine preempts and enters the
  node through the mechanism's convention scaffold; it is never
  reactor-dispatched. Latency is the hardware's — these nodes take no
  `within` bound.
- **addresses namespace** (`@ thermal_alert` from `addresses.dbvl`,
  `@ 0x0200BFF8`) or a pointer (`@ *rx_ready`): reactor-pass. The node stays
  in reactor dispatch, gated by the wiring (the existing trigger machinery —
  an inline trigger). Dynamic pointers are always this class (mtvec needs a
  static handler).

A wired node may omit `[pre]` — the wiring is the eligibility. Ambiguous
names (present in both namespaces) are a compile error naming both.

### Latency contracts

A memory-mapped value changing does not notify the CPU; only interrupt lines
do. So the two classes differ in wake source, and the latency story is
honest per class:

- vectored: fires at interrupt latency — the machine's, not ours to bound.
- reactor-pass: fires **within one dispatch pass** while the reactor runs.
  At equilibrium the compiler's park policy is frontier-driven (below); a
  declared bound moves the tradeoff into the open:

```briev
node overtemp @ thermal_alert { … }                // default: fires within one pass
node overtemp @ thermal_alert within 1 ms { … }    // bound: compiler picks spin or park+quantum
```

`within` reuses the watchdog deadline syntax. The default needs no keyword
(Rule 2): determinism is the default; power saving is the declared,
checked tradeoff.

## `bootstrap node` — the authored program entry

The program's machine beginning. Without one, the compiler emits the canned
`_start` (set sp, zero .bss, call the reactor) — the fallback. With one, the
author's typed body runs at the head of main (after the state initializer,
before the first dispatch pass); the compiler still emits the ISA scaffold
(sp, .bss — architecture facts, never board facts).

```briev
bootstrap node reset [armed == true && ticks == 0] {
    wire_trap_vector();      // kernel shim (typed)
    interrupts_enable();     // kernel shim
    armed = true;            // typed store — proves the handoff contract
};
```

- **The handoff postcondition describes the state AFTER the body** — at
  reactor handoff — and is proven from the body's typed stores plus the
  state initializer (constant evaluation; the reactive convergence checker's
  existing reachability proof). Machine behavior of the scaffold is
  compiler-owned ABI — the same trust class as every prologue.
- One bracket group (postcondition) — nothing fires a bootstrap but the
  machine. A `[pre][post]` form is a compile error. No brackets = no
  obligation (allowed); a written `[true]` asserts nothing and is rejected.
- Placed in `.text.start` (the entry symbol — QEMU virt `-bios none` enters
  at the start of RAM, not `e_entry`). First-declared bootstrap is the
  reset entry.
- Always a liveness root; never inlined; excluded from reactor dispatch and
  the concurrency gate. Currently requires the direct-SSA dispatch (other
  paths reject with the fix).

## The ABI layers

| Layer | Owner | Contents |
|---|---|---|
| ISA scaffold | compiler | sp/bss (reset); save-all/restore/`mret` (`full_context`); prologues |
| Frame layout | compiler | `@__briev_trap_frame`: x1–x31, sp, pc — pinned, documented |
| Mechanism selection | target profile | `isr_mechanism` field → registry row |
| Vector + address names | board files | `interrupts.dbvl`, `addresses.dbvl` |
| Register access | kernel shim library | `mcause()`, `set_mepc()`, `ecall()`, … — typed one-liners over `Asm#` |
| Kernel policy | kernel `.bv` | dispatch, schedule, syscall table, task table |

The context switch is typed memory: the scheduler writes the next task's
registers into `@__briev_trap_frame` + `mepc`; the scaffold's restore path
resumes whoever the frame names. U-mode entry is the same path — `mepc` +
`mstatus.MPP = U` set in the typed body; the scaffold's `mret` drops
privilege as a CSR side effect.

## Machine-entry node bodies — straight-line, never convergence

A `node @ vector` body runs **once per event** — the event is the
iteration. Machine-serviced bodies emit straight-line + return: **no
convergence loop, no post-check at the tail** — the postcondition is a
documented obligation (checked by the derivation examples and the test
gate), never a runtime re-run loop. A handler that "loops until post"
would spin at interrupt priority with the event's own cause still
latched (found in the Phase 4 kernel: the trap_service body with
brackets looped forever — `csrw mepc` alternating task entries, no
`mret`). The same rule now governs `defn`: **a defn with contract
brackets must not compile as a convergence loop** (logged BUGS.md
2026-09-14 — the kernel's `defn schedule() [pre][post]` re-ran its
switch forever; the demo drops the brackets until the emitter is
fixed).

## The scheduler pattern

Phase 4's kernel uses **restart scheduling**: each switch writes the
next task's stack (frame slot 8) and entry (live `mepc`) and mrets —
the task re-runs from its top, sound for stateless slice bodies.
**Resume scheduling** (keeping the task's interrupted pc and full
register set in its context area, copied back on switch) rides the same
frame rewrite — the ctx_save/ctx_restore helpers in the demo are the
seed. Both are kernel policy; the scaffold is identical. (2026-09-14,
plan `2026-09-14-rv64-finish.md`: resume scheduling is DEFERRED until the
language has a loop construct — the parked task bodies make restart and
resume observably identical.)

## Second-architecture proof (2026-09-14)

The same mechanism-inference chain boots a COMPLETELY different ISA with
zero Briev-level changes. QEMU MPS2-AN385 (Cortex-M3) runs hello world and
a SysTick timer via the identical `bootstrap node` / `node @ vector`
pattern (plan `2026-09-14-rv64-finish.md` Phase 5). The target row
(`config/targets.dbvl`: `target.thumbv7m` → `arm_cortex_m` mechanism) is
all that differs. Board data carries what the compiler must not know:

- `lib/boards/mps2-an385/startup.S` — hardware boot table (SP + Reset) and
  the canonical bare-metal init: copy `.data` from its load address and
  zero `.bss`. Briev globals like `briev_begin_boot` live in `.data`;
  without the copy, RAM-starting-at-zero reads the flag as false and the
  reactor never boots. `Default_Handler` is defined WEAK here (the
  compiler emits its strong spin-loop only for programs that declare ISR
  handlers).
- `lib/targets/qemu-mps2-an385.ld` — code in ZBT SSRAM1 at 0x0 (Cortex-M
  boots by reading the vector table there), data/stack in RAM.
- `lib/runtime/compiler_rt_arm.{c,S}` — AEABI division shims (see below).

ISA differences the pattern absorbs:

- **Vector model**: RISC-V has one `mtvec` handler reading `mcause`; Cortex-M
  has a hardware vector table. The `@ N` node resolves through the target's
  mechanism row, which supplies the table layout (entry stride, SP slot,
  Thumb bit) — the compiler emits the table, boot patches the SysTick slot.
- **Bare-metal entry**: RISC-V `_start` sets `sp`, zeroes `.bss`, calls
  `main`; Cortex-M vector table sets SP and calls Reset_Handler, which
  does the `.data` copy / `.bss` zero then branches to the same `_start`.
- **MMIO width**: `Int` is the abstract 64-bit register; on a 32-bit bus
  the register is `Ptr<Bit<32>>` (a SysTick CTRL i64 store clobbers the
  adjacent LOAD register). The volatile intrinsics width-adapt to the
  pointee.
- **Compiler-rt ABI**: LLVM emits `__aeabi_ldivmod` for 64-bit division on
  Cortex-M3 (no hardware divider). The AEABI return convention (quotient
  + remainder in r0–r3) is not expressible in C (AAPCS uses sret for a
  >4-byte composite), so the entries are assembly. The core-call
  convention (n in r2:r3, d as a full 8-byte stack slot, r1 free) was
  derived from the compiled C core under qemu-arm, not assumed.

## The register shim

Kernel logic never sees asm. The shim library (e.g.
`lib/kernel/rv64-machine.bv`) carries the machine facts as typed functions —
the `riscv::register` pattern:

```briev
defn trap_cause() -> Int { (Asm#("raw", "csrrs $0, mcause, zero", 0)) & 15; }
defn ecall()             { Asm#("raw", "ecall", 0); }
defn wire_trap_vector()  { Asm#("raw", "la $0, trap_service; csrw mtvec, $0", 0); }
```

Disclosed necessity: `wire_trap_vector` names a symbol inside its template —
the one place asm text must reference a program symbol. Inline-asm
discipline: every register flows through constraint registers (`$0`/`$1`);
a template that touches t0/t1 behind the compiler's back corrupts whatever
LLVM hoisted into them (recorded 2026-09-13, plan
`2026-09-11-rv64-capability-kernel.md` Addendum D).

## Frontier-driven equilibrium (wake sets)

The reactor's equilibrium behavior is a **static per-program decision**, not
a runtime policy. The frontend computes per wake source the
*re-evaluation set* — the preconditions that reference fields that source
can affect — from typed writer sets (the `build_write_masks` precedent) ×
precondition reader sets, stored in `AnalysisResults.wake_sets`.
Conservative by construction: a field written anywhere live is may-written
for any wake reaching that writer; externally-wired fields are permanently
in the external frontier. The emitter asserts its park policy against the
computed frontier — under-approximation fails loudly at compile time.

| Program frontier | Equilibrium | Re-check on wake |
|---|---|---|
| state-sequenced only | direct fallthrough — fold machinery chains provably-next nodes | none |
| vectored only | `wfi` park | the trap's dependent set only |
| external frontier | spin (default), or park+quantum under a `within` bound | the external set + state fallthrough |

Everything the compiler proves cannot change is not re-checked: omitted
checks are proven dead, so observable behavior is identical to full
evaluation.

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
  state makes the first node eligible; `beginprogram` nodes keep working and
  remain the idiomatic native-target form.
- **asm in kernel logic**: replaced by shim + scaffolds; asm survives only as
  compiler-owned ABI text and six typed shim one-liners.
- **`trg` as the only trigger form**: coexists with inline-`@` wiring (named
  reusable binding vs point-of-use).
