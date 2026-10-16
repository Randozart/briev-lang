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

**Lifecycle laws (2026-10-15, SPEC §9.4.3).** Equilibrium is forever unless
the program declares otherwise. `when G { endprogram; };` lowers to a
reactor monitor (a synthesized `__briev_lifecycle_N` txn) that ends the
process when `G` holds; `when G { reside; };` declares open-ended residency
at compile time. A program with no reachable terminator and no reside law
warns — stalling stays legal, the warning names the fix.

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

> **2026-10-16 (term-semantics plan).** The checkpoint is P-gated in both
> directions: postcondition MET ⇒ the firing stops here (the txn commits,
> the node stops firing); postcondition UNMET ⇒ control CONTINUES past
> the `term` into the rest of the body (the same convergence loop that a
> body without `term` takes). Bare `term;` and value `term <e>;` are both
> P-gated in txn/node bodies; in plain defns `term <e>;` is an
> unconditional return and a final bare expression (a tail) is the
> body's implicit value (SPEC §9.1). The reference interpreter implements
> the identical rule (`interpreter::eval::pgated_term`); the backend's
> P-gated emission landed with plan step 4, tails with step 5.

> **2026-10-16 (cpp-vs-briev findings).** Multi-node handoff via a shared
> phase field: the compiler's front door now settles every shape a
> contributor hit in the LLVM-17 era. A `beginprogram` (or any convergent)
> node whose goal the body does not counter-advance toward is rejected at
> compile time with the fix in the message; a reactive cycle with no
> completion in any postcondition is rejected as non-convergent ("state
> the completion in the nodes' postconditions … or make the exit
> condition explicit in a pre"). Plain convergent counter nodes hand off
> correctly without any keyword. For per-tick parallel-LOOKING nodes the
> classification demand (Rule 22) stands: mark the pair `async` (the
> `benchmarks/nbody_newton_accel.bv` distinct-phase-gate pattern — also
> the shape of `benchmarks/recommender.bv`) or `sync<g>`. The
> `__wait_for_trigger__` runtime panic reported against the LLVM-17-era
> compiler no longer reproduces; the liveness analysis rejects those
> shapes before emission. Probe programs: `benchmarks/recommender_corpus/`
> reconstruction notes and the 2026-10-16 results file.

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

## Reactor Dirty Lifecycle

The interpreter reactor fires nodes driven by a **dirty precondition set**, not
a global re-scan:

- `build_from_program` marks every reactive txn dirty — the **bootstrap
  wake**: every node's precondition is evaluated at least once at program
  start.
- `run()` iterates the dirty set: for each txn, evaluate its precondition;
  if true, fire the body (convergence loop via `term` checkpoints); if false,
  skip. `run()` does NOT clear the set.
- The driver clears between rounds. The correct lifecycle is
  `run() → clear_dirty() → run() → …`, or no clear at all for a single-shot
  program (the bootstrap set is consumed by the first `run()`).
- `dependency_map` is currently never populated — `mark_dirty` is a no-op.
  Re-firing after a state mutation relies on the pre-condition being
  re-evaluated in the next round, not on a dirty propagation.

**Trap:** a driver that calls `clear_dirty()` *before* the first `run()`
wipes the bootstrap set and can never fire any node (`run_reactor`,
reactor.rs:428, has exactly this bug — see BUGS.md). `run_reactor_continuous`
(reactor.rs:452, no pre-clear) is the working driver.

## Codegen Dispatch

The compiler generates one of two codegen paths (see `a006-dispatch.md`):

| Path | When used | Generated code |
|------|-----------|----------------|
| **Direct SSA** | No async/MMIO triggers | Inline all txn bodies in `main()`, phi-based loop |
| **Reactor tick** | Async triggers or MMIO | `@reactor_tick()` function + `main()` loop |

Both paths implement the same reactor semantics. The Direct SSA path is an
optimization for simple programs — it avoids the `@reactor_tick` indirection.

## Embedded: Equilibrium as Idle — frontier-driven (2026-09-14)

On bare-metal targets (riscv64, thumb), the reactor loop IS the idle loop —
but what "idle" means is a **static per-program decision**, computed from the
program's eligibility frontier (what can make a node eligible and how):

| Program frontier | Equilibrium | Re-check on wake |
|---|---|---|
| state-sequenced only | direct fallthrough — provably-next nodes chain without re-checks | none |
| vectored only (`@ vector` nodes) | `wfi` park | the trap's dependent preconditions only |
| external frontier (`@ address` wires) | continuous evaluation (spin) — or park with a timer quantum under a declared `within` bound | the external set |

The wake-dependency analysis (per wake source: typed writer sets ×
precondition reader sets, conservative) computes the re-evaluation sets;
omitted checks are proven unable to change. Machine entries are declared via
`bootstrap node` (authored) or the canned `_start` (fallback), and machine
events via `node @ vector` (mechanism-inferred scaffold) — see
`machine-entry.md`. No explicit interrupt handler is needed: the hardware
writes state (or the machine vectors), and the reactor's evaluation of the
affected frontier IS the dispatch.

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
