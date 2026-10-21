# Machine-backed state and loop emission

**Date:** 2026-10-20
**Status:** ACTIVE — design + Phase 0 experiment record.
**Supersedes/extends:**
- `docs/plans/2026-07-29-phi-register-pressure-capping.md` (the register-budget
  idea was never fully implemented; this plan revives it on the current
  direct-SSA pipeline).
- `docs/plans/2026-10-20-performance-regression-bisect.md` (results record).

## Charter

We want the best result on both classes of loop without pattern matching,
benchmark names, vocabulary recognition, or a strategy keyword:

- **print/state loops** — `queue_drain`, `interval_step`: stateless print
  chain, no `%State` escape through print.
- **dense float loops** — `float_math`, `nbody_*`: state fields must not
  create register pressure that spills the loop's accumulator.

The compiler must decide this from the **machine model** (target register
classes/budgets) and the **loop shape** (carried fields, liveness, write set,
types) — not from a benchmark name, a field-name prefix, or an algorithm
pattern. The loop emission must be deterministic and independent of LLVM SROA's
local choice.

## What we measured (2026-10-20)

Fast era `3c15268a` vs state/plain `5aa90185`, same machine/harness:

| Benchmark | Fast | State/plain | Current | Note |
|---|---:|---:|---:|---|
| float_math | 0.90x | 2.01x | 2.74x | real regression |
| telemetry_stream | 0.86x | 1.03x | 1.48x | real regression |
| pid_control | 0.99x | 1.23x | 1.23x | real regression |
| sweep_sparse | 1.30x | 1.44x | 1.49x | real regression |
| queue_drain | 1.35x | 0.45x | 0.47x | real improvement |
| interval_step | 1.23x | 0.61x | 0.72x | real improvement |
| ws_frame | — | — | 1.58x → **1.48x** | fixed separately |

Controlled `5aa90185` experiments:

| Variant | float_math | queue_drain | interval_step |
|---|---:|---:|---:|
| stateless print + stack state | 2.72x | 0.45x | 0.61x |
| stateless print + stateful stdout | **0.92x** | 1.44x | 1.46x |
| stateless print + global state | 2.71x | 1.68x | 1.67x |
| stateless print + launder state base | 2.61x | 0.49x | 0.69x |
| stateless print + noinline print fns | ~2.7x | — | — |

Conclusions:

1. The regression is **not** the raw-pointer provenance patch (`float_math` has
   no `Ptr`).
2. It is **not** simply SROA/state escape: global state and a launder barrier
   do not recover float_math.
3. It is **not** print-function inlining: `noinline` on the print functions
   does not recover it.
4. It is **not** solved by making stdout stateful: that trades queue/interval
   away.
5. The fast binary keeps the trace accumulator in `xmm0`; the slow binary
   spills it to `0x14(%rsp)` every iteration. The trigger is the stateless
   print/state shape interacting with clang's SROA/register allocation.

The lesson: we cannot leave the state/register decision to LLVM's local SROA
choice. We need an explicit, target-driven loop state plan.

**Phase 0 negative result (2026-10-20):** capping the countdown-loop Float
phis to the target budget (11/12) and making the state base opaque with
`llvm.launder.invariant.group` did **not** recover float_math (2.61x). The
trace accumulator still spilled. The state/phi plan is necessary for
deterministic register control, but it is **not sufficient** for this
regression.

**Refined primary hypothesis:** the fast stateful-print experiment changed
the *memory shape*: print functions read/wrote through a state pointer instead
of addressing bare `@stdout_*` globals. Forcing all defns to take `%state`
(pointer ABI alone) did **not** recover float_math (2.93x); adding two dummy
state fields also did not (2.72x). The distinguishing factor is the print path
**accessing memory through a context pointer**. Group plain/plumbing lets into
a **plain context** and pass its pointer to functions that use them; keep
machine `%State` unescaped. That is a storage-class ABI, not a print-name
special case. It should recover the pointer-memory codegen while preserving
the stateless machine state that wins queue/interval.

## Target architecture

### 1. `StateLayoutPlan` (frontend)

For every folded bounded-counter node, compute a plan from `LoopShape`:

- **carried fields** (already frontend data),
- **types/register class** for each carried field,
- **write/read sets** and liveness,
- **live accumulators/temporaries** (e.g. the trace sum),
- **vector-isomorphic groups** (existing `vector_groups`),
- **target register budget** (from `config/targets.dbvl`: float/int registers,
  reserved temps).

Classify each field:

- `HotPhi` — kept as an explicit SSA phi in the loop.
- `Memory` — loaded/stored through `%State` each iteration (or hoisted if
  invariant).
- `Invariant` — read-only relative to the loop; one hoisted load.

The plan is in `AnalysisResults` (frontend-driven), not re-derived in the
backend.

### 1b. Plain/effect context (added from Phase 0)

Plain lets (`stdout_buf`, `stdout_fill`, future plumbing) lower into a
`PlainContext` aggregate. Functions that use plain state take a `ptr` to that
context, exactly as defns that use machine state take `%state`. Machine `%State`
stays unescaped. This removes bare `@global` mutation from hot effectful calls
while preserving the plain/state split. It is the first experimental slice,
because the state/phi-cap prototype alone did not recover float_math.

### 2. Opaque state allocation

The loop must not depend on SROA to decide state promotion. For folded hosted
mains, state is allocated as an **opaque aggregate**:

- hosted targets: one heap allocation for `%State` (or a per-program
  allocation), pointer passed to the loop;
- freestanding/embedded targets: a stack/static allocation whose address is
  legitimately observable (no fake no-op barrier);
- parallel/reactor paths: per-thread/per-worker state, not one process global.

Global `@__briev_state` remains for ISR (existing mechanism) but is not the
general solution: global state alone did not recover float_math.

The state base choice is target-config-driven; no benchmark names.

### 3. Explicit loop emission

Extend the loop engine to emit the `StateLayoutPlan` directly:

- `HotPhi` fields: phi + backedge value, no per-iteration memory round-trip.
- `Memory` fields: GEP + load/store at use sites; no SROA dependence.
- `Invariant` fields: one hoisted load (only with a real immutability proof).
- `HotPhi` selection is capped by the target register budget, with reserved
  slots for the loop counter, accumulator, and body temporaries.
- The trace/accumulator is part of the reserved hot set; it must not be pushed
  into memory by bulk fields.

This is the revival of the 2026-07-29 capping idea, but now:
- the state base is opaque, so SROA cannot silently undo the cap;
- the decision is frontend/target-driven, not an ad-hoc `write_density`
  threshold;
- vector groups can pack isomorphic hot fields into vector phis when the
  target supports them.

### 4. Stateless print stays

`stdout_buf`/`stdout_fill` remain plain plumbing. Print bodies do not take
`%State`. The loop plan, not a stateful print signature, is responsible for
register pressure. This preserves the queue/interval wins.

### 5. Why this is universal

- It is derived from the **machine model** (register budgets) and **loop
  facts** (carried fields, liveness, types, vector groups).
- It contains no benchmark names, no field-name prefix matching, no algorithm
  vocabulary.
- It works for any future target by changing the target register model, not
  by adding a match arm.
- It makes the loop shape deterministic: the same plan emits the same
  phi/memory split regardless of clang's SROA mood.
- It does not add a user-facing keyword or require a source hint.

## Storage-class inference is the source of truth

The programmer states **semantics and contracts**. The compiler derives storage
class and materialization. `plain` and `state` are not performance choices; they
are optional assertions for cases inference cannot prove, plus `vol` for genuine
hardware volatility.

### Semantic classes

- **Const/Local** — immutable or not live beyond one invocation; no address
  escape.
- **Plain/plumbing** — address-taken, FFI-visible, or shared through helpers,
  but not reactor state: no cross-tick liveness, no contract surface, no
  rollback/snapshot obligation.
- **Machine state** — read/written across reactive ticks/nodes, referenced by a
  contract surface, part of rollback/snapshot, exported, or shared with
  async/multi-node execution.
- **Volatile** — externally mutated hardware; not derivable, remains the
  explicit `vol` correctness tag.

### Derivation facts

The compiler must derive the class from observable behavior, not names:

- cross-tick and cross-node read/write liveness,
- contract pre/post reference surfaces,
- rollback/snapshot participation,
- address-of / FFI / export escape,
- async/multi-node sharing,
- local/SSA lifetime and mutability.

If a value is only used inside one node and does not escape, it should remain
local/SSA — neither `plain` nor `state`. `Plain` is selected only when
persistence/addressability is required but reactor visibility is not. Machine
state is selected when any cross-tick/contract/rollback/export/async fact
requires it. Uncertainty falls back to machine state; never silently choose
plain for speed.

### Annotations are assertions, not hints

- `state let` asserts “must be reactor-visible”; the compiler diagnoses a
  contradiction.
- `plain let` asserts “must not be reactor-visible”; the compiler diagnoses a
  contradiction.
- Neither annotation may be required to make ordinary code fast.
- `vol` remains the only explicit hardware-volatility tag.
- Library annotations are temporary scaffolding; remove them once inference is
  proven. A benchmark that only wins with an annotation is a compiler bug.

### Materialization is derived

Given the semantic class and the target model, the compiler chooses:

- local/SSA,
- stack alloca,
- plain context pointer,
- machine `%State`,
- global/thread-local where exports/threads require it.

The `PlainContext` ABI is therefore a **materialization of derived Plain
state**, not a response to a user keyword. The same applies to the machine-state
`HotPhi`/`Memory` plan.

### Experiment order

1. Remove `plain` from `stdout_buf`/`stdout_fill`; prove inference still selects
   plain (globals/context, not `%State`). **DONE 2026-10-20:** annotations
   removed; `classify_plain_lets` still produces `@stdout_buf`/`@stdout_fill`
   module globals and the 15-field `%State`; conformance + tier-1 E2E green.
   No performance change expected from the annotation removal alone.
   `compute_defn_needs_plain` + `defn_takes_plain` landed as the ABI analysis
   foundation, with a transitive/plain-only unit test.
2. Implement `needs_plain` and the plain-context ABI over derived plain fields.
3. Validate on a second, independent plain-state program, not just print.
4. If inference cannot prove an export/FFI/threading case, add the missing fact
   or keep an explicit assertion; do not reintroduce a performance keyword.

## Implementation note — where plain lets actually live

`plain let` declarations are parsed as `TopLevel::Init`, not as
`Statement::Let`. The context prototype must therefore key off:

- `ctx.plain_lets` for membership,
- `ctx.inits` for the declared type + initializer,
- deterministic sorted order of the plain names.

`ctx.plain_global_types` is currently never populated; do not build the
context on it without first either populating it from `Init` items or removing
it. The existing plain-global emission in `emit_toplevel`/`mod.rs` reads
`ctx.plain_global_types` and falls back to `ctx.inits` declarations; this is
the first thing to normalize in the context slice.

## LLVM relationship — complement, never fight

LLVM already performs SROA, GVN, LICM, loop reduction, vectorization, and
register allocation. The C reference proves it: the C loop collapses to one
`xmm0` accumulator and spills only around `fprintf`.

The compiler's job is **fact exposure**, not register allocation:

**Complement LLVM**
- derive storage duration from cross-tick/contract/rollback/escape facts;
- emit ordinary SSA values for non-observable, non-escaping state;
- attach correct `noalias`, `readonly`, `argmemonly`, `memory`, alignment,
  range, and invariance facts;
- make effect boundaries explicit (distinct effect/plumbing context);
- emit a canonical loop with clean liveness and no artificial state alias.

**Fight LLVM**
- a frontend register-budget model deciding how many phis survive;
- manual phi caps or force-materialization to control spills;
- opaque/launder barriers that block SROA;
- manual accumulator formation when the input is already SSA;
- `plain`/`state` as a performance toggle.

The earlier phi-cap/global/launder prototypes failed precisely because they
fought the optimizer. The C reference never needed a register budget; it
exposed the loop.

### Revised design rule

> The compiler proves storage class/escape and emits SSA where the language
> permits. LLVM decides registers, spills, reductions, and vectorization.
> The compiler forces memory only where the language/ABI requires it.

`LoopShape` remains a **semantic** analysis: cross-tick/ABI/observable
boundaries, address-taken fields, contract visibility, required memory. It is
not a register allocator.

`PlainContext` is legitimate only as an **effect/alias boundary**: distinct,
`noalias`, correct effect summary; machine state stays SSA where possible.
If it only works by preventing SROA or forcing spills, reject it.

### C-shape probe

Before more context ABI work:

1. Emit a Briev loop with the same local/SSA shape as C, no print/state
   toggle.
2. Compare Briev IR/asm with C: one accumulator in a register, no
   per-iteration stack traffic.
3. Add the print boundary; identify exactly what changes.
4. If Briev IR is C-like but LLVM spills, fix metadata/effect facts. If not,
   fix storage/loop emission.
5. Record the IR/asm diff in the results file.

## Base truth 2026-10-20 — the C shape is already reachable

Minimal Briev SSA probes (no `%State`, no plain/state toggle) show:

- a pure local recursive accumulator compiles to a register-resident loop;
- adding a periodic `println!(acc)` keeps the accumulator in `xmm0` across
  the backedge and spills it only around `print_float`.

Therefore Briev IR + LLVM already have the C capability. The failure is in
**state/print materialization**, not in the optimizer.

### Revised primary direction

Add a derived **Local** storage class for bounded single-node state:

- fields whose only lifetime is inside one bounded node/loop, with no
  contract/rollback/export/async observation, are loop-local SSA values;
- they are committed to `%State` only at a real observable boundary (or not
  at all if the node never needs persistence);
- print/effect calls receive scalar/local values through the normal ABI, not
  a machine-state or plain-plumbing context that aliases the hot loop;
- no `plain`/`state` performance toggle, no register budget, no phi cap.

This is the C shape: locals + explicit effect boundary. `PlainContext` is
demoted to a fallback for genuinely address-taken/plain effect plumbing; it is
not the primary fix.

### Next experiment

1. Implement a minimal Local classification for bounded single-node fields
   (analysis only; no emission change).
2. Lower one test node's fields as loop-local SSA and compare the asm to the
   SSA probes/C.
3. If the shape matches, extend to `float_math`; if not, inspect the loop
   emission/commit boundary.
4. Only then revisit effect-context ABI.

## Rejected alternatives

- **Stateful print / stateful stdout** — loses queue_drain/interval_step.
- **Global state alone** — float_math still 2.71x; queue/interval worse.
- **Launder barrier alone** — float_math 2.61x.
- **`noinline` print alone** — float_math still ~2.7x.
- **Volatile accesses** — unrelated correctness workaround; forbidden for
  ordinary memory.
- **Shape/name/vocabulary heuristics** — forbidden by the compiler charter.
- **A strategy keyword** — the default must win; `vol` is for hardware
  volatility, not performance.

## Phases

### Phase 0 — experiment matrix (recorded + refined)

Already measured: stateless/stateful stdout, global state, launder, noinline.
Remaining before implementation:

0. Prototype the **plain context pointer** for the print family on 5aa:
   group stdout_buf/fill into a context, pass `ptr` to print functions, keep
   machine `%State` unescaped. Measure float_math + queue/interval.
   **Negative result 2026-10-20:** a `%PlainState` **module global** passed as
   `ptr @__briev_plain` (including from txns) leaves float_math at **2.87x**.
   The next prototype must use a **stack alloca `%plain` in main** and thread
   that pointer through the transaction + defn ABI. Global-context fallback is
   not equivalent to the fast alloca state shape.
1. Prototype the **state plan + capped phis** on the actual IR for float_math:
   - keep stateless print;
   - cap hot phis to the target budget + reserved accumulator/temps;
   - materialize the rest from an opaque state allocation;
   - link with the harness-exact command and compare.
2. Run the same prototype for queue_drain/interval_step; both must stay fast.
3. Decide hosted allocation: heap vs stack-escaped vs target-specific.
4. Record the budget formula and results here before landing.

### Phase 1 — frontend plan

- Add `StateLayoutPlan`/`LoopPressure` to `AnalysisResults`.
- Populate from `LoopShape` + type/register-class info + target settings.
- Unit tests for classification (hot/memory/invariant/vector).
- No codegen change yet.

### Phase 2 — opaque state base

- Add the target-driven state allocation mode for folded mains.
- Keep ISR global behavior.
- IR tests: no SROA-eligible `%State` alloca in the selected mode; no fake
  barrier/no-op call.
- Runtime suite + parity + determinism.

### Phase 3 — explicit phi/memory emission

- Backend consumes `StateLayoutPlan`.
- Emit HotPhi/Memory/Invariant accesses per plan.
- Reserve the accumulator/trace slot.
- Use vector phi groups where structurally legal and target-supported.
- Remove reliance on SROA for ranked fields.

### Phase 4 — measurement and calibration

- Full runtime suite, same machine/geometry; interleaved with baseline.
- Focus rows: float_math, float_math_nonzero, nbody_*, queue_drain,
  interval_step, telemetry_stream, pid_control, sweep_*, ws_frame.
- Calibrate the reserved-temp budget and any vector thresholds.
- Record before/after table in the results file.

### Phase 5 — permanent regression gate

Add `benchmarks/perf_sentinel.sh`:

- rows: float_math, queue_drain, interval_step, ws_frame, nbody_newton,
  telemetry_stream, pid_control;
- same harness, 5 iterations;
- ratio thresholds calibrated to catch 2–3x regressions without noise flakes;
- run on loop/state/print/backend changes and in CI when the runner is stable.

Add IR tests:

- high-pressure plan emits capped phis plus memory fields;
- low-pressure plan keeps the promoted set;
- state base is opaque in the selected mode.

## Acceptance

- `float_math` returns to the fast-era ratio (~0.9x) without stateful print.
- `queue_drain` and `interval_step` retain their ~0.45x/~0.7x wins.
- `nbody_*` and other high-field loops do not regress.
- No strategy keyword, no volatile, no name/pattern matching.
- `cargo test --lib`, bin tests, tier-1 E2E, conformance, contract census,
  parity, determinism all green.
- Perf sentinel demonstrates the guard against future regressions.

## Risks

- Opaque state allocation changes stack/heap behavior on embedded targets;
  make it target-config-driven and document the freestanding path.
- A bad pressure estimate can over-materialize fields and lose SROA wins;
  calibrate on the full suite, not just float_math.
- Vector phis can increase code size/register pressure if misused; gate on
  structural groups + target support.
- Parallel/reactor state must not share one allocation across threads; scope
  Phase 2 to folded single-node mains first, then extend with per-worker state.
- The old `phi-register-pressure-capping` plan was never fully landed; revive
  its register-budget logic with the current direct-SSA pipeline rather than
  re-adding the old InlineSsa/MemoryCounter paths.

## Result ledger

- `benchmarks/results/2026-10-20-performance-regression-bisect.md`
- `benchmarks/results/2026-10-18-fixture1-ws-frame.md` (ws_frame recovery)
- Backlog P2-8 in `docs/plans/2026-10-20-open-work-backlog.md`

## Undo

Land each phase separately. If the explicit state plan is refuted by the
Phase 0 prototype, return to the record and refine the pressure model; do not
fall back to a benchmark-specific behavior. Reverting Phase 2/3 restores the
current alloca/SROA path without touching the print chain.

## Local-SSA probe result (2026-10-20)

A temporary local-let `foreach` variant of the float loop keeps its
accumulators in registers (`xmm1`–`xmm3` in the exact probe) and spills only
on the print path; measured ~0.056s vs C ~0.043s (~1.3x), versus ~2.9x for the
state path. The probe is a shape/timing result, not a parity result.
Next implementation target: derive a Local class for bounded single-node
state and lower it as loop-local SSA.


## Local slice result (2026-10-20; diagnosis corrected 2026-10-21)

Local-field classification + dead exit-store elimination landed (`285356c0`).
`float_math` stayed ~2.9x. A forced per-field/literal-seed variant also stayed
~2.9x and was reverted: its body/guard/latch shape does not fold like the
local-`foreach` probe.

The 2026-10-21 diagnostic run corrects two earlier statements:

- `float_math` has **no swan song** (`term;`), and `post_hoist_groups=0`.
  Literal phi seeds and skipped local exit stores are already active. The
  "pending post-loop hoist forces `%State` seeds" explanation is withdrawn.
- The current countdown loop does **not** keep the accumulator in `xmm0`. Its
  hot loop reloads/stores `0x24(%rsp)` every iteration. The earlier
  register-resident observation belongs to the stateful-print fast era, not
  this shape.

Interleaved medians at `BOUND=50000000` (5 runs each, same host):

| Variant | Median | vs C |
|---|---:|---:|
| state countdown (current) | 0.1239s | 2.89x |
| local `foreach`, three-accumulator probe | 0.0559s | 1.30x |
| local `foreach`, exact 12-field probe | 0.0564s | 1.32x |
| C reference | 0.0429s | 1.00x |

The local probes are timing/shape probes, not parity results (their checksums
do not match C). The state countdown does match C numerically at 5M.

Root cause is the **loop materialization shape**: the countdown emits a
body → guard → latch batch loop with phi joins, and LLVM allocates the reduced
accumulator to a stack slot. The local `foreach` lowers to a single
header/body/latch with guard-taken effects; SROA/regalloc keeps its accumulators
in `xmm1`–`xmm3` and spills only in the guard-taken block. This is not state
traffic, Local classification, or post-hoist.

Do **not** wire the currently unused `emit_countable_batched_main` as the
shortcut: gated on `float_math` it emits dominance-invalid IR (e.g. `%icn83`,
`%t142`). It needs repair before use.

Next target: implement a single-header/single-latch loop shape for Local fields
(or the equivalent alloca/SROA shape without explicit batch phis), with the
effect block rejoining the latch once. Acceptance: output parity at print
boundaries and median <= 1.35x C at 50M, then the full suite.
