# Briev Execution Model

**2026-09-13:** The core insight: Briev programs are reactive pseudo-loops,
not imperative `main()` functions. The reactor IS the scheduler, the idle
loop, and the task dispatcher. There is no traditional kernel overhead.

## The Reactor Is the Pseudo-Loop

A Briev program does not have a `main()` function. The compiler generates a
**reactor** — a loop that continuously evaluates node preconditions. When
no node can fire, the program is at **equilibrium** (idle). When a
precondition becomes true (hardware interrupt, sensor read, timer tick), the
reactor fires that node.

```
┌─────────────────────────────────────────────┐
│  Reactor Loop                               │
│  ┌───────────────────────────────────────┐  │
│  │ Evaluate all node preconditions       │  │
│  │   ├─ Some true?  → fire first one     │  │
│  │   │                (body runs)         │  │
│  │   │                (check postcondition)│  │
│  │   │                (loop or converge)  │  │
│  │   └─ None true?  → equilibrium        │  │
│  │                      (idle / wfi)      │  │
│  └───────────────────────────────────────┘  │
└─────────────────────────────────────────────┘
```

This eliminates the overhead of a traditional kernel:
- No context switching — nodes fire sequentially (or `async` explicitly)
- No scheduler — the reactor IS the scheduler
- No poll loop — preconditions encode the scheduling logic
- No interrupt handler — hardware writes to volatile registers, node
  preconditions read them

## No `main()` — There Is No Main

**SPEC §11.5:** "There is no `main` declaration in Briev. The program entry
is either an explicit `beginprogram` node (§11.5.1) or, absent one, whichever
reactive node fires first."

This is the primary source of confusion for developers coming from C/Rust.
Briev programs execute via the reactor, not via a called function. The
compiler generates a `main()` function in the IR, but it IS the reactor — it
evaluates node preconditions and fires the first satisfiable one.

## `beginprogram` — Entry Sugar

`beginprogram` is a keyword usable as a conjunct in a node's precondition:

```briev
node entry [beginprogram][true] {
    // fires once at program start
    halt;
};
```

Without `beginprogram`, the equivalent is:

```briev
let started: Bool = true;

node entry [started][true] {
    // fires once at program start
    halt;
};
```

`beginprogram` is **pure sugar** — it eliminates the need for a bootstrapping
variable. The compiler desugars it to a per-node entry flag
(`@briev_begin_<name>`) that is true exactly once at program start.

### Entry loop rules

- Entered exactly once at program start when state conditions hold
- Precondition evaluated once, never re-checked during the loop
- Body runs repeatedly until postcondition (goal) is met
- Goal must be provably reachable — `[true]` = single pass
- At most one `beginprogram` node eligible at start — compiler proves
  mutual exclusion

## Nodes — Reactive Transitions

A `node` is a reactive transition that fires when its precondition is
satisfied (SPEC §9.4):

```briev
node update [pending][!pending] {
    pending = false;
    term;
};
```

- **Precondition** = eligibility (when can this fire?)
- **Postcondition** = completion (when is this done?)
- **No parameters, no return value** — nodes read/write state directly
- **Atomic ticks** — all RHS read from pre-tick state (SPEC §9.3)
- **Convergence** — body loops until postcondition is satisfied

## Convergence — The Contract Drives Termination

There is no `while(true)`, no `break`, no `loop`. The contract drives
termination:

```
[pre]  { body }  [post]  →  [pre]?
                              ├─ yes → body → post → ...
                              └─ no  → exit (converged)
```

`term` inside the body is a convergence checkpoint — the reactor evaluates
the postcondition at this point (SPEC §11.3):

```briev
node tick [count < total][count == total] {
    count = count + 1;
    term;   // reactor checks: is count == total? if yes, converge.
};
```

Without `term`, the reactor has no point at which to evaluate the goal.
`term` is mandatory in looping bodies.

### Postcondition semantics

| Postcondition | Behavior |
|---------------|----------|
| `[true]` | Single pass — body runs once, then converges |
| `[count == total]` | Loops until count reaches total |
| `[false]` | Never converges — infinite loop (compiler warns) |

## Three Constructs

| Construct | Calling convention | State mutation | Convergence loop |
|-----------|-------------------|----------------|-----------------|
| `node` | Auto-fired by reactor | Yes | Yes |
| `txn` | Explicit call | Yes | Yes |
| `defn` | Explicit call | **No** (pure) | No |

`node` and `txn` are semantically identical (same atomicity, same convergence
loop). The only difference is calling convention — `node` waits for its
precondition, `txn` waits for a caller. See `txn-semantics.md` for the full
treatment.

## Codegen Dispatch

The compiler generates one of two codegen paths (see `a006-dispatch.md`):

| Path | When used | Generated code |
|------|-----------|----------------|
| **Direct SSA** | No async/MMIO triggers | Inline all txn bodies in `main()`, phi-based loop |
| **Reactor tick** | Async triggers or MMIO | `@reactor_tick()` function + `main()` loop |

Both paths implement the same reactor semantics. The Direct SSA path is an
optimization for simple programs — it avoids the `@reactor_tick` indirection.

## Embedded: Equilibrium as Idle

On bare-metal targets (riscv64, thumb), the reactor loop IS the idle loop.
When no node can fire:

1. The reactor evaluates all preconditions → all false
2. The program is at equilibrium
3. On embedded: `wfi` (wait-for-interrupt) halts the CPU until hardware
   wakes it
4. Hardware interrupt fires → sets a volatile flag → node precondition
   becomes true → reactor fires that node

No explicit interrupt handler is needed (though `isr<name>` can declare one).
The hardware writes to a memory-mapped register; the node's precondition reads
it via `VolatileLoad#`. The reactor's continuous evaluation IS the interrupt
dispatch.

## Related Documents

| Document | Covers |
|----------|--------|
| `txn-semantics.md` | Atomicity, convergence, reactive vs callable, parallelism |
| `concurrency-and-modifiers.md` | Concurrency gate, sync groups, async keyword |
| `a006-dispatch.md` | Direct SSA vs reactor_tick codegen paths |
| `glossary.md` | Reactor, DirectSSA, ReactorTick definitions |
| `plans/2026-08-06-endprogram-beginprogram.md` | beginprogram design decisions |
| SPEC §9.4 | Node definition, eligibility/completion |
| SPEC §11.3 | `term` as convergence checkpoint |
| SPEC §11.5/11.5.1 | Program entry, beginprogram entry loops |
