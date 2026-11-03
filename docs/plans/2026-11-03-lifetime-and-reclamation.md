# Lifetime and reclamation — north star + phased roadmap (2026-11-03)

Status: ACTIVE (planning complete; Phase 0 actioned).
Parent: `2026-11-03-enum-boundary-gate-removal.md` (Proof 3),
`2026-11-03-ws-frame-fast-path-execution.md` Step 3.
Architecture authority: `docs/architecture/value-destination-ownership-decision-record.md`
(Part VII added 2026-11-03 by this plan).
Root-cause evidence: `BUGS.md` ("Enum image escapes to state without owning"),
`docs/plans/2026-11-03-ws-frame-safe-fast-path.md` Proof 3.

## 0. Decision in one sentence

**Lifetime and reclamation are proofs in the value's destination/ownership
plan, consumed uniformly by emission — never per-construct sink patches, never
a tracing GC, never pervasive runtime refcounting. Unknown ⇒ box/copy/leak,
surfaced by diagnostics, never a premature free.**

### Decisions fixed by this revision (2026-11-03)

1. **Full arc, no interim.** Implement the correct end-state directly: the
   lifetime model *and* the parallel async substrate, with per-worker arenas.
   No "async stays on heap until later" half measure.
2. **Parallel by default where threads exist.** `async` means simultaneous
   firing; the efficient default is real parallelism. Cooperative execution
   remains only as a **declared capability fallback** for targets without
   threads (with a diagnostic), never a silent downgrade.
3. **Per-worker arenas require per-worker identity.** Under
   `clone(CLONE_VM)` a shared worker-id global races, so worker-indexed global
   slots alone are insufficient. Decision: on **thread-capable (hosted)
   targets** the arena cursor lives in a **per-worker TLS control block**
   (`clone(CLONE_SETTLS)`, FS-relative); **cooperative/embedded targets** use a
   single arena. No shared mutable worker id. (Corrected 2026-11-03 after the
   worker-indexed-without-TLS idea was shown unsound.)
4. **`sync<group>` is a real join** in the parallel reactor.
5. **Shared `%State` is safe for fields** because the concurrency gate proves
   eligible `async` pairs have no XOR read-write overlap and each tick reads
   pre-tick state. Only the arena/allocator/output are shared infrastructure.
6. **Determinism policy:** async output interleaving is contract-unspecified;
   compiler/IR determinism is preserved; tests compare per-txn sequences or run
   the cooperative mode.

## 1. Why (the health problem this fixes)

The compiler currently gets enum images right by patching individual sinks
(`own_enum_value` at state stores, tuple elements, struct fields, collection
pushes, lambda captures). That is correct for the covered sinks, but:

- **It does not scale.** Every new construct (tuple, closure, collection,
  aggregate, string, list, task) needs its own copy site and its own leak.
- **It splits the source of truth.** The destination/ownership plan
  (`destination_ownership.rs`, `destination_model.rs`) already models sinks,
  escapes, obligations, and destination classes, but emission consumes only a
  fraction of it. Ownership decisions are therefore duplicated between analysis
  and ad-hoc emitter code (violates Rules 2, 3, 10, 15, 19, 23, 24).
- **It leaks.** `own_enum_value` mallocs and nothing frees; the state-store
  fix trades a use-after-return for a per-store leak. The `free`/`keep` hints
  cannot reclaim these images (`emit_destroy_register` no-ops for untracked
  registers), and the garbage scheduler cannot see them (it only models state
  fields whose initializer contains `Malloc#`/`Alloc#`, and frees once after
  the last touch — not per store).
- **It is unsound to "fix" cheaply.** Local bindings alias enum handles
  (`let x: E = saved;` copies nothing), so buffer reuse and free-on-overwrite
  are unsound without an ownership/alias proof.

The durable fix is to make lifetime a first-class derived fact and let every
allocation, copy, and deallocation follow it.

## 2. Non-goals (explicitly rejected)

| Rejected | Why |
|---|---|
| Tracing GC | Pauses; runtime/libc dependency; incompatible with `.ebv`/freestanding/no-libc targets and the deterministic reactor. |
| Pervasive runtime refcount | Atomic overhead, cycles, async/thread ABI churn; the recorded free-check refcount is already **unsound** (per-fire decrement over-counts multi-fire txns). |
| Full borrow checker | Large language commitment. The language already has `~<-` consume and linear task handles; a lightweight region/ownership discipline is the right amount. |
| More per-sink copy patches | Correct locally, but leak sprawl and hidden per-construct knowledge — the accidental complexity Rule 3 forbids. |
| New syntax | Rule: no new syntax. Everything here is derived; `free`/`keep`/`defer`/`~<-` already exist. |

## 3. The model

### 3.1 Semantic value facts

Extend the existing `ValueFact` (never invent a parallel analysis) with a
`Lifetime`:

```
ValueFact { shape, ownership, destination, escapes, copy_points, lifetime }
```

- `shape`: existing `ValueShape` (scalar/enum/object/tuple/collection/slice/
  closure/task/reference/opaque).
- `ownership`: existing classes (`Owned/Borrowed/Consumed/Shared/Unknown`).
- `destination`: existing classes (`ScalarValue/CallerDestination/
  ScalarReplacedObject/StableCarrier/PackedMemory/ZeroCopyView/Boxed/Unknown`).
- `lifetime`: **new**. Derived from `escapes` (`SinkKind`/`EscapeKind`) plus
  reactor reachability.

### 3.2 Lifetime lattice (new fact)

| Lifetime | Proven when | Allocation | Reclamation |
|---|---|---|---|
| `Frame` | value never escapes the producing callable/txn body | arena bump, or stack slot hoisted to the callable prologue (**never an alloca in an inlined txn loop**) | callable/arena end |
| `TxnBody` | value escapes its callable but not the current transaction | the txn's bump arena | `emit_arena_fini` |
| `StateField(i)` | value's only escape is one state slot `i` (no other alias) | owned image tied to slot `i` | overwrite discipline + scheduled final free |
| `LoopCarrier` | value is loop-carried | existing carrier machinery | loop exit / carrier reuse |
| `Program` / `Unknown` | escapes broadly, unordered reader, alias | boxed heap (immutable) | leak, reported by `memcheck` |
| `External` | export/FFI-visible | frozen boxed ABI | boundary (condition 3) |

The meet is the existing conservative rule (decision record §23): unknown
never selects a faster lifetime; `program` dominates; `external` forces boxed.

### 3.3 Allocation follows lifetime (construction-time strategy)

The image/aggregate is allocated in the region its lifetime names **at
construction**, not copied at sinks. This reuses the existing `Alloc#`
escape→strategy bridge:

- `analysis/allocation.rs` already runs a name-based dataflow DAG
  (`Alloc/Assign/Call/Return/StateWrite`), stamps `analysis_id` on `Alloc#`
  nodes, and `backend/llvm/intrinsics.rs:696` consumes the strategy.
- Generalize the DAG: recurse into expressions to stamp ids on enum
  constructors (`Expr::Call`/`Expr::Field` variants), tuple literals, and
  closure env construction; compute their escapes; emit
  `analysis_id → {Arena|Alloca|Malloc}`.
- Extend `analyze_arena_need` so a txn/defn that owns `Frame`/`TxnBody` images
  participates in the arena, and size the arena to demand (see §8 Phase 1).

Aggregates are the subtlety: **a heap aggregate may not hold a frame-region
element image.** The aggregate's lifetime drives both the container block and
its embedded enum images; they share one strategy.

### 3.4 Reclamation follows lifetime

- `Frame`/`TxnBody` → reclaimed by the txn bump arena (`emit_arena_fini`
  already frees the base). No per-image free.
- `StateField(i)` → one owned image per slot at a time (see §8 Phase 4).
- `Program`/`Unknown` → boxed immutable heap, **leaked** and reported.
- `External` → boundary, frozen.

### 3.5 Copy on escape / destination threading

Keep value semantics with **immutable images**. When the source lifetime does
not dominate the destination lifetime, either:

1. **construct into the destination** (destination threading — the existing
   Proof 19 conditions: complete write, lifetime covers uses, non-overlap, no
   hidden escape, width agreement), or
2. **copy on escape** (explicit, licensed by Proof 22/lifetime domination).

Never mutate a shared immutable image. Never reuse a slot while an alias may
still read it (this is why buffer-reuse/`B1-lite` is rejected).

### 3.6 Async/sendability dimension (corrected 2026-11-03)

`async` is a **semantic declaration of simultaneous firing**
(`docs/architecture/concurrency-and-modifiers.md:67`); the efficient default is
real parallel execution. Family H replaced the pthread pool with a cooperative
sequential phase and documented that multicore parallelism is LOST
(`briev-native-runtime.md:120-168`); the intended substrate is
`SysCall#(SYS_clone)` + futex. Phase 1 restores that substrate; the cooperative
form survives only as a **capability-gated** fallback (with a diagnostic) on
targets without threads.

Consequences for lifetime:

- `Frame`/`TxnBody` images live in a **per-worker arena**, never shared.
- `StateField` images are immutable; handle swaps are atomic; no shared-slot
  reuse (tearing).
- Shared `%State` is safe for fields because the concurrency gate proves
  eligible `async` pairs have no XOR read-write overlap and each tick reads
  pre-tick state.
- A `Sendable`/thread dimension records cross-thread transfer; active only
  where workers are.
- Async interleaving is contract-unspecified; compiler/IR determinism is
  preserved; tests compare per-sequence or run cooperative mode.

### 3.7 Relation to existing machinery (reuse map)

| Machinery | Role in the model |
|---|---|
| `destination_ownership.rs` (`SinkKind`, `EscapeKind`, `ValueFact`, obligations) | escape vocabulary → lifetime derivation |
| `destination_model.rs` (plan, meet, proofs, `EnumCallablePlan`) | canonical plan; add `lifetime` |
| `enum_repr.rs` (`escaping_return_lets`, `sret_safe_defns`) | return-escape evidence |
| `allocation.rs` DAG + `analysis_id` | construction-time strategy transport |
| `global_lifetime.rs` + `memcheck` | `StateField` final-free scheduling + fallback visibility |
| `free`/`keep`/`defer`, `~<-` consume, linear tasks | explicit ownership/move surface; `Consumed` ownership |
| `enum_lowering.rs` validators | condition 1 (layout agreement) |
| `boundary_ownership.rs`, `collect_boundary_enums` | condition 3 (external boundary) |

### 3.8 Execution substrate (parallel async, Phase 1)

- **Workers:** `SysCall#(SYS_clone, CLONE_VM|CLONE_FS|CLONE_FILES|
  CLONE_SIGHAND|CLONE_THREAD, …)` once at startup; no pthreads, no C runtime.
- **Per tick:** release workers (futex/atomic), each runs its assigned
  `async_body_*` on the shared `%State`, join; `sync<group>` is a real join.
- **Arenas:** per-worker TLS control block on thread-capable targets
  (`clone(CLONE_SETTLS)`; FS-relative arena cursor), a single arena on
  cooperative/embedded targets. `emit_arena_alloc` reads/writes the current
  worker's cursor. Worker identity comes from TLS, never a shared global.
- **Lifecycle:** init/reset/free per worker per tick. This also fixes the
  current async-arena-lifecycle gap (async bodies never init/fini the arena
  today; the parallel reactor's `reactor_tick` no-ops for async).
- **Capabilities:** parallel `async` only on thread-capable targets; elsewhere
  cooperative is the declared fallback with a diagnostic.
- **Shutdown/watchdog:** join workers at program end; per-phase watchdog.
- **Remove** the vestigial `%arena_mutex` alloca (`mod.rs:2379`).

## 4. Diagnostics and language surface

- No new syntax. `free x;`, `keep x;`, `defer`, `~<-` remain the explicit
  ownership tools; they gain precise meaning under the model (`keep` = "do not
  reclaim/reuse this slot"; `free` = verified deallocation point; `~<-` =
  consume/move).
- `brievc memcheck` and strict profiles report, per value region, either the
  proven deallocation point or "lives for the program" with the reason
  (unordered reader / escaping alias / unknown ownership). Diagnostics state
  what, why, and the concrete fix (supply a proof, add `free`/`keep`, or a
  contract).
- Every conservative fallback is visible (Rule: no silent special treatment).

## 5. Health metrics

Tracked per commit/benchmark:
- LSan leak growth over N repeated firings on the escape corpus (must be flat
  for proven regions).
- Allocations per firing on the benchmark corpus (arena/heap counts).
- `memcheck` fallback counts (owned-value leaks) — trend down, never up.
- Determinism (`ll_determinism_check`), cross-backend conformance corpus.
- No premature-free ever: ASan/UAR + free-then-read compile errors.

## 6. Phased plan

Each phase is the final mechanism (no throwaway interim) and is gated and
reversible. Unknown ⇒ box/copy/leak at every phase. Dependency order: the
substrate and per-worker arenas land before any arena allocation in async
bodies; analysis facts land before the emission that consumes them.

### Phase 0 — Correctness net

Goal: every escape sink is correct (no UAF) while the model lands.

- Closure-let env site owned — **DONE** (`f397dcae` + its `let`-arm fix).
- Audit remaining sinks: state (done), tuple elements (done), both closure
  sites, struct field, collection push, array element, aggregate recursion.
- ASan-no-inline escape corpus (tuple, closure-let, closure-expr, aggregate
  field, collection-of-aggregate, cross-firing state).
- Exit: corpus green; `cargo test --lib`, `daily_use_smoke`; no new Praetor
  diagnostics.
- Undo: revert the additive out sites; state-store fix stays.

### Phase 1 — Execution substrate + per-worker arenas

Goal: real parallel `async` (the efficient default) with thread-safe memory.

**Prerequisite (found 2026-11-03): the arena is currently never selected and
is unsafe when it is.** `arena_churn` lowers to `@malloc` because two analysis
passes stamp conflicting `Alloc#` ids (the arena-need pass overwrites codegen's
ids). Enabling selection exposes four coupled defects — all must be fixed as
one arena-correctness pass (`BUGS.md` "Alloc# never selects the arena"):
1. Direct-SSA main never calls `emit_arena_init` (inlined bodies grow from a
   zero base).
2. `emit_arena_fini` `@free`s a `brk` pointer (invalid free).
3. Grow resets the bump to base (overwrites live arena data).
4. Arena allocation emits check/grow/ok control flow that folded-loop phi
   predecessor bookkeeping does not account for → invalid IR.
Exit gate: an IR test that `arena_churn`/a folded-loop `Alloc#` program uses
the arena and produces valid, runnable IR; then per-worker arenas.
Implementation plan: `2026-11-03-arena-enablement.md`. Chosen fix reuses the
existing loop convention (dedicated latch/end blocks + captured final block),
not an out-of-line allocation helper.

- **Substrate:** `SysCall#(SYS_clone, CLONE_VM|CLONE_FS|CLONE_FILES|
  CLONE_SIGHAND|CLONE_THREAD, …)` at startup; futex release/wait per tick;
  fn-pointer body table; join at exit; per-phase watchdog; no pthreads, no C
  runtime.
- **Per-worker arenas:** worker-indexed global slots (`@__arena[worker]`:
  ptr/end/base); `emit_arena_alloc` uses the current worker's slot; per-worker
  init/reset/free per tick. Removes the vestigial `%arena_mutex`.
- **`sync<group>`:** real join barrier in the parallel reactor.
- **Async lifecycle fix:** init/fini the arena per worker per tick (the current
  async path neither inits nor frees it).
- **Capabilities:** parallel `async` on thread-capable targets only; elsewhere
  cooperative is the declared fallback with a diagnostic.
- **Determinism:** IR/timing determinism preserved; async interleaving is
  contract-unspecified; tests compare per-txn sequences or run cooperative mode.
- Validate: async parallel correctness (per-sequence), `sync<group>` join,
  many-firing arena correctness (leak growth, no cross-firing corruption),
  `net_stack` O0–O3, determinism, capability diagnostics.
- Exit: `async` runs in parallel on capable targets; arenas per worker; no
  shared-arena race; cooperative fallback documented and gated.
- Undo: restore cooperative `emit_async_phase`; delete the substrate; per-worker
  slots may stay (harmless with one worker).

### Phase 2 — Analysis: `Lifetime` + `Sendable` + per-worker strategy

Goal: the single fact set every later phase consumes.

- Add `Lifetime` derivation (`Frame`/`TxnBody`/`StateField`/`LoopCarrier`/
  `Program`/`External`) to `ValueFact` from `SinkKind`/`EscapeKind` + reactor
  reachability; add the `Sendable`/thread dimension.
- Extend the `Alloc#` escape DAG to enum constructors, tuple literals, closure
  envs; emit per-worker allocation strategy (arena vs heap).
- Conservative meet (D23): `Unknown`/alias ⇒ `Program`.
- Every fact has a named consumer (no dangling facts).
- Exit: unit tests per lattice cell + meet; plan snapshot; `memcheck` can
  report per-value lifetime.

### Phase 3 — Emission: construction-time strategy

Goal: reclaim txn-local images; remove sink copies where construction decides.

- Backend constructs into the region (`Frame`/`TxnBody` → per-worker arena,
  else heap); aggregate container and element images share strategy.
- Retire the ad-hoc `own_enum_*` sink calls for proven regions; keep the heap
  implementation for `Program`/heap and as the conservative fallback.
- Validate: LSan leak-growth flat for non-escaping corpus; allocations/firing;
  no benchmark regression; determinism.
- Exit: non-escaping enum/aggregate images reclaimed; sink copies removed for
  those regions.

### Phase 4 — State-field slots (`StateField(i)`)

Goal: bound persistent-image memory.

- Owned slot discipline: destination-thread or free-on-overwrite, legal only
  when the sole escape is that slot (no alias); else heap/leak (reported).
- Extend `global_lifetime` to schedule the final free for owned slots; honor
  `keep`/`free` and the free-check move rules.
- Async: no shared-slot reuse (immutable swap only).
- Exit: state-image leaks bounded to one per slot; free-then-read is a compile
  error; `memcheck` reflects it.

### Phase 5 — Observability

- `memcheck`/strict: per-value region, proven deallocation or fallback reason;
  owned-value leak report.
- Leak-growth and allocations-per-firing in benchmark evidence.
- Exit: fallback counts tracked; docs updated.

### Phase 6 — Optional sound refcount (only if proven necessary)

- Only for shapes the plan proves require shared ownership, with the
  firing/ownership proof the free-check lacked; atomics only where sendable;
  never pervasive.

### Phase 7 — Gate removal (Proof 3 conditions 1/2/3)

- Condition 1: single `EnumLoweringPlan` at every materialization site.
- Condition 2: lifetime/escape ownership complete for all sinks (Phase 3/4).
- Condition 3: `collect_boundary_enums` keeps only export/FFI.
- `allow_object_inline = c1 ∧ c2 ∧ c3`; `Unknown ⇒ boxed`.
- Exit: gate removed for proven-internal enums; Proof 2 unblocked
  (`ws_partial` malloc gone); I11 + all gates green.

## 7. Validation matrix (all phases)

- Unit: lifetime lattice + meet; DAG escape→strategy; plan snapshots.
- IR-shape: ownership/allocation emitted before publishing (tuple, closure,
  state); negative controls (non-enum, non-escaping).
- Behavioral ASan: `-O0 -fno-inline -fsanitize=address`,
  `ASAN_OPTIONS=detect_stack_use_after_return=1` for the escape corpus.
- LSan leak-growth over repeated firings.
- `cargo test --lib`; `tests/daily_use_smoke` (incl. `net_stack_helpers`
  O0–O3); tier1 corpus.
- `bash benchmarks/ll_determinism_check.sh --quick` (full on backend commits).
- Praetor on changed dirs (no new diagnostics).
- Kani: the lifetime/escape facts (not codegen invariants).
- Runtime: `build_and_bench.sh --runtime`; interleaved A/B for `ws_frame`
  (target: `ws_partial` malloc gone, ≤ C parity) and regressions elsewhere.
- I11: external recommender app/runner is absent from this tree; gate on the
  minimal corpus + net_stack + tier1 until it is fetched.

## 8. Risks and mitigations

- **Wrong lifetime fact ⇒ corruption** → conservative meet; `Unknown ⇒
  Program` (heap/leak); ASan/UAR; Kani; never free without a proof.
- **Aliasing** → immutable images; copy-on-escape; no buffer reuse; free only
  for sole-escape slots.
- **Shared infrastructure races** → per-worker arenas; shared `%State` fields
  are safe by the gate's XOR-disjoint write proof; cross-thread values are
  immutable with atomic handle swaps; no shared-slot reuse.
- **Thread substrate risk** (clone/futex, join, shutdown, watchdog) → isolated
  in Phase 1; capability-gated; cooperative fallback; dedicated async tests.
- **Interleaving nondeterminism** → contract-unspecified for async; IR/timing
  determinism preserved; per-sequence comparison or cooperative test mode.
- **Arena cost** → per-worker, demand-sized; never loop-body alloca; no 64KB
  forced per firing.
- **Drag on compile time/complexity** → facts live in `src/analysis`; DAG is
  already O(program); Praetor ≤15/≤100.
- **Two ownership systems drift** → unify `Consumed`/linear/`free`/`keep` into
  the same plan; one shared lowering helper.

## 9. Undo

- Phase 0: revert the additive out sites (state-store fix stays).
- Phase 1: remove the DAG extension + strategy reads; restore sink copies.
- Phase 2: drop `Lifetime` from the plan (facts-only, no behavior change).
- Phase 3/4: restore ad-hoc `own_enum_value` sink calls; keep the heap path.
- Phase 7: restore `allow_object_inline = !boundary_enums.contains(name)` and
  the full `collect_boundary_enums` arms.
- Decision record Part VII: revert the section; the existing D1–D26 stand.

## 10. Open questions (remaining)

1. Arena sizing per worker: demand-sized vs a fixed per-worker floor. Default:
   demand-sized, grown via the existing arena growth contract.
2. `StateField` reclamation: destination threading vs free-on-overwrite.
   Default: threading where Proof 19 holds, free-on-overwrite for the
   sole-escape case, leak otherwise.
3. Which thread-capable targets get the parallel substrate at first (Linux
   x86_64 only vs aarch64 too). Default: x86_64 first, capability table drives
   the rest. Worker identity/TLS is required on those targets (see §3.8).
4. Whether the cooperative mode is also a user-selectable execution mode
   (`seq`-like) or only a capability fallback. Default: capability fallback +
   a test/benchmark flag, not new syntax.
5. Whether `memcheck` fallbacks become errors under `.s` strict profiles for
   owned-value leaks. Default: warning until Phase 5 stabilizes.

Resolved this revision: full arc (no interim); parallel by default where
threads exist; worker-indexed arena slots; `sync<group>` real join; shared
`%State` safe for fields; async interleaving contract-unspecified.
