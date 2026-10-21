# Performance regression bisect — fast era vs current

**Date:** 2026-10-20
**Machine:** same host, 5 iterations, BOUND=50000000, harness
`benchmarks/build_and_bench.sh`.

## Trigger

The Oct 18 fixture-1 table showed several rows slower than the Oct 17
"post-stability-runtime-fixed" fast era. The question was whether the
provenance patch caused it.

## Bisect result

| Commit | float_math Briev | C | Ratio |
|---|---:|---:|---:|
| `3c15268a` fast era | 0.0423s | 0.0449s | **0.94x (Briev wins)** |
| `5aa90185` state/plain | 0.1260s | 0.0464s | **2.72x (C wins)** |
| current (`394ee57c`+ws fix) | ~0.126s | ~0.046s | ~2.74x |

The regression first appears at **`5aa90185` (state/plain storage classes)**.
It is not caused by the raw-pointer provenance patch; `float_math` has no
`Ptr` and the provenance patch does not touch its state fields.

## What the IR shows

At both commits `@txn_tick` uses the same countdown-loop shape and the same
13 loop-carried Float phis (FMUL/FADD fast). The main differences are:

- fast `%State` is 88 bytes with `stdout_buf`/print-chain fields present;
  state/plain `%State` is 72 bytes (the print chain is stateless);
- fast calls `@print_float(ptr %state, float)` / `@put_char(ptr %state, i64)`;
  state/plain calls `@print_float(float)` / `@put_char(i64)`;
- field indexes shift accordingly.

The hot loop is structurally the same. The slowdown is therefore in how the
stateless print chain / state layout interacts with the loop optimization, not
in the matrix arithmetic. It needs asm-level attribution before a fix; do not
"fix" it by restoring `ptr %state` to the print chain blindly.

## Other fast→current rows (same historical tables)

| Benchmark | Fast `3c15268a` | State `5aa90185` | Current |
|---|---:|---:|---:|
| float_math | 0.90x | 2.01x | 2.74x |
| telemetry_stream | 0.86x | 1.03x | 1.48x |
| pid_control | 0.99x | 1.23x | 1.23x |
| sweep_sparse | 1.30x | 1.44x | 1.49x |
| queue_drain | 1.35x | 0.45x | 0.47x |
| interval_step | 1.23x | 0.61x | 0.72x |
| ws_frame | — | — | **3.50x → 1.48x fixed** |

Two facts matter for the fix: state/plain **improved** queue_drain and
interval_step (the print-chain escape fix), and it **hurt** the float/stream
lane. A fix must preserve the former.

## Next

1. Compare `3c15268a` vs `5aa90185` at the linked-binary asm level for
   `@main`/`@txn_tick`: spills, register pressure, loop body size, FMA.
2. Try a minimal change on `5aa90185`: keep the print chain stateless but
   verify whether `%State` SROA/promotion or call inlining causes the gap.
3. Apply the general fix (no shape matcher, no strategy keyword) and re-run
   the fast-era table interleaved.

## ws_frame (already fixed)

The 3.50x ws_frame regression was a separate, self-inflicted contract-migration
artifact: per-byte runtime guards on `get_u8`/`ws_payload_byte` plus a
benchmark-level guard. Foreach range facts + a proven direct `Load#` removed
them; measured **1.48x**, better than the old 1.58x.


## Controlled experiment 2026-10-20 — stateless print is the trigger

On the `5aa90185` worktree, keep everything else identical but exclude
`stdout_buf`/`stdout_fill` from plain classification (they stay `%State`
fields). Result:

| Benchmark | 5aa stateless print | 5aa stateful print |
|---|---:|---:|
| float_math | 2.72x | **0.92x** |
| queue_drain | 0.45x | 1.44x |
| interval_step | 0.61x | 1.46x |

So the tradeoff is confirmed: **the stateless print chain recovers
queue_drain/interval_step but causes the 3x float_math spill**. The asm shows
the float accumulator held in `xmm0` with stateful print, but spilled to
`0x14(%rsp)` every iteration with stateless print. The state alloca is no
longer passed to print, SROA promotes the 13 float fields, and register
pressure spills the trace accumulator. The fix must be a general
register-pressure-aware state/loop strategy, not a blanket return to stateful
print (which loses the queue/interval wins) and not a source workaround.

This is now the concrete P2-8 root cause. Next: implement the cost model in the
frontend-driven loop/state strategy, or a loop-engine memory/register choice
for high-field float loops that keeps the accumulator in a register while
leaving the bulk fields in memory.


## Additional experiments 2026-10-20 — what does NOT fix it

All on the `5aa90185` worktree, same harness:

| Variant | float_math | queue_drain | interval_step | Verdict |
|---|---:|---:|---:|---|
| stateless print + stack state (current) | 2.72x | 0.45x | 0.61x | baseline |
| stateless print + **stateful stdout** | **0.92x** | 1.44x | 1.46x | tradeoff, rejected |
| stateless print + **global state** | 2.71x | 1.68x | 1.67x | fails both |
| stateless print + **launder state base** | 2.61x | 0.49x | 0.69x | fails float |
| stateless print + **noinline print functions** | ~2.7x (0.125s) | — | — | fails |

Conclusions:

- It is not simply SROA/state escape: global state and a launder barrier do not
  recover float_math.
- It is not print-function inlining: `noinline` on print_float/put_char/
  print_int/print_str does not recover.
- It is not "make stdout stateful" as a fix: that loses the queue/interval
  wins.
- The fast codegen keeps the trace accumulator in `xmm0`; the slow codegen
  spills it to `0x14(%rsp)` every iteration. The trigger is the combination of
  the stateless print call shape and clang's SROA/register allocation, not one
  isolated flag.

Therefore the long-term fix must control the loop's state/register materialization
explicitly from the frontend/target model, not rely on clang's SROA decision.


## Phi-cap prototype 2026-10-20 — capping alone does not recover float_math

On the `5aa90185` worktree, cap countdown-loop Float phis to the target
budget (11 of 12 float fields) and make the state base opaque with
`llvm.launder.invariant.group`; excluded fields go through `%State`
load/store. Result:

| Metric | Value |
|---|---:|
| Briev | 0.1238s |
| C | 0.0475s |
| Ratio | **2.61x** |
| Correctness | MATCH |

The loop still spills the trace accumulator. Capping phis + opaque state is
**not sufficient**. The only variant that recovered float_math was stateful
stdout, where the print calls receive a **state/context pointer** instead of
addressing bare globals. That shifts the hypothesis to the plain/effect
context ABI, not just machine-state pressure.

## Refined hypothesis — plain context pointer

The state/plain split currently lowers plain lets to individual module
globals and print functions read/write those globals directly. The fast
stateful experiment gave print functions a `ptr %state` argument; the slow
variant uses bare `@stdout_*` globals. A context pointer is a different
machine-level shape (pointer argument + GEPs) even when the state alloca is
opaque.

Next experiment: group plain/plumbing lets into one **plain context** and
pass its `ptr` to the functions that use it (print family first). Keep machine
`%State` unescaped. If that recovers float_math and preserves queue/interval,
the long-term architecture is:

- machine state: explicit pressure/plan (phi/memory) as designed;
- plain/effect state: separate context pointer with its own ABI;
- no bare global mutation in hot effectful calls;
- no pattern/name matching: the split is by declared/inferred storage class.

This is now the primary Phase 0 hypothesis. The phi-cap prototype remains a
useful negative result and a guardrail for the eventual state plan.


## Further isolation 2026-10-20 — pointer ABI, field count, and stdout writes

| Variant | float_math | Verdict |
|---|---:|---|
| stateless print + stack state | 2.72x | baseline |
| force **all defns** to take `%state`, stdout still globals | 2.93x | pointer ABI alone: no |
| add two unused Int state fields, stdout still globals | 2.72x | field count/layout: no |
| stdout fields in `%State` (stateful stdout) | **0.92x** | only recovering shape |

The recovering shape is specifically **the print path writing through state
memory**, not merely a state pointer argument, not field count, and not print
inlining. Bare `@stdout_*` global writes keep the slow codegen even when every
defn takes `%state`.

This strengthens the **plain/effect context** hypothesis: give the print
path a real context pointer whose fields it reads/writes (not the machine
`%State` and not bare globals). The context ABI carries the memory shape that
the fast stateful-print variant accidentally exercised, while keeping machine
`%State` unescaped.

The next prototype must implement the context, not simulate it with an unused
pointer, dummy fields, or global state.


## Plain-context prototype 2026-10-20 — global context pointer is not enough

Prototype: `%PlainState` aggregate + `@__briev_plain` global, print functions
take `ptr %plain` (or `ptr @__briev_plain` from txns), GEP accesses. Machine
`%State` remains unescaped.

| Metric | Value |
|---|---:|
| float_math | **2.87x** |
| Correctness | MATCH |
| queue/interval | not rerun (global-context path failed the primary check) |

A **global plain context** passed as a pointer does not reproduce the fast
stateful-print shape. The fast experiment used an **alloca-backed** `%state`
context; the global-context variant does not. Next prototype must allocate
`%plain` on the stack in main and thread it through the transaction/defn ABI
(not fall back to `@__briev_plain` inside txns). If an alloca context also
fails, the plain-context hypothesis is refuted and the work returns to explicit
loop accumulator/state materialization.


## Base-truth probe 2026-10-20 — Briev/LLVM can produce the C shape

Two minimal Briev SSA probes (no `%State`, no plain/state toggle):

1. **Pure local recursive accumulator** (`step(n,i,acc)`), no print:
   - LLVM produced a register-resident accumulator (and vectorized the
     add loop); no per-iteration stack traffic.
2. **Same loop with a periodic `println!(acc)` inside it**:
   - the accumulator stays in `xmm0` across the backedge;
   - it spills only around `print_float`:
     `vmovss %xmm0, 0x4(%rsp)` before the call, reload after.

So Briev IR + LLVM already have the C capability: expose ordinary SSA locals,
keep the hot accumulator in a register, spill only at the effect boundary.

The failures are therefore in **state/print materialization**, not in Briev's
IR or LLVM: `float_math` keeps its 13 loop fields in `%State`/plain-global
form and the print path/ABI disrupts the hot accumulator's live range.

This reframes the fix: add a derived **Local** class / loop-local
scalarization for bounded single-node state, instead of toggling print storage
or building a context ABI to trick register allocation. PlainContext remains
only a possible effect/alias boundary if local scalarization alone cannot
cover a required effect path.

(An additional 13-float recursive probe was inconclusive — the periodic print
was hoisted/optimized differently and the result was not a reliable loop-shape
measurement; do not cite it as evidence.)


## Local-SSA loop probe 2026-10-20 — the target shape is faster

A temporary Briev variant of `float_math` uses ordinary local lets in a
`foreach` loop (no `%State`, no plain/state toggle), with the same periodic
`Print#` boundary:

| Variant | Briev | C | Ratio |
|---|---:|---:|---:|
| current state path | 0.123s | 0.043s | ~2.9x |
| local-SSA foreach probe | **0.057s** | 0.043s | **~1.33x** |

The local probe's asm keeps the accumulator in `xmm0` and spills only around
the print call — the C shape. The probe's math was simplified for isolation,
so its printed checksum is not a parity result; the timing/shape comparison is
the evidence.

Conclusion: lowering bounded single-node state to **loop-local SSA** is the
promising path. It is not a register-budget heuristic and does not rely on
context/global tricks.


## Local-field classification + literal init (2026-10-20) — no float gain yet

Implemented the first Local slice:

- classify fields written by a loop whose only references are inside that
  txn's body/guards (contracts/other txns/defns/top-level expressions keep them
  machine state);
- loop-local fields skip the post-loop `%State` store;
- loop-local fields with literal initializers seed their phis from the literal
  instead of a `%State` load.

Result: `float_math` **2.94x** — no improvement. The IR now has literal phi
seeds and no exit stores, but LLVM still does not collapse the independent
updates. The next variable is the **countdown loop shape itself** (body →
cold guard block → latch phis), not the state commit. The local-`foreach`
probe (0.057s) used a simpler SSA loop and did fold.

Conclusion: keep the Local classification/elimination (it is semantically
sound and removes dead state traffic), but the performance fix needs an SSA
loop strategy for local-heavy loops, not more state-materialization work.


## Forced per-field/local SSA attempt 2026-10-20 — still no float gain

Tried forcing the per-field (non-countdown) loop for local-heavy loops, plus
literal phi seeds and no local exit stores. `float_math` stayed at
~2.9x/2.65x. The per-field path still initializes local phis from `%State`
because a pending post-loop hoist is present, and its body/guard/latch shape
does not fold like the local-`foreach` probe.

The attempt was reverted rather than landed: it adds complexity without a
measured win. The validated Local slice (`285356c0`: classification + dead
exit-store elimination) stays. Next target is the post-hoist/guard split and
the loop shape itself — the simple SSA probe remains the reference.
