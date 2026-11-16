# Per-Worker Arena Partition + Interprocedural Resource Effects

**2026-11-16.** Status: **plan — not yet implemented.** This is the prerequisite
work for explicit thread assignment (Stage 2 of
`2026-11-15-thread-assignment.md`), and it stands on its own: it closes a
soundness gap where the compiler's own emitter state is invisible to the
analyses that refuse data races.

**Companions:** `2026-11-15-thread-assignment.md` (§7.1, §7.2 — the findings this
plan implements), `derivation-not-recognition.md` (declared facts must be
consumed; naive fallback carries a retirement gate), `keyword-taxonomy.md`
(ambiguity vs strategy keywords — **no new keyword is introduced here**),
`docs/architecture/overview.md`.
**Feeds:** Golden Rules 2 (derive the fastest correct default), 3 (no hidden
special treatment), 22 (no implicit concurrency).

---

## 1. The problem, stated precisely

Briev refuses cross-corruption for **declared** state:

- `src/analysis/concurrency_gate.rs` — jointly satisfiable pairs must be
  `async` or `sync<group>` (Rule 22).
- `src/analysis/thread_plan.rs` (landed 2026-11-15) — cross-worker write/write
  and write/read overlap is a hard error naming the field.

Two classes of state escape that refusal, and both escape for the *same* reason:
the access sets are computed from the AST body, so anything the author did not
write in that body is invisible.

### 1.1 Emitter-injected `%State` slots

`src/backend/llvm/mod.rs` injects at codegen time, after every frontend analysis
has run:

| Slot | Injected when | Written by |
|------|---------------|------------|
| `__arena_ptr` | `ctx.needs_arena` non-empty | `emit_arena_alloc`, `emit_arena_init`, `emit_arena_reset` |
| `__arena_end` | same | `emit_arena_init`, grow path |
| `__arena_base` | same | `emit_arena_init`, grow path |
| `cycle_count` | cycle-count dispatch | reactor tail |

An `Alloc#(n)` call never names `__arena_ptr`, so `alloc_strategies`,
`xor_overlap`, `fusable_pairs`, `build_write_masks`, the concurrency gate and the
thread gate all treat two allocating nodes as independent.

### 1.2 Callee-reached shared globals

`collect_assigned_identifiers` / `collect_read_identifiers`
(`src/backend/mod.rs:1298`) are **intraprocedural**: they walk the body's own
assignments including `when` bodies, `foreach` bodies, match arms, and index/field
stores — but `Expr::Call` is not expanded into callee effects.

So `lib/std/cast_lanes.bv:652-669`:

```briev
let stdout_buf: Int = 0;
let stdout_fill: Int = 0;
...
Store#(stdout_buf + stdout_fill, b, 1);
```

declares real, visible state that a node mutating it through the print lane never
appears to touch. The `stdout_fill` cursor is a genuine shared resource with an
*observable* effect: interleaved bytes and reordered output.

### 1.3 Why this is not a live bug — and why it is still a defect

Nothing emitted runs two nodes concurrently:

- no `pthread_create`, no `std::thread` spawn anywhere in `src/backend`;
- the pthread pool was removed 2026-09-10 ("Family H") — `emit_async_phase`
  emits async bodies as sequential direct calls;
- `emit_parallel_reactor` is a **masked sequential** dispatch — `%fired_mask`
  orders writes, it does not spawn threads.

The model is therefore *unsound but unexercised*. It becomes a live data race the
moment any real concurrency appears — thread pins (Stage 2), the GPU runner, or
a future parallel dispatcher. Leaving it means the next concurrency feature
inherits a silent race, which is exactly what this language exists to prevent.

### 1.4 What is deliberately NOT in this plan

No new keyword. The author declares nothing: the compiler already knows which
worker runs each node (`ThreadPlan`) and already computes transitive arena need
(`analyze_arena_need`), so both fixes are **derivations** (Rule 2). A keyword
here would make the author pay for a compiler-internal ownership question.

---

## 2. Part A — one reservation, disjoint per-worker slices

### 2.1 Why the current backing store cannot be partitioned naively

Three backing modes exist (`emit_arena_init`, `arena_uses_brk`,
`arena_initial_size` = 65536 from `config/ir-lowering.toml`):

| Target | Init | Grow | Fini |
|--------|------|------|------|
| hosted Linux (x86_64/aarch64) | `brk(0)` then `brk(base + initial)` | extend `brk` in place | shrink `brk` back to base |
| hosted other | `malloc(initial)` | `realloc`, base moves | `free(base)` |
| embedded | static `@embedded_heap` | static | nothing |

The process break and `realloc` are **process-global**. So merely duplicating the
three state slots would be wrong in three ways:

1. Two workers calling `brk(0)` in the same tick see the same break, then both
   extend it — the second worker's slice starts where the first's ended, and the
   comment "the region never moves" stops being true.
2. `emit_arena_fini` on one worker shrinks the break to *its* base, unmapping
   memory another worker's slice still lives in.
3. `realloc` moves a base, which breaks the fixed partition and any pointer
   identity the arena promised.

### 2.2 The design

**Reserve once, slice per worker, never grow the backing store from a worker.**

- **Reserve** in the main prologue, before any worker starts:
  `reserve = workers * arena_initial_size`, one `brk(0)` + one `brk(base+reserve)`
  on hosted Linux (one `malloc` elsewhere; a static slice for embedded). The
  reservation base is the only thing stored beyond the per-worker triples.
- **Each worker owns** `[base + w*size, base + (w+1)*size)`:
  `ptr = base_w` (after init), `end = base_w + size`, `base = base_w`, where
  `base_w = reservation_base + w * arena_initial_size`.
- **Init becomes a pure rewind**: `ptr = base_w` — three state stores, **no
  syscall**. This is strictly better than today's per-tick `brk(0)`/`brk(+)`,
  and it preserves the existing guarantee that repeated ticks do not grow memory
  (`emit_arena_reset` already does exactly this rewind).
- **Overflow inside a slice → `malloc` fallback.** `emit_malloc_fallback` already
  exists for the no-arena case. A worker whose bump would exceed its `end` takes
  the fallback for that allocation; such allocations are freed normally. The
  arena becomes a fast path, not a hard cap, and no worker ever calls `brk` or
  `realloc`.
- **Fini is a no-op for pinned programs** (the reservation lives for the process,
  as `@embedded_heap` already does). The unpinned path keeps today's
  shrink-to-base.
- **The single-worker path is byte-identical to today.** `workers == 1` → one
  triple over the whole reservation, same init, same grow, same fini. This is the
  property the determinism gate and the IR A/B check.

### 2.3 Cost and bounds

`workers × 65536` bytes reserved. At `MAX_THREADS = 63` that is ~4 MB of
*reservation* (not touched memory — `brk` on Linux and `malloc` elsewhere commit
lazily). Documented in `config/ir-lowering.toml` next to `arena_initial_size`,
with a note that the reservation scales with the pinned worker count and pinned
programs are opt-in.

### 2.4 Code shape

Replace the three `Option<usize>` fields with one source of truth:

```rust
/// One bump region: the cursor, its end, and the base the reset rewinds to.
pub(crate) struct ArenaTriple { pub ptr: usize, pub end: usize, pub base: usize }

// backend fields
pub(crate) arena: Vec<ArenaTriple>,   // index = worker; empty = no arena
pub(crate) current_worker: u32,       // set when a tick function starts emitting

/// The triple for the function currently being emitted. Falls back to the
/// first (worker 0) for helpers emitted outside a tick (defn bodies, probes).
pub(crate) fn arena_triple(&self) -> Option<&ArenaTriple> {
    self.arena.get(self.current_worker as usize).or_else(|| self.arena.first())
}
```

- Injection (`mod.rs`, the `needs_arena` block) pushes `workers` triples instead
  of setting three scalars.
- The 23 direct index reads become `self.arena_triple()?` lookups; the 22
  init/reset/fini call sites keep their signatures and read
  `self.current_worker`.
- Injection must be deterministic: field indices are appended in
  `__arena_ptr_w0, __arena_end_w0, __arena_base_w0, __arena_ptr_w1, …` order so
  the `.ll` stays byte-stable.

---

## 3. Part B — worker identity through emission

Minimal, because the plan already exists:

- `emit_reactor` / `emit_parallel_reactor` gain an explicit worker index (and
  `emit_reactor` gains the `fn_name` parameter from the thread-assignment plan —
  it has exactly one hardcoded `define void @reactor_tick(...)` site).
- `current_worker` is set at the top of each tick emission and restored on exit
  (the emitter is `&mut self`, so a small guard or explicit save/restore).
- Every other path (SSA main, counter engine, probes, library shims, defn bodies)
  emits at worker 0, which is the current behaviour.

This part is independent of threading: with no pins there is exactly one worker
and it is always 0.

---

## 4. Part C — interprocedural resource effects

One fixpoint summary, computed once in the frontend (the
frontend-driven-dispatch pillar), modelled on the existing
`analysis::allocation::analyze_arena_need` + `call_graph::propagate_arena_need`
(which already does exactly this shape for arena need — `src/analysis/call_graph.rs:157`).

### 4.1 The summary

```rust
/// What a callable does to state that outlives it, transitively (callees
/// included).
pub struct ResourceEffects {
    /// Writes to top-level `let`s (globals), by name.
    pub writes: BTreeSet<String>,
    /// Reads of those globals (needed for write/read overlap between workers).
    pub reads: BTreeSet<String>,
    /// Performs an observable sink: an `out`-marked call, `Print#`, a syscall,
    /// or any name in `AnalysisResults::observable_names`.
    pub observable: bool,
    /// Needs the bump arena (already computed by `analyze_arena_need`).
    pub allocates: bool,
}

pub fn analyze_resource_effects(items: &mut [TopLevel]) -> ResourceMap;
```

- Direct part: reuse `collect_assigned_identifiers` /
  `collect_read_identifiers` per body, plus a call scan for the observable set.
- Transitive part: the same fixpoint shape as `propagate_arena_need`, over
  `CallGraph` (`src/analysis/call_graph.rs`) — union callee `writes`/`reads`,
  OR the `observable` flag, OR `allocates`. Deterministic via `BTreeSet`.
- `allocates` is **not** recomputed — take `analyze_arena_need`'s result so the
  two can never drift.

### 4.2 Where it lives and who consumes it

- Computed beside `needs_arena` in `src/compile.rs` (currently line ~954), so
  both share one call-graph build.
- Carried in `AnalysisResults` (frontend-driven dispatch).
- **Thread gate** consumes it (see Part D). This requires moving the gate after
  the summary is computed: today `compile.rs` runs the gate at ~line 643 and
  computes `needs_arena` at ~954. Two options — move the gate down (it is a
  refusal, so ordering is free), or extract a shared helper both call sites use.
  **Chosen: move the gate below the summary.** One definition, no drift.
- One definition of "observable sink" replaces today's two notions
  (`has_unguarded_ffi` for reactor attributes, `observable_names` for liveness).
  The summary *consumes* both rather than redefining them.

### 4.3 What the summary is NOT

- Not a full effect system. It is a **may-write / may-observe** over-approximation,
  which is the conservative direction for a refusal. Precision work (field
  granularity, per-argument effects) is a later pass if a real program is
  refused for a false positive — and then it improves the general system, not a
  special case.
- Not a proof of ordering. It says "this pair must be classified", which is
  exactly what the existing gates already say for declared state.

---

## 5. Part D — the gate and the emission assertion

### 5.1 Gate rule (thread_plan)

For a cross-worker pair `(A, B)` whose preconditions are jointly satisfiable:

| Condition | Verdict |
|-----------|---------|
| `A.writes ∩ B.writes ≠ ∅` (globals, transitive) | **refuse** — data race |
| `A.writes ∩ B.reads ≠ ∅` or vice versa | **refuse** — racy read |
| both allocate and the arena is **not** per-worker | **refuse** — shared cursor (this row disappears once Part A lands) |
| `A.observable || B.observable`, not both in one `sync<group>` | **refuse** — output order is observable |
| otherwise | accept (both must still be `async`, Rule 22) |

The last row is the new capability: once arenas are per-worker, two allocating
nodes on different workers are **allowed**, which is precisely the derivation
win — no author-visible restriction.

The diagnostic names the resource and the fix, house style:

> `'producer' (worker 0) and 'consumer' (worker 1) both write 'stdout_fill' and
> run concurrently. Move both nodes to one worker, give them disjoint state, or
> serialise the pair with 'sync<group>'.`

### 5.2 Emission assertion (the general guard)

`build_write_masks` already computes per-node `%State` write masks. Add a
**debug-only** cross-worker check right after it:

- For every state slot, the set of workers whose tick functions can write it must
  have size ≤ 1.
- Exception list: none initially. A slot must be explicitly exempted (with a
  comment and a reason) if it is genuinely shared — which is itself a design
  smell worth seeing.

This is the part that stops the *next* injected field from repeating the
omission: the assertion fires on any new emitter state without the author of that
field having to remember this plan.

---

## 6. Test plan

### 6.1 Unit

- `analyze_resource_effects`
  - direct global write is summarised;
  - transitive write through one `defn` hop, and through two hops;
  - `Print#`/`out` call sets `observable`;
  - `Alloc#` in a callee sets `allocates` and **matches**
    `analyze_arena_need` exactly (an equality assertion between the two, the
    same shape the codebase already uses for HIR/AST drift).
- `thread_plan` gate
  - cross-worker `stdout` pair → refused with the sink named;
  - same pair with `sync<group>` on both → accepted;
  - cross-worker pair where both allocate → **accepted** (Part A landed);
  - transitive write through a callee → refused;
  - unpinned program still produces an empty plan and no diagnostics.

### 6.2 IR / byte-level

- Pinned, allocating program emits `__arena_ptr_w0` and `__arena_ptr_w1`, and
  **no** `brk`/`realloc` inside either tick body (init is a rewind).
- Each tick writes only its own triple (grep the emitted `.ll` per function).
- **The default path is untouched**: for the whole `ll_determinism_check`
  corpus (unpinned), `.ll` must be **byte-identical** to the pre-change
  compiler. This is the primary regression gate and is run as a before/after
  comparison, not a spot check.
- `%State` layout for unpinned programs is unchanged (same field count and
  order).

### 6.3 Runtime

- After the threaded lane lands: two workers each allocating a growing buffer
  produce identical output to the same program pinned to one worker. (Not part of
  this plan's closure — recorded so the gate exists.)

---

## 7. Risks and rollback

| Risk | Mitigation |
|------|-----------|
| Byte-diff in unpinned `%State`/IR | The `workers == 1` path is specified to be identical; the corpus byte-compare is the gate. Any diff blocks the change. |
| Snapshot tests asserting field indices | Only pinned programs gain slots; pinned programs are new. Any surprise diff is a finding, not noise. |
| Reservation size at 63 workers (4 MB) | `brk`/`malloc` commit lazily; documented next to `arena_initial_size`; pinned programs are opt-in. |
| Effect summary false positives refusing valid programs | Over-approximation is correct for a refusal; if a real corpus program is refused, precision work improves the general pass (recorded as a follow-up, never a special case). |
| Emission assertion firing on a legitimate shared slot | Slot must be exempted explicitly with a reason — the assertion paying off is the point. |
| Late hit: another injected slot I have not enumerated | The Part D assertion is designed to find it mechanically. |

Rollback: each part is a separate commit; Parts A+B are inert without pins,
Part C+D stand alone. Reverting C+D restores today's gate exactly.

---

## 8. Order and estimates

| Step | Content | Estimate |
|------|---------|----------|
| 1 | Part C: `analyze_resource_effects` + fixpoint + equality test against `analyze_arena_need` | 1 day |
| 2 | Part D.1: move the thread gate below the summary; add the sink/transitive rows | 0.5 day |
| 3 | Part A: `ArenaTriple`, `workers`-aware injection, rewind init, malloc fallback, no-op fini; single-worker identical | 1.5 days |
| 4 | Part B: `current_worker` plumbing + `emit_reactor` `fn_name` | 0.5 day |
| 5 | Part D.2: cross-worker write-mask assertion | 0.5 day |
| 6 | Verification: unit suites, IR greps, full lib gate, determinism corpus byte-compare, Praetor | 1 day |

**~5 days.** Steps 1–2 are landable and valuable on their own (they close the
callee-reached-sink gap without touching codegen); 3–5 close the arena gap.

Doing 1–2 first is deliberate: it means the gate is already honest about sinks
before any codegen risk is taken, and each commit is independently reviewable.

---

## 9. Decisions taken (so they are not re-litigated)

1. **Derive, do not keyword.** No `arena` modifier, no "this node may allocate"
   annotation. The compiler knows both facts already.
2. **Reserve-and-slice, not per-worker `brk`.** One reservation; per-worker
   cursors; overflow falls back to `malloc`, never to `brk`/`realloc`.
3. **No arena growth for pinned workers.** Growth's `realloc`/`brk` paths are
   process-global; a fixed slice plus fallback is sound and keeps pointer
   identity.
4. **Refusal over serialisation.** The gate refuses rather than silently
   serialising a pair; serialising is the author's call via `sync<group>`, which
   makes the ordering explicit.
5. **The emission assertion is debug-only.** It costs nothing in release, and its
   purpose is to catch a future injected field during development and in CI.

## 10. Open item worth a decision before step 3

`arena_initial_size` per worker vs total. Reserved above as **per worker**
(`reserve = workers × size`), which keeps a single worker's usable arena exactly
what it is today — so a program that fits today still fits when pinned. The
alternative (split one 64 KB budget across workers) would silently shrink each
worker's fast-path budget and change allocation behaviour as pins are added,
which is the kind of hidden coupling this plan exists to remove.
