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

**Fix (proof-licensed reservation):**
- For a contract-bounded loop, reserve `N * K` once before the loop
  (N = bound register, K = constant alloc per iteration) and **hoist the
  capacity check out of the loop** (the loop cannot overflow a correctly-sized
  reservation). This is the "prove it can be faster" lever.
- Hoist arena init to the loop preheader (drop the per-alloc init check).
- Keep the cursor pointer-typed in registers (no inttoptr/ptrtoint per iter).
- Embed version keeps the static-heap path.

Gate: `arena_churn_sym` ≤ C (target < 200 ms at 50M), no regression.

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
