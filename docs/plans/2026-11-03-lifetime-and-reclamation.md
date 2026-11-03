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

### 3.6 Async/sendability dimension

`async` txns run on a worker pool (`strategy.rs`, `dispatch.rs`). Lifetime
proofs must include cross-thread transfer:

- `Frame`/`TxnBody` images are **not** shared (arena is thread-local or
  mutex-guarded; default: async programs use `Malloc` for anything that could
  cross a task).
- `StateField` images written by async txns keep the existing explicit-
  concurrency contract; a reused slot is forbidden (tearing); heap images are
  immutable and swapped atomically.
- Add a `Sendable`/thread dimension when the plan gains task/message sinks,
  reusing the `SinkKind` escape vocabulary.

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

Each phase is independently valuable, gated, and reversible. Unknown ⇒ box at
every phase.

### Phase 0 — Correctness net (interim; unblocks gate removal)

Goal: every escape sink is correct (no UAF) while the model lands.

- Fix the **closure-let env site** (`emit_stmt.rs:494`; the earlier fix only
  covered `emit_expr.rs:1955`): own enum captures before storing.
- Audit remaining sinks: state (done), tuple elements (done), both closure
  sites, struct field, collection push, array element, aggregate recursion.
- Add the ASan-no-inline escape corpus (tuple, closure-let, closure-expr,
  aggregate field, collection-of-aggregate, cross-firing state).
- Exit: corpus green; `cargo test --lib`, `daily_use_smoke`; no new Praetor
  diagnostics.
- Undo: revert the additive out sites; state-store fix stays.

### Phase 1 — Construction-time strategy (local region inference)

Goal: reclaim txn-local images; remove sink copies where construction decides.

- Extend `analysis/allocation.rs` DAG to enum constructors, tuple literals,
  closure envs; stamp ids; emit strategies.
- Backend: constructor/materializer consult the strategy; `Frame`/`TxnBody`
  → arena, else heap. Aggregate container and element images share strategy.
- Arena: reuse when `needs_arena` is already set; otherwise add a **small
  demand-sized** arena for the txn (never force the 64KB default per firing;
  never `alloca` inside an inlined txn loop — stack growth).
- Async programs: heap-only.
- Validate: LSan leak-growth flat for non-escaping corpus; allocations/firing;
  no benchmark regression; determinism.
- Exit: non-escaping enum/aggregate images reclaimed; sink copies for those
  regions removed; delay no regression.

### Phase 2 — General `Lifetime` fact in the plan

Goal: one fact for all shapes, not just enums.

- Add `Lifetime` derivation to `ValueFact` from `SinkKind`/`EscapeKind` +
  reactor reachability; extend `CallablePlan`/`ArgumentPlan` with the resulting
  allocation/reclamation requirement.
- Conservative meet; `Unknown`/alias ⇒ `Program` (box/leak).
- Unit tests for each lattice cell and the meet.
- Exit: plan snapshot test; `memcheck` can report per-value lifetime.

### Phase 3 — Region-end deallocation + copy-on-escape

Goal: retire sink-copy for proven regions across all shapes.

- Emission consumes `Lifetime` uniformly (shared lowering helper): construct
  in region, copy-on-escape only when domination fails, destination-thread
  where Proof 19 holds.
- Migrate tuple/closure/collection/struct/state sites off ad-hoc
  `own_enum_value`; keep it only as the `Program` (heap) implementation.
- Exit: sink-copy removed for proven regions; leaks flat for Frame/TxnBody;
  performance A/B; all gates.

### Phase 4 — State-field slots (`StateField(i)`)

Goal: bound persistent-image memory.

- Owned slot discipline: either destination-thread into a per-slot image or
  free-on-overwrite, legal only when the value's sole escape is that slot (no
  alias). Where unprovable ⇒ heap/leak (reported).
- Extend `global_lifetime` to schedule the final free for owned slots; honor
  `keep`/`free` hints and the free-check move rules.
- Async: no slot reuse (immutable swap only).
- Exit: state-image leaks bounded to one per slot; free-then-read is a compile
  error; `memcheck` reflects it.

### Phase 5 — Observability

Goal: no silent leaks.

- `memcheck`/strict: per-value region, proven deallocation or fallback reason;
  owned-value leak report.
- Add leak-growth and allocations-per-firing to benchmark evidence.
- Exit: fallback counts tracked; docs updated.

### Phase 6 — Optional sound refcount (only if proven necessary)

Goal: reclaim `Program`-region values that must be shared.

- Only for shapes the plan proves require shared ownership (e.g., cross-thread
  closures), with the firing/ownership proof the free-check lacked; atomics
  only where sendable; never pervasive.
- Exit: gated behind a proof; default path unchanged.

### Phase 7 — Gate removal (Proof 3 conditions 1/2/3)

Goal: replace the blanket enum boundary gate.

- Condition 1: single `EnumLoweringPlan` at every materialization site
  (validators).
- Condition 2: lifetime/escape ownership complete for all sinks (Phase 3 + 4).
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
- **Async races** → no shared-slot reuse; heap for sendable/unknown; explicit
  concurrency contract unchanged.
- **Arena cost** → reuse existing arena; demand-sized; never 64KB/firing;
  never loop-body alloca.
- **Drag on compile time/complexity** → facts live in `src/analysis`; DAG is
  already O(program); Praetor ≤15/≤100.
- **Two ownership systems drift** → unify `Consumed`/linear/`free`/`keep` into
  the same plan (Phase 2/4); one shared lowering helper.

## 9. Undo

- Phase 0: revert the additive out sites (state-store fix stays).
- Phase 1: remove the DAG extension + strategy reads; restore sink copies.
- Phase 2: drop `Lifetime` from the plan (facts-only, no behavior change).
- Phase 3/4: restore ad-hoc `own_enum_value` sink calls; keep the heap path.
- Phase 7: restore `allow_object_inline = !boundary_enums.contains(name)` and
  the full `collect_boundary_enums` arms.
- Decision record Part VII: revert the section; the existing D1–D26 stand.

## 10. Open questions

1. Arena sizing policy for `Frame`/`TxnBody` images: demand-sized dedicated
   arena vs reuse the 64KB arena when already needed. Default: reuse when
   present; dedicated only if a benchmark proves it.
2. `StateField` reclamation: destination threading vs free-on-overwrite. Default:
   threading where Proof 19 holds, free-on-overwrite for the sole-escape case,
   leak otherwise.
3. Async: whether any `Frame`-region image may live in a thread-shared arena.
   Default: no; heap for sendable/unknown.
4. When to promote `Lifetime` from the allocation DAG into the full plan
   (Phase 2) — after Phase 1 proves the DAG strategy on enums/aggregates.
5. Whether `memcheck` fallbacks should become errors under `.s` strict
   profiles for owned-value leaks. Default: warning until Phase 5 stabilizes.
