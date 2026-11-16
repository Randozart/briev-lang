# Thread Assignment — Explicit CPU Pinning for Briev Nodes

**2026-11-15.** Status: **Stage 1 landed; Stages 2-5 are the recorded follow-up.**

Stage 1 (parse + validate + prove + plan + diagnose) is implemented and tested.
The emission half is deliberately held: the plan is proved and carried, but
`on thread <N>` is **refused at emission** because running worker slices
sequentially would reorder node firing, and firing order is observable. The
refusal names the declared plan and the missing piece. Section 9 records exactly
what the threaded dispatch lane must build, with the findings that de-risk it.

This is a design plan for explicit thread/worker assignment on reactive nodes,
so an author can control CPU load distribution (cache locality, NUMA, RT
isolation) while unmarked nodes keep the reactor's existing equilibrium
scheduling.

**Companions:** `keyword-taxonomy.md` (three keyword categories, ambiguity vs
strategy), `briev-execution-model.md` (reactor IS the scheduler, equilibrium),
`derivation-not-recognition.md` (compiler obligation), `gpu-syntax-decision-record.md`
(the keep/drop/defer pattern), `docs/plans/2026-09-30-hardware-manipulation-expressiveness.md`
(non-negotiables: derive the default, disclose escapes, no speed keyword).
**Feeds:** Golden Rules 2, 3, 22.

---

## 1. The problem

The reactor's existing dispatch (`src/backend/llvm/dispatch.rs`) emits a
`reactor_tick` function where **all** reactive txns fire in a single loop
per tick. Conflict detection (`fusable_pairs`) decides which txns are
simultaneous and which serialize; `sync<group>` gates groups behind a
barrier; `async` marks pairs that may fire concurrently (the proof engine
requires this classification).

There is **no mechanism** to say "fire node X on core 0, node Y on core 1".
This means:

- **No CPU affinity control.** The OS scheduler decides which thread runs
  where; on NUMA or multi-socket systems that can mean cache-line ping-pong
  or cross-socket latency.
- **No RT isolation.** Real-time critical nodes can't be pinned to a
  dedicated core.
- **No load pinning for the reactor runtime itself.** Even though the
  reactor is single-threaded, when it runs inside a host (GLUE FFI) the
  host thread's affinity matters.

## 2. Design: three layers of control

### Layer 1: `on thread <n>` — explicit pinning (NEW)

```briev
node counter on thread 0 { i = i + 1; term; };
node logger on thread 1 { Print#(i); term; };
```

A node annotated `on thread <N>` has its body **emitted into a separate
worker thread function** and the reactor spawns a thread pool at startup.
The thread index is compile-time-determined (a literal integer or `any`
for compiler choice).

**Semantics:**
- `on thread 0` → body lives in `@reactor_tick_0`, a separate LLVM function
  with its own state snapshot and arena. The reactor dispatches it on
  thread 0.
- `on thread any` → the compiler's default (equilibrium scheduling); no
  thread annotation needed.
- `on thread <N>` with N ≥ thread_count → compile error.

**Effect on conflict detection:**
- Nodes on the same thread serialize within that thread's `reactor_tick_N`.
  Conflict detection still runs (fusable_pairs is per-thread), but nodes
  on different threads don't conflict — the barrier between threads
  guarantees isolation.
- Cross-thread reads/writes are explicit (FFI or shared state with
  proven isolation). The proof engine must verify cross-thread dataflow.

### Layer 2: `async` + pinning — multi-core reactor

```briev
async node compute_a on thread 0 { ... };
async node compute_b on thread 1 { ... };
```

`async` already declares "these pairs may fire simultaneously." When
combined with `on thread <N>`, the reactor creates N worker threads, each
running its assigned nodes. The main thread handles equilibrium park and
the `wfi` loop.

**Derivation:** if all async-eligible nodes are pinned, the compiler knows
exactly which txns share cores — no implicit runtime thread pool guesswork.

### Layer 3: `queue <name>` — work distribution (optional, stdlib-first)

Not a new keyword. The queue is expressible as a shared `List<T>` (or ring)
with `sync<group>` on the producer/consumer:

```briev
let work: List<Job> = [];

node producer [ready][work.len > 0] {
    work <- make_job();
    // The push is the signal
};

async node worker on thread 0 [work.len > 0][done] {
    let j = work[0];
    work <- drop_front();
    process(j);
    term;
};
```

A named `queue` keyword would be a stdlib convenience macro, not a reactor
primitive. The compiler only needs to know: "these nodes share a List" +
"these are async" + "these are on different threads."

## 3. Keyword classification

Per `keyword-taxonomy.md`:

| Keyword | Category | Rationale |
|---------|----------|-----------|
| `on thread <N>` | **Ambiguity keyword** | Compiler cannot rank which core is "best" for a node without hardware topology; the pick depends on the author's intent (RT, NUMA, cache) |
| `on thread any` | **Ambiguity keyword (default)** | Compiler's default — equilibrium scheduling |
| `queue <name>` | **Ambiguity keyword** (if added) | Who draws from the shared list — producer-driven vs consumer-pulled |

`on thread <N>` is **never a strategy keyword.** It does not constrain
*how* a node fires; it constrains *where* it fires. Speed differences are
consequences, not intent. The observability razor is satisfied: removing
`on thread` changes observable cache/NUMA behavior (cache hits/misses are
observable in wall-clock time).

## 4. Parser surface

The existing `ModifierPrefix` struct (`src/parser/helpers.rs:16`) already
holds `annotations`, `is_async`, and `sync_groups`. Thread pinning adds one
field:

```rust
pub struct ModifierPrefix {
    pub annotations: Vec<Annotation>,
    pub is_async: bool,
    pub sync_groups: Option<Vec<String>>,
    pub thread_assign: Option<ThreadAssign>,  // NEW
}

pub enum ThreadAssign {
    Thread(usize),        // `on thread 0`
    Any,                  // `on thread any`
}
```

The modifier scanner (`consume_modifier_prefix`, helpers.rs:193) gains a
new case:

```rust
Some(Token::Identifier(word)) if word == "on" => {
    self.pos += 1;
    self.expect_identifier("thread")?;
    if self.check(&Token::Identifier("any")) {
        self.pos += 1;
        prefix.thread_assign = Some(ThreadAssign::Any);
    } else {
        let n = self.parse_integer()?;
        prefix.thread_assign = Some(ThreadAssign::Thread(n));
    }
}
```

This slots naturally among existing prefix modifiers (`seq`, `async`,
`sync<g>`, `approx`, `exact`). Composition rules:
- `on thread <N> async node X` — valid (pin + concurrency classification)
- `on thread <N> seq node X` — valid (order-pinned + specific core)
- `async node X on thread <N>` — **error** (pin must be prefix, consistent
  with `async node` prefix-only rule from 2026-09-22)

## 5. Analysis pipeline changes

### 5.1 Concurrency gate (`src/analysis/concurrency_gate.rs`)

The gate currently classifies pairs by `sync_groups` and `is_async`.
Thread pinning adds another axis: **same-thread = serial within that thread,
cross-thread = already serialized (barrier-free, different cores).**

The gate must be extended to:
1. Index nodes by `thread_assign` from their `Transaction` modifiers.
2. For each eligible async pair:
   - Same thread → serial in the same `reactor_tick_N` → check `fusable_pairs`
     as normal.
   - Different threads → already safe (different cores) → no gate needed.
   - `any` or unassigned → same as today (error if unclassified eligible).

### 5.2 Transition graph (`src/analysis/transition_graph.rs`)

Nodes with `thread_assign = Thread(N)` are emitted as a separate entry in
the transition graph with their own `fusable_pairs` subset. The graph
already handles `sync<group>` nodes separately; thread-assigned nodes
follow the same pattern.

### 5.3 `AnalysisResults` (`src/backend/mod.rs`)

Add:

```rust
/// 2026-11-15 (thread-assignment plan): node → thread index map.
/// Empty = equilibrium scheduling (no pinning). Non-empty = thread pool
/// dispatch. Consumed by the LLVM backend to emit per-thread functions.
pub thread_assignments: HashMap<String, ThreadAssign>,
```

## 6. LLVM backend changes

### 6.1 Per-thread `reactor_tick_N` functions

Currently `dispatch.rs` emits a single `@reactor_tick` function. With
thread assignment, the backend emits:

```llvm
define void @reactor_tick_main(ptr %state) { ... }   // main thread: wfi, equilibrium
define void @reactor_tick_0(ptr %state) { ... }      // thread 0 nodes
define void @reactor_tick_1(ptr %state) { ... }      // thread 1 nodes
```

Each `reactor_tick_N` is a separate LLVM function with its own state,
arena, and dispatch logic for the txns assigned to that thread.

### 6.2 Thread pool initialization

The `main` function gains a small prologue:

```c
// Inside main():
pthread_t threads[NUM_THREADS];
for (i = 0; i < NUM_THREADS; i++) {
    pthread_create(&threads[i], NULL, reactor_tick_worker_N, state);
}
// main thread runs reactor_tick_main (wfi loop)
// threads[i] runs reactor_tick_i
```

Or, more efficiently, inline the pthread_create into the LLVM IR with
`pthread_create` FFI calls. The state pointer is shared across threads;
each thread reads a different slice of the conflict-detection table.

### 6.3 Cross-thread state access

If thread N reads/writes state fields owned by thread M, the compiler
must:
1. Emit a memory barrier (LLVM `memoryscope` / LLVM atomic fence) at the
   boundary.
2. Flag the access in diagnostics (cross-thread state read is sound iff
   the field is `async`-classified on both threads).

For most programs, cross-thread access is unnecessary — pinned nodes
operate on disjoint state fields. If a node reads from another thread's
state, it becomes an FFI boundary (the other thread's `reactor_tick_N`
return value or a shared volatile field).

## 7. Implementation plan (staged)

### Stage 0 — DONE (2026-11-15): parser + AST

Landed shape (the plan's original `ThreadAssign` field on `ModifierPrefix` was
not needed — an `Annotation` with a value already carries the payload through
the existing modifier plumbing, so no `Transaction` struct churn):

- `on thread <N>` / `on thread any` parsed in `consume_modifier_prefix`
  (`src/parser/helpers.rs`) as a `thread` annotation — value
  `Some(Expr::Decimal(n))` for a pin, `None` for `any`.
- `on`/`thread` stay **contextual identifiers** (no lexer tokens); the arm
  fires only when `on` is directly followed by `thread`, and
  `starts_modifier_run` mirrors that lookahead.
- Prefix-only (like `async`); duplicate pins and non-integer/`any` payloads are
  syntax errors; a pin on a `let`/`defn` is rejected ("has no firing site").
- Canonical printing round-trips both forms (`thread_pin_prefix` in
  `briev-verify/src/ast/canonical.rs`).
- Typed HIR carries it: `HirCallable::thread_pin: Option<ThreadPin>`
  (`Index(u32)` / `Any`), lowered from the annotation list.
- Tests: `parser::definitions::tests::thread_pin_*` (5) + contextual-name test.

### Stage 1 — DONE (2026-11-15): analysis gate + plan

`src/analysis/thread_plan.rs`:

- `ThreadPlan { assignments, workers, worker_count, explicit_any }` — the
  per-worker assignment, deterministic (BTreeMap, sorted lists).
- **Cross-worker soundness**: two nodes on different workers may fire in the
  same tick, so the gate refuses when their guards are jointly satisfiable and
  (a) both write a field, or (b) one writes what the other reads. Mutually
  exclusive guards are safe (the existing `check_satisfiable`).
- **`async` requirement**: a jointly-satisfiable cross-worker pair must be
  `async` on both, else refused (Rule 22 stays the authority for WHETHER).
- Advisory warnings: a pin that leaves everything on worker 0 (no parallelism
  gained), `on thread any` mixed with real pins, a pin on a non-reactive `txn`.
- Worker index above `MAX_THREADS` (63) is refused.
- Wired into `compile.rs` after the concurrency gate; plan rides in
  `AnalysisResults::thread_plan` (frontend-driven dispatch).
- Tests: 10 unit tests in the module + 2 `tests/compile_fail` fixtures
  (`thread_pin_race.bv`, `thread_pin_unemitted.bv`).

### Stage 1b — the emission hold (2026-11-15)

Pins are **refused at emission** with a diagnostic that names the proved plan
and the missing lane (`unemitted_pin_message`). Rationale: a pin says WHERE a
node fires; serving it sequentially would reorder firing, and firing order is
observable. The only honest outcomes are real workers or a refusal — silently
reordering and silently ignoring are both forbidden.

### Stage 2 — TODO: threaded dispatch lane

What the investigation established (all verified in-tree):

1. **There is no thread pool today.** The pthread pool was removed 2026-09-10
   ("Family H"): `emit_async_phase` now emits async bodies as *direct
   sequential calls in deterministic order*, and `emit_thread_pool_metadata` is
   a documented no-op. `lib/runtime/briev_rt.c` (309 lines) includes
   `<pthread.h>` but has no thread or barrier implementation left.
2. **Four dispatch paths must each be considered.** In `src/backend/llvm/mod.rs`
   the order is: enum-trigger dispatch → library/shared-lib → **direct SSA loop**
   (`emit_ssa_main`, what simple programs get) → reactor loop
   (`emit_reactor` / `emit_parallel_reactor`) → pure-kernel. A pinned program
   must funnel into one known path; the direct SSA loop inlines every node into
   `@main`, so threading it means one loop per worker.
3. **`emit_reactor` is nearly parameterizable**: exactly one hardcoded
   `define void @reactor_tick(...)` site, so a `fn_name` parameter plus a
   filtered txn slice yields `@reactor_tick_w<N>` cheaply.
4. **The arena is the real blocker.** `emit_arena_init` reads/writes arena
   cursor fields **in `%State`** (`arena_ptr_idx`/`arena_end_idx`/`arena_base_idx`)
   — one shared cursor per program. Two workers that allocate would corrupt each
   other. The threaded lane needs per-worker arena triples (state field
   injection × N) or a proof that pinned nodes never allocate.
5. **Tick protocol** (mirrors the removed pool and the C reference benchmark):
   two `pthread_barrier_t`s — main releases (`b_start`), every worker runs its
   tick, all meet at `b_done`; main participates as worker 0. Barriers give the
   memory ordering, so no explicit fences are needed per tick.
6. **Exit**: `endprogram` lowers to `call i64 @exit_prog(...)`, so a worker
   terminating the process ends all workers; the `.end` path additionally needs
   `briev_workers_stop()` + join.
7. **Linking**: `pthread_create` is in glibc ≥ 2.34 (no `-lpthread` needed on
   modern Linux), but the harness link line must be checked on the oldest
   supported target before this ships.

Order of work: (a) per-worker arena triples, (b) `emit_reactor` `fn_name`
parameter + per-worker txn slices, (c) funnel pinned programs into the reactor
path, (d) `briev_workers_start/stop` in the runtime + threaded `emit_main`
variant, (e) remove the emission hold, (f) determinism gate + IR A/B on an
unpinned corpus to prove the default path is untouched.

### Stage 3 — TODO: tests for the threaded lane

- `tests/fixtures/thread_pin_smoke.bv` (restore the deleted positive fixture:
two workers, disjoint counters, an unpinned reporter) once emission lands.
- A cross-worker race fixture proving the refusal (already landed as
  `tests/compile_fail/thread_pin_race.bv`).
- IR regression: `@reactor_tick_w1` defined, main calls the start/stop pair,
  and no worker function contains a shared arena cursor write.
- Optional: `lib/std/queue.bv` (shared ring + `sync<group>`) — a stdlib pattern,
  not a reactor keyword (see §8).

**Remaining estimate: ~6-8 days** (arena work dominates).

## 8. Trade-offs and alternatives

### Why not `sync<group>` for threading?

`sync<group>` is a **barrier** — nodes within the same group fire
simultaneously and hold off finishing until all fired members complete.
It is about **synchronization**, not **distribution**. Two async nodes on
different threads don't need a barrier; they're already serialized by
hardware. Forcing them into a `sync<group>` would add unnecessary
serialization overhead.

### Why not multi-threaded reactor at all?

The current single-threaded reactor is fast — the conflict-detection loop
is tight, cache-local, and the OS scheduler handles thread migration for
the one reactor thread. Multi-threading is **optional** (only when
`on thread <N>` is used) and the default is unchanged. This preserves
the fast path for 99% of programs while giving power users the pinning
they need.

### Why prefix-only (like `async`)?

The existing rule from 2026-09-22: `async node X` parses, `node X async`
does not. This avoids ambiguity where a modifier could appear in multiple
positions. Thread pinning follows the same pattern: `on thread <N>` is
always a prefix, never a postfix. The grammar position is unambiguous.

### Why `any` instead of omitting the keyword?

`on thread any` is explicit: "I intend the compiler to choose the best
thread for this node." It is distinguishable from "no pinning declared"
(which also maps to the default, but `any` can be consumed by the compiler
for warnings: "node X on thread any, but all other nodes are pinned —
consider explicit pin or remove `any`"). In practice, unmarked = `any`
internally; the author surface omits it when `any` is desired.

## 9. What this does NOT do

- **No GPU thread mapping.** GPU `scope<subgroup|workgroup|...>` (D3, D5)
  is a separate surface. CPU thread assignment is for the reactor runtime,
  not the GPU.
- **No dynamic thread creation.** No "spawn a new thread at runtime."
  Thread count is determined at compile time from `on thread <N>` max
  value. This keeps it simple and predictable.
- **No load balancing.** `on thread any` does not do runtime load
  distribution — it maps to the reactor's existing equilibrium scheduling
  (conflict-detection loop). The keyword is "I don't care where this
  runs" not "distribute this dynamically."
- **No RTOS integration.** No priority scheduling, time-slicing, or
  deadline enforcement. That's an external concern (the host process
  can set thread priorities with `sched_setscheduler` if needed).

## 10. Decisions taken (was: open questions)

1. **Cross-thread state access: decided — proved disjoint, no annotation.**
   Worker slices must be provably independent: jointly satisfiable guards AND
   no write/write or write/read overlap. No `cross_thread_read` keyword: the
   proof engine already owns this question, and a keyword would let an author
   assert what the compiler can verify. Implemented in `thread_plan.rs`.

2. **Thread count limit: decided — `MAX_THREADS = 63`.** A pin above it is
   refused. No hardware-topology awareness: the compiler cannot know the
   deployment's core count, and warning about it would be a guess dressed as
   analysis. (A future target-profile source could supply the number.)

3. **Worker 0 is the main loop: decided.** Worker 0 also owns every unpinned
   node, so the default program is exactly one worker and the threaded lane
   degenerates to today's single-threaded reactor.

4. **`async` is still required: decided.** Pinning answers WHERE, `async` /
   `sync<group>` answer WHETHER (Rule 22). A jointly-satisfiable cross-worker
   pair without `async` on both is refused by the thread gate, in addition to
   the concurrency gate.

5. **No `queue` keyword: decided.** A queue is a runtime data structure, not a
   concurrency class. Express it as a shared `List`/ring with `sync<group>` on
   the producer/consumer pair — `tests/tier1/spsc_smoke` already proves the
   pattern compiles. A new keyword would be permanent compiler knowledge for
   something the language already expresses.

6. **Sequential fallback: rejected.** Serving pins in order would reorder node
   firing; firing order is observable, so a sequential "honoring" of the
   partition changes program behaviour. This is why Stage 1b refuses instead.

## 11. Relation to existing doctrine

- **Rule 2 (Most Efficient Default):** The default is unchanged — single-threaded
  reactor with conflict detection. Pinning is opt-in.
- **Rule 3 (No Obfuscation):** `on thread <N>` is explicit — the hardware
  topology is disclosed at the site of use.
- **Rule 22 (No Implicit Concurrency):** Pinning does not replace `async`.
  The proof engine still requires concurrency classification; pinning is
  orthogonal (it decides *where* nodes fire, not *whether* they may).
- **Derivation-not-recognition:** The compiler derives thread assignment
  from the DAG + proofs + explicit pins. No pattern-matching on node
  names or shapes.
- **Observability razor:** Removing `on thread <N>` changes cache behavior
  (observable in wall-clock time), so the keyword is legitimate.
