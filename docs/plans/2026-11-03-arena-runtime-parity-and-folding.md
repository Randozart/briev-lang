# Arena runtime parity + proof-driven allocation folding (2026-11-03)

Status: ACTIVE. Predecessor: `2026-11-03-arena-enablement.md` (selection now
works: default `Alloc#` uses the bump arena, IR-safe, lazy init, brk-aware
teardown). Parent: `2026-11-03-lifetime-and-reclamation.md` Phase 1.

Two goals, both required:

- **R — runtime parity or better than C** on an arena workload that actually
  does the work (arena vs arena).
- **O — keep the proof-driven ability to fold away an allocation** when its
  bytes are provably unobservable.

## Evidence (2026-11-03, BOUND scaling)

`arena_churn` (`Alloc#` + `Store#`/`Load#` + `sum`):

| BOUND | baseline compiler | current (arena) | C arena |
|---:|---:|---:|---:|
| 100k | 2 ms | 2 ms | — |
| 1M | 2 ms | 7 ms | — |
| 5M | 4 ms | 21 ms | — |
| 50M | **18 ms** | **338 ms** | **~240 ms** |

- Baseline is **sublinear** → it proves the allocation dead and folds the
  loop (store→load forwarding, dead malloc eliminated). Legal, but it does not
  measure the arena.
- Current is **linear** (real work) because the bump writes `%state` every
  iteration; LLVM cannot eliminate it. It is **~1.4x C** (338 vs 240).
- So R must close 1.4x (target: ≤ C, ideally beat it), and O must restore the
  fold where semantics allow it.

## Workstream R — runtime parity/better

### R1. Observable runtime benchmark (arena vs arena)

`arena_churn` is tagged `runtime`, so it must do observable work. Split per
benchmark doctrine:
- `arena_churn_sym` (or make `arena_churn` itself) observable in a way the C
  reference mirrors: the allocation's *identity* must be observable, e.g.
  `sum = sum ^ (addr & AMASK)` (address-dependent) or store the arena pointer
  into state and read it after the loop, so store→load forwarding + dead-malloc
  elimination cannot remove the bump. Mirror `arena_churn_c.c` exactly.
- Keep a foldable variant (`arena_churn_fold`, `optimizer` tag) for O.

Gate: Briev output == C output at the harness geometry; timing ratio ≤ 1.0
(record in `benchmarks/results/`).

### R2. Register-resident bump/end — FINDING: already promoted

A local-cursor experiment (entry allocas) gave no gain (~338 vs ~343 ms):
LLVM's **SROA already promotes** the inlined main's `%State` arena fields to
loop-carried phis (zero allocas in `-O3` IR). The change was reverted.

The real gap is per-iteration **control flow and address conversion**:
- lazy-init check + capacity check branches every allocation;
- `i64` ⇄ `ptr` conversions around the bump.

**Update (2026-11-03, after trying both):**
- Reservation (reserve N*K, bump unchecked) was implemented and **regressed**
  (~1270 ms): it grows to N*64 = 3.2 GB with no reuse (cache-hostile). Reverted.
- The benchmark C reference had an 8-byte vs 64-byte stride mismatch; fixed.
  With a symmetric stride, a **correct growing** arena is at **parity**
  (Briev ~1270 vs C ~1370 ms). Briev's current ~320 ms comes entirely from
  **reset-on-grow reuse**, which overwrites live arena allocations — a latent
  soundness bug (`BUGS.md`).

**Real fix (scope-lifetime reset with a liveness proof):**
- Prove which arena allocations do not cross a scope boundary (loop iteration
  / txn firing). Reset the arena at that boundary only when no live allocation
  outside the released set remains.
- Then reuse is **sound and fast** (bounded, cache-hot), no per-allocation
  capacity check needed within the scope, no unbounded growth.
- This is the proof-licensed lever: Briev proves the lifetime and reuses;
  a naive/growing arena is the floor.

**Implemented 2026-11-03 (countdown emitter):** grow preserves the bump;
per-iteration reset emitted under `arena_scope_reset_safe`. Result:
`arena_churn_sym` **~35 ms** (sound) vs C ~1370 ms; the escaping variant is
correct and rejects the reset. Regression: `arena_scope_reset_gate`.
Follow-up: extend the reset to the SSA/version-DAG inlined loop emitters.

Gate: `arena_churn_sym` ≤ C with a **sound** reuse proof (no live allocation
overwritten), plus a regression that a live arena allocation spanning a reset
point is either preserved (heap) or rejected.

### R3. Reset/reclaim

Per-scope reset of the local cursor (arena allocations are non-escaping;
escaping allocations are `Malloc`). No 64KB/tick growth. `emit_arena_fini`
stays brk-aware (shrink), or no-op when the process exits.

## Workstream O — proof-driven allocation folding

### O1. Dead-allocation elimination

Consume the escape proof to elide an `Alloc#` whose bytes are unobservable:
- Inputs: the allocation DAG / `ValueFact.escapes` (already the strategy
  source) and the destination model. Replace the TEMP
  `will_escape_current_allocation() -> false` stub with a real query.
- If every use of the allocation is a forwarded store/load that is itself
  dead, and the pointer never escapes (no state store, no collection, no
  call arg to a retaining callee, no return), emit **nothing** (no allocation,
  no bump); let LLVM fold the surrounding arithmetic.
- Conservative: `Unknown`/escape ⇒ keep the allocation (R path).
- Must remain sound with the lifetime model: the proof is "the value's region
  carries no observable effect".

### O2. Optimizer benchmark + IR gate

- Keep `arena_churn_fold` (or the current `arena_churn`) as an
  `optimizer`-tagged benchmark; assert the fold (e.g., no bump in the hot
  loop / output equals the closed form) and record the ~18 ms result.
- IR regression: an unobservable `Alloc#` emits no assignment/malloc;
  an observable one (R1) still does.

## Interaction / doctrine

- R is the **runtime** default: real work must be fast (Rule 2: the compiler
  must reach the efficient default for every program).
- O is the **optimizer** default: when semantics permit elimination, the
  compiler proves and eliminates (no dead frame/bump). Both coexist: O's
  proof decides, R's codegen executes when the value is live.
- No "keyword to win": the fast path is the default; no strategy keyword is
  required for parity.

## Validation

- Correctness: harness outputs == C (R1, O2); BOUND=5 gate.
- Runtime: interleaved A/B ×5 for `arena_churn_sym` vs C (record in
  `benchmarks/results/2026-11-03-arena-runtime.md`).
- Optimizer: `arena_churn_fold` asserts the fold; IR gate.
- `cargo test --lib`, `daily_use_smoke`, `ll_determinism_check --quick`,
  Praetor, Kani for the observability/escape fact, full `--runtime` A/B.

## Risks / undo

- Register-resident cursor must stay correct across the reactor/tick boundary
  (sync to `%state` at boundaries). Undo: revert to the `%state` cursor.
- Dead-allocation elimination is soundness-sensitive: `Unknown ⇒ keep`.
  Undo: restore the allocation (O is additive).
- Do not weaken the C reference; mirror the observable semantics in both.
