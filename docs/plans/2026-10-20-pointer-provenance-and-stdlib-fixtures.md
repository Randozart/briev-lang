# Pointer provenance, default performance, and the stdlib fixture campaign

**Date:** 2026-10-20
**Status:** ACTIVE — Phase 0.
**Supersedes/extends:** `docs/plans/2026-10-18-stdlib-fixture-campaign.md`
(the fixture charter is unchanged; this plan adds the provenance prerequisite
and the performance contract the fixtures must be measured against).

## Charter

Briev's default codegen must be the most efficient codegen for the program.
The compiler must infer provenance, aliasing, invariance, alignment, lifetime,
and specialization from the program and its proofs. Strategy keywords exist
for **correctness intent that restricts the optimizer** — never to make the
default fast.

Concretely:

- Ordinary `Ptr<T>` memory is not volatile. It must stay fully optimizable.
- `vol` / `VolatileLoad#` / `VolatileStore#` remain the explicit tags for
  genuinely volatile objects (MMIO, externally mutated state, required
  reload/ordering semantics).
- No temporary optimizer fence is acceptable. Structural correctness comes
  first; the default stays non-volatile.
- If a benchmark needs `vol` to be correct, the compiler is wrong.
- If a benchmark needs `vol`, `#vectorize`, `#unroll`, or any strategy keyword
  to be competitive, the default codegen is wrong.
- The stdlib fixtures are language probes. A fixture that exposes a compiler
  gap is a result, not a failure; a source workaround that hides the gap is a
  failure.

## Current truth — 2026-10-20

- Branch/commit: `main` at `f8c36f21` (contract gate `deny`).
- `cargo test --lib`: 3,219 green.
- Conformance sweep green under `deny`; contract census 0/0.
- Six parity corpora PASS; full 79-gate harness historically MATCH but must be
  rerun for performance work.
- `cargo test --bin brievc` is red on a pre-existing test-only construct
  (`src/compile.rs:3541`, missing `variadic_param`); `cargo build` is green.
- No CI; gates are local scripts.
- PR #29 (`fix/ptr-index-provenance`) is open and rejected as-is: it marks all
  raw scalar `Ptr<T>` loads/stores volatile. Review covers correctness scope,
  `vol` semantics, test gap, and missing benchmarks.
- Fixture 1 (WebSocket codec) is complete. Fixtures 2–4 are not started:
  - `lib/std/atomic.bv` has primitive wrappers only; no SPSC ring.
  - `lib/std/memory/arena.bv` still has the value-semantics bug.
  - `lib/std/fsm.bv` does not exist.

## The bug this plan must remove

A raw `Ptr<T>` scalar index read and write each recover the pointer with a
fresh `inttoptr i64 %base to ptr`:

- read: `src/backend/llvm/emit_expr.rs:1537-1542, 1580-1593`
- write: `src/backend/llvm/emit_stmt.rs:1247, 1287-1298`
- loop engine duplicate: `src/backend/llvm/loop_engine/counter.rs:2196-2205`
- deref: `emit_expr.rs:1947-1978`, `emit_stmt.rs:1182-1194`
- `Load#`/`Store#`: `intrinsics.rs:946-1019`

Two independent `inttoptr` values of the same address have no shared
provenance. Under `-O3 -flto`, BasicAA can treat a load and a store as
non-aliasing and hoist/CSE the load across the store. The recommender
`picked[]` stale read is the witnessed case.

Related structural hazards:

- `Ptr + n` lowers through the integer template (`add nsw i64`) before the GEP
  arm can run: `emit_expr.rs:7044`, `intrinsics.rs:483`.
- `definition_touches_raw_memory()` does not recognize `Ptr`-typed
  `Expr::Index`/`Expr::Deref`, so a defn can wrongly receive
  `#8 memory(argmem: readwrite)` instead of `#13 memory(readwrite)`:
  `emit_toplevel.rs:3826-3860`.
- `Ptr<T>` resolves to LLVM `ptr` (`casting/graph.rs:1215-1221`) but storage,
  ABI, and many registers are i64 (`mod.rs:1612-1693`,
  `emit_toplevel.rs:3601-3609`).
- `%State` `Ptr` fields use `!invariant.load` (`helpers.rs:2582-2595`); this is
  sound only if the slot is never reassigned.
- `i64` boxing Phase 1 was never executed; `adapt_to_i64` is identity for Ptr
  (`helpers.rs:2430-2438`).
- Loop vectorization still fails on several benchmarks (`loop not vectorized`
  in `benchmarks/results/2026-10-17-state-plain-suite.md`).
- `dyn Trait` LLVM lowerer still panics; `lib/compiler` dogfood is 10/12 red.

## Performance contract

These are the capabilities the compiler must have, not merely be able to
request via keywords.

### P0 — provenance-correct memory

- One canonical pointer identity per allocation/storage origin, or native
  `ptr` SSA carried through dataflow.
- GEP for pointer arithmetic.
- No per-access `inttoptr` when a canonical pointer is available.
- Alias identity preserved across inlining, calls, `%State` loads, fields, and
  params.
- Correct transitive memory effects and attributes.
- `!invariant.load` only for proven-immutable slots.
- No volatile on ordinary memory.

### P0 — proof/fact lowering

- Contracts and analyses become LLVM facts: `!range`, alignment, `nonnull`,
  `dereferenceable`, `noalias`, alias scopes, trip counts, `!prof`,
  invariance, `nsw`/`nuw`.
- No runtime check when the fact is proven.
- Purity/effect facts feed CSE, DCE, LICM.
- Linearity, single-writer, independence, escape/lifetime facts feed alias and
  parallelization decisions.

### P0 — loop and arithmetic optimization

- Canonical/rotated loops, preheaders, induction variables, SCEV.
- Loop and SLP vectorization with alias, alignment, trip-count, and reduction
  facts.
- If-conversion, unrolling, interleaving.
- Strength reduction (div/mod constants, power-of-two masks, pointer
  increments).
- Float vectorization under declared fast-math; FMA contraction where legal.
- Branch layout and profile weights.

### P0 — inlining, specialization, devirtualization

- Cost-model inlining, cross-module/LTO inlining.
- Generic monomorphization and trait specialization.
- Devirtualization where the concrete type is provable.
- Closure/small-helper inlining.
- Contract-fact specialization at call sites.
- Per-byte helper loops collapse when profitable.

### P1 — layout, allocation, boxing

- Escape analysis and stack promotion.
- Arena/scoped allocation from lifetime proofs.
- Eliminate avoidable `i64` boxing.
- SoA/AoS and field ordering from independence proofs.
- Zero-copy slices; bulk copy/memset recognition.
- Avoid `%State` materialization where SROA should see scalars.
- Register-pressure-aware emission; no avoidable spills/frame bloat.

### P1 — whole-program effects

- Transitive effect summaries through inlined/called defns.
- Correct attributes under LTO.
- Facts cross module boundaries.
- No metadata that lies about reads/writes.

### P1/P2 — concurrency

Real threads, atomics/orderings, async scheduling, work distribution. Needed
for SPSC to be a true concurrency test; absent today.

### P0 process — measurement

- Benchmark the actual `-O3 -flto` binary, never `llc -O2`/raw IR.
- Same-commit, same-geometry correctness at a print boundary before timing.
- Interleaved A/B against the baseline worktree.
- Inspect IR/asm for vectorization, spills, and alias failures.
- Any keyword-only win is a default-codegen bug.

## Structural options under investigation

### C — canonical pointer per stable origin (primary)

Add a `FunCtx` cache for canonical `ptr` values, keyed by storage origin
(`(state, idx)`, `(slot, local)`, `(alloc, reg)`, `(arg, i)`) and, where safe,
by the base i64 SSA value. Populate at Malloc#/Alloc#/AddressOf#, Ptr params,
state/local/field loads, and FFI returns. Consume at index/deref/Load#/Store#
and the loop engine. Invalidate on Ptr-slot reassignment.

- Scope: medium.
- Runtime cost: none (fewer conversions).
- Risk: cache invalidation; origins reached through different base regs;
  cross-function calls still re-materialize.

### D — correct raw-memory attributes (primary)

Extend `definition_touches_raw_memory` to recognize `Ptr`-typed `Index`/
`Deref`/arithmetic, using exhaustive `ast::visit`, and propagate transitively.

- Scope: small.
- Runtime cost: none (metadata only).
- Risk: only constrains callers; does not fix an inlined body's intra-function
  alias problem.

### E — Ptr arithmetic via GEP (primary)

Route `Ptr + n` through `getelementptr i8`; keep `PtrAdd#` pointee-scaled
semantics explicit.

- Scope: small–medium.
- Risk: byte-vs-element scaling, double-scaling.

### H — hybrid native pointer SSA (fallback)

Keep i64 storage/boxing/FFI where required, but carry `Ptr<T>` as LLVM `ptr`
through SSA registers, Ptr params/returns, local slots, and internal calls.
Convert once at storage/FFI boundaries.

- Scope: large.
- Runtime cost: none.
- Benefit: provenance survives cross-defn calls and inlining, where C may not.

### A — full native `ptr` (end-state)

Ptr in `%State`, structs, collections, params/returns, FFI, wasm. Best long-term
ceiling; real ABI/layout change. Use only if H+C+D+E cannot deliver the
performance contract.

### Rejected

- Volatile default (PR #29).
- Any fence/barrier inserted to hide the alias bug.
- New user-facing “unsafe pointer” keyword.
- Shape/vocabulary heuristics.
- Requiring a strategy keyword to reach competitive performance.

## Phase 0 — reproduce and measure before building

1. Record the current-commit benchmark baseline (all runtime benchmarks,
   harness command, 5 iterations).
2. Add a failing runtime regression for the cross-defn stale read:
   - raw `Ptr<T>` store in one defn,
   - raw `Ptr<T>` load in a sibling defn large enough not to inline,
   - assert the loaded value at a print boundary,
   - run at `-O0` and harness-exact `-O3 -flto -march=native -ffast-math`.
3. Extract the actual `.ll` and produce experiment variants:
   - V0: current fresh-`inttoptr` per access.
   - VC: one reused canonical pointer per base.
   - VD: raw-memory attrs corrected to `#13`.
   - VE: `Ptr + n` via GEP if the repro uses arithmetic.
4. Link each variant with the harness-exact command; verify output equality.
5. Interleave reference/experiment/C timings where a C analogue exists.
6. Decide C vs D-first vs E-first vs H from measured correctness and
   optimization behavior. Record the decision here before implementation.

## Phase 1 — structural provenance fix

Implement the chosen option set on `main` (no volatile).

Minimum target: **C + D + E**. Escalate to **H** only if the experiment shows
the cross-defn inlined case still loses alias identity. Use **A** only if the
performance gate cannot be met otherwise.

Required changes:

- canonical-pointer mechanism and invalidation rules;
- raw-memory attribute recognition and transitive propagation;
- GEP pointer arithmetic;
- preserve `!range`/alignment/invariance facts already emitted;
- audit every `!invariant.load` Ptr slot for actual immutability;
- ensure wasm32 layout is unchanged unless A is chosen.

Regression pins:

- runtime cross-defn store→load test (compiled, `-O3 -flto`);
- state-slot variant and Ptr-param variant;
- IR assertion that raw-memory defns get `#13`, not `#8`;
- IR assertion that a canonical pointer is reused (no per-access
  `inttoptr` chain) where applicable;
- PR #29’s volatile test is not adopted.

## Phase 2 — performance verification

Run the full harness on the same machine/geometry:

- all runtime benchmarks;
- raw-Ptr focus: `ring_buffer`, `hash_ops`, `hash_ops_idio`, `linked_list`,
  `ws_frame`, `recommender`, `arena_churn`;
- float/loop focus: `float_math`, `float_math_nonzero`, `sweep_*`,
  `telemetry_stream`, `pid_control`, `nbody_*`.

Acceptance:

- no regression beyond noise versus the Phase 0 baseline;
- no `volatile` on ordinary memory;
- no new runtime checks/fences;
- correctness at print boundaries;
- determinism, parity, contract census, suite.

If a benchmark remains behind C, record the specific missing capability
(vectorization, layout, inlining, etc.) — do not hide it with a keyword.

## Phase 3 — fixture campaign resumption

Order: **4 → 3 → 2**, then Fixture 1 `_idio`.

### Fixture 4 — FSM (`lib/std/fsm.bv`)

- `enum TcpState`, `enum TcpEvent`, `enum TcpError`.
- Pure `fsm_step(state, event) -> Result<TcpState, TcpError>`.
- Exhaustive match; illegal edges return `Err`, not UB.
- Contract-gated transitions where statically provable.
- Benchmark: event-stream dispatch vs C `switch`; deterministic corpus.
- Tests: exhaustive arm coverage, legal/illegal transition table,
  interpreter/backend parity.
- Discovery focus: enum payload match lowering, contract-gated edges.
- Known risk: open backend enum-`match` lowering bug (`BUGS.md:8719`). Per
  charter, stop and escalate before a core change if the fixture hits it.

### Fixture 3 — Arena (`lib/std/memory/arena.bv`)

- Convert `struct Arena` → `obj Arena` (reference semantics); fix the lost
  offset update.
- Tighten `init/reset/free` contracts.
- Add `arena_alloc_aligned` with power-of-2 alignment contract.
- Migration: grep all `arena_*` callers; none today outside the module, but
  verify across examples/tests.
- Benchmark: bump allocator vs C; update `arena_churn` only if its shape
  actually exercises the module.
- Tests: alignment, reset, exhaustion, nested/free lifecycle.

### Fixture 2 — SPSC ring (`lib/std/atomic.bv`)

- First verify async-pair scheduling against `benchmarks/async_counters.bv`.
- `obj SpscRing`: Malloc# slots, monotonic head/tail, power-of-2 capacity,
  mask indexing, `AtomicLoad/Store/Cas` with explicit orderings.
- Producer/consumer async node pair; checksum at drain.
- C11-atomics reference; fair bar.
- Tests: full/empty/wrap, CAS, single-producer/single-consumer sequence.
- Honest limitation: reactor is single-threaded today; this validates atomics
  lowering and scheduling, not true cross-thread races.

### Fixture 1 follow-up

- `_idio` variant: word-at-a-time unmask or inlined payload walk, targeting
  the 1.58× gap. Keep the bounds-proven codec as the reference shape.

## Gates per phase

- `cargo test --lib`
- `cargo test --lib conformance_sweep`
- contract census = 0 under `deny`
- `benchmarks/ll_determinism_check.sh --quick`
- parity corpora + full harness where codegen is touched
- Praetor on changed directories
- BUGS.md entry for every discovered compiler gap
- plan update with measured results and decisions
- commit per logical step (no squash of unrelated work)

## Baseline (Rule 12) — commit `b8989e34`, 2026-10-20

`cargo build --release` + `bash benchmarks/build_and_bench.sh --runtime`,
5 iterations, BOUND=50000000. Ratio = Briev/C (<1 = Briev wins). Full log:
`/tmp/opencode/baseline_prov.log`.

| Benchmark | Briev | C | Ratio | Winner | Correct |
|---|---|---|---|---|---|
| ring_buffer | 0.0459s | 0.0487s | 0.94x | Briev | MATCH |
| float_math | 0.1238s | 0.0469s | 2.64x | C | MATCH |
| float_math_nonzero | 0.2458s | 0.1651s | 1.49x | C | MATCH |
| sparse_dispatch | 0.0435s | 0.0604s | 0.72x | Briev | MATCH |
| print_loop | 0.0309s | 0.0571s | 0.54x | Briev | MATCH |
| nbody_newton | 7.7333s | 7.2192s | 1.07x | C | MATCH |
| nbody_newton_accel | 1.4540s | 0.1376s | 10.57x | C | MATCH |
| nbody_sqrt | 2.8140s | 3.4902s | 0.81x | Briev | MATCH |
| nbody_sqrt_idio | 2.8933s | 3.2094s | 0.90x | Briev | MATCH |
| fasta | 0.1935s | 0.2647s | 0.73x | Briev | MATCH |
| fannkuch_redux | 0.0624s | 0.0562s | 1.11x | C | MATCH |
| mandelbrot | 0.6404s | 0.6721s | 0.95x | Briev | MATCH |
| kalman_filter_runtime | 0.1883s | 0.1818s | 1.04x | C | MATCH |
| knucleotide | 0.1813s | 0.1929s | 0.94x | Briev | MATCH |
| cancel_math | 0.0430s | 0.0618s | 0.70x | Briev | MATCH |
| bit_clear | 0.0001s | 0.0003s | 0.33x | Briev | MATCH |
| queue_drain | 0.0287s | 0.0624s | 0.46x | Briev | MATCH |
| queue_drain_sym | 0.0311s | 0.0641s | 0.49x | Briev | MATCH |
| queue_drain_idio | 0.0302s | 0.0587s | 0.51x | Briev | MATCH |
| stack_push_pop | 0.0322s | 0.0597s | 0.54x | Briev | MATCH |
| interval_step | 0.0436s | 0.0630s | 0.69x | Briev | MATCH |
| telemetry_stream | 0.2357s | 0.1651s | 1.43x | C | MATCH |
| pid_control | 0.4277s | 0.3492s | 1.22x | C | MATCH |
| matrix_pipeline | 0.4997s | 0.5256s | 0.95x | Briev | MATCH |
| accumulator_flush | 0.1338s | 0.1470s | 0.91x | Briev | MATCH |
| sweep_sparse | 0.2315s | 0.1546s | 1.50x | C | MATCH |
| sweep_mid | 0.2317s | 0.1772s | 1.31x | C | MATCH |
| sweep_dense | 0.4032s | 0.3715s | 1.09x | C | MATCH |
| sweep_arr | 0.4561s | 0.4397s | 1.04x | C | MATCH |
| series_converge | 0.0002s | 0.0002s | 1.00x | ~tie | MATCH |
| global_lifetime | 0.0318s | 0.0708s | 0.45x | Briev | MATCH |
| deep_recursion | 0.0003s | 0.0005s | 0.60x | Briev | MATCH |
| arena_churn | 0.0303s | 0.0968s | 0.31x | Briev | MATCH |
| linked_list | 1.0472s | 2.0168s | 0.52x | Briev | MATCH |
| hash_ops | 0.8685s | 0.9991s | 0.87x | Briev | MATCH |
| hash_ops_idio | 0.7495s | 0.7069s | 1.06x | C | MATCH |
| enemy_swarm | 0.1129s | 0.1405s | 0.80x | Briev | MATCH |
| recommender | 0.0128s | 0.0115s | 1.11x | C | MATCH |
| ws_frame | 0.0949s | 0.0246s | 3.86x | C | MATCH |

`ws_frame` is 3.86x at this baseline versus 1.58x in
`benchmarks/results/2026-10-18-fixture1-ws-frame.md`; the provenance work must
not accept that as normal. Track it explicitly in Phase 2.

The 2026-10-18 table in `docs/plans/2026-10-18-stdlib-fixture-campaign.md` is a
historical reference, not the baseline for this work.

## Risks

- Canonical-pointer caching can be unsound if a Ptr slot is reassigned while a
  cached pointer survives.
- `!invariant.load` may already be unsound on reassigned Ptr fields; audit
  before relying on it.
- Cross-defn inlining may still defeat C; H/A may be required.
- GEP changes can alter byte-vs-element scaling; pin both Ptr index and
  PtrAdd# semantics.
- Fixture 4 may hit the match-lowering bug; that becomes a separate structural
  workstream, not a source workaround.
- Fixture 2 cannot prove true concurrency until real threads exist; mark its
  result honestly.

## Undo

Every structural change lands as its own commit with rationale comments.
If the provenance approach is refuted, revert that commit and return to the
Phase 0 experiment record; never restore volatile-by-default.
