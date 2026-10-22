# Canonical effect-context ABI + FP evaluation-order policy

**Date:** 2026-10-21
**Branch:** `feat/canonical-effect-abi` (from `main` at `54a24cc4`)
**Status:** ACTIVE — design + implementation plan.
**Supersedes:** carry/escaped selection, E1 plain-context, narrow volatile fence.
**Checkpoint (rollback):**
- `effect-shape-research-2026-10-21` (merged research on `main`)
- `checkpoint-effect-carry-selection-2026-10-21`
- `checkpoint-effect-state-carry-2026-10-21`

## Goal

One loop/effect model that replaces all current shape toggles:

- Local loop state stays in SSA registers.
- Effect state lives in an explicit context passed through the ABI.
- Machine `%State` is written only at real observable boundaries.
- FP evaluation order is deterministic across codegen shapes.
- `float_math`, `queue_drain`, `matrix_pipeline`, and every mixed program
  reach their best measured ratio with zero benchmark-derived selectors.

## Design principles

1. **Derivation, not recognition** (Rule 23): the effect context is derived
   from the program's declared effect surfaces, not from loop shape
   classification. A field's storage class determines where it lives; the
   effect context is the union of all plain/effect fields reached by any
   effect call in the program.
2. **One stdout buffer, one ordering domain**: the context owns `stdout_buf`
   and `stdout_fill`; both escaped and stateless print variants write through
   it. There is never a second buffer.
3. **`noalias`/`nocapture` proof**: the effect context pointer must not alias
   or capture loop-local SSA. This is proven structurally: the context alloca
   is distinct from `%State` and from loop-local allocas; effect functions
   declare `nocapture` on the context parameter (they may write through it
   but do not store the pointer).
4. **No strategy keyword**: the compiler derives the context; no user-visible
   `effect` or `carry` keyword. The existing `vol` keyword remains for
   genuine MMIO/external-mutation intent only.
5. **FP order is a contract**: programs whose benchmark gate requires C
   parity preserve the guard-bearing evaluation order. The canonical loop
   emits the guard inside the hot path (not batched out), so fast-math
   reassociation cannot change the result across shapes.

## Architecture

### Effect context

```
%EffectCtx = type {
    ptr,    // stdout_buf
    i64,    // stdout_fill
    // ... other plain/effect fields derived from effect-call surfaces
}
```

- Allocated once per `main` (or per reactor).
- Passed as `ptr noalias nocapture align 8 %effect` to every effect/print
  defn that reaches a plain field.
- Machine `%State` no longer contains effect fields; no program-level demotion.
- Every loop (direct, temp-mediated, aggregate) uses the same context; no
  per-loop variation in effect memory.

### Effect ABI

- Effect/print defns take `ptr %effect` as a hidden parameter (after `%state`
  if they also need machine state, or standalone if they only use effect
  state).
- The context pointer is `noalias nocapture`: effect functions write through
  it but never store the pointer value itself.
- The existing `defn_takes_plain` / `compute_defn_needs_plain` analysis
  already identifies which defns reach plain fields; extend it to produce
  `defn_takes_effect`.
- Every effect-call site passes the context. The canonical pointer for the
  context is seeded once in the calling function (like `canonical_ptr`) and
  reused for all calls.

### Canonical Local loop

The loop emitter keeps Local fields as explicit SSA phis (already done in the
countdown engine) and the guard/effect call receives the effect context.
No effect-state carry phis; no `%State` demotion; no SROA-blocking escape.

Loop shape:

```
header:
  phi counter, local fields, ...
body:
  compute; counter++
  guard condition
  br guard, effect_block, header

effect_block:
  call print_float(ptr %effect, float %value)
  ...
  br header
```

This is the escaped shape's CFG structure (which preserves FP order) but
with `%effect` instead of `%State` as the effect memory. The accumulator
stays in `xmm` because the loop-local SSA is clean and the effect context
is `noalias nocapture`.

### FP evaluation-order policy

**Rule:** the guard/effect call stays inside the hot loop. It is never batched
into a pure inner loop. This preserves the FP evaluation order that C's
`-ffast-math` loop also preserves (the guard is a sequencing barrier for
reassociation in practice, even though `fast` technically permits it).

**Consequence:** the batched pure inner loop (E2 batch experiment) is
permanently rejected. It changed float output and cannot be used without an
explicit FP-order annotation.

**Future:** if a program needs a guard-free loop, the FP contract must be
made explicit (e.g., `strict_float` on the node, or a `Contract` obligation
that pins the evaluation order). This is a future language decision, not a
backend toggle.

## Implementation slices

Each slice is a commit with tests; no slice changes the default policy until
marked.

### S1 — Effect context analysis (frontend)

- Extend `compute_defn_needs_plain` to produce `defn_takes_effect` for every
  defn that reads or writes any plain/effect field through the context.
- Extend `AnalysisResults` with `effect_fields: Vec<String>` — the sorted
  union of plain fields reached by effect calls.
- Add `effect_ctx_type: String` — the LLVM struct type string for `%EffectCtx`.
- No codegen change; tests pin the analysis on float/queue/matrix fixtures.

### S2 — Effect context emission (backend)

- Allocate `%EffectCtx` in `main` after state init.
- Initialize from plain-field initializers (zero/false; non-literal falls back
  to globals for now, then migrates).
- Thread `ptr %effect` through every defn where `defn_takes_effect`.
- Effect defns load/store plain fields through `%effect` GEPs, not globals.
- Remove program-level `plain_lets.retain` demotion.
- Behind `BRIEV_DIAG_EFFECT_CTX=1`; default off.

### S3 — Canonical loop with effect context

- In the countdown emitter, when `BRIEV_DIAG_EFFECT_CTX=1`:
  - effect calls pass `ptr %effect` (from the context seeded in the caller);
  - no effect-state carry phis;
  - no program-level escaped fallback.
- The guard stays inside the hot loop (FP order preserved).
- Measure `float_math`, `queue_drain`, `matrix_pipeline`, mixed fixture.

**Gate:** if `float_math ≤1.0x` and `queue_drain ≤0.55x` and matrix unchanged,
proceed. If float stays >1.0x, investigate `noalias` facts; if queue
regresses, check that `%State` is truly unescaped.

### S4 — Mixed-program per-loop test

- `tests/tier1/mixed_effect_shape.bv` should now produce both loops at their
  best shape without program-level selection.
- Direct loop: accumulator in `xmm`, effect via context.
- Temp loop: SROA/vector, effect via context.
- Output multiset exact (integer counters).
- Guard pins per-loop decisions.

### S5 — Promote to default

- Remove `BRIEV_DIAG_EFFECT_CTX` gate; canonical path is the only path.
- Retire:
  - carry/escaped selection (delete `prefer_effect_carry`,
    `program_carry_mode`, `escaped_effect_cause` from the dispatch);
  - narrow volatile Ptr-parameter fence (if interprocedural provenance now
    sufficient; otherwise keep and document retirement gate);
  - E1 plain-context diagnostic (remove `BRIEV_DIAG_PLAIN_CONTEXT` code);
  - effect-state carry emission (`BRIEV_DIAG_EFFECT_CARRY`).
- Update guard to pin the canonical shape (no carry/escaped lines).
- Full suite + timing guard + research report update.

### S6 — FP contract formalization (future)

- Language-level decision: how do programs declare strict FP?
- Not part of this implementation; record as a follow-up plan.

## S2/S3 experiment result (2026-10-21)

Implemented behind `BRIEV_DIAG_EFFECT_CTX=1`: stack `%EffectCtx` alloca,
`ptr noalias nocapture %effect` defn parameter, effect-context plan consumption,
no program-level escaped demotion, and guard-bearing countdown loops.

The gate **fails** for the carry-winning direct loops:

| Benchmark | `%EffectCtx` | current carry selection | C |
|---|---:|---:|---:|
| float_math_nonzero | 1.25x | 0.97x | 1.00x |
| kalman_filter_runtime | 1.03x | 0.86x | 1.00x |
| sweep_sparse | 1.32x | 0.98x | 1.00x |
| float_math | ~0.91–0.99x | 0.89–0.96x | 1.00x |
| queue_drain | 0.51x | 0.50x | 1.00x |

Interpretation: the explicit effect context removes the `%State` escape, but
by itself it restores the **stateless SROA/vector shape**, which is wrong for
the carry-winning loops. It is the E1 tradeoff again, now with a clean ABI.
The canonical ABI is necessary but not sufficient; the Local-loop
materialization/carry shape must coexist with it.

The experiment also required placeholder `%effect.intf` values in dead txn
copies (the direct-SSA main replay owns the real alloca). That is acceptable
only for a gated experiment; a production version must thread the context
through txn/task/async ABIs explicitly.

Status: **not promoted**; default behavior unchanged. This branch records the
negative result and the ABI scaffolding for the next attempt (Local-loop
materialization + context), or a decision to keep the interim selector.

## S3b: context + effect-carry (2026-10-21)

Attempted the obvious combination: keep `%EffectCtx` and also carry the
context-owned stdout buffer/fill as loop-carried SSA phis (same shape as the
interim carry experiment, but reading/writing `%EffectCtx` instead of globals).

Result: **still fails the S3 gate.**

| Benchmark | context+carry | current carry selection |
|---|---:|---:|
| float_math_nonzero | 1.25x | 0.97x |
| kalman_filter_runtime | 1.17x | 0.86x |
| sweep_sparse | 1.35x | 0.98x |

Conclusion: the carry win is not merely “effect values are loop-carried”.
It depends on the **shared/global effect memory shape** interacting with LTO.
Moving the same values into a context alloca changes the shape enough that
carrying phis does not restore it. Therefore `%EffectCtx` cannot simply be
combined with carry as-is.

This tightens the long-term decision:

- keep the interim carry/escaped selection as the production policy; or
- design a loop-local effect-state model that reproduces the carry shape
  without the context alloca (the remaining canonical research question).

No further speculative ABI combination is warranted without a new measured
hypothesis.

## Experiments / measurements

Every slice runs:

| Gate | Threshold |
|---|---|
| `float_math` | ≤1.0x, ideally 0.9–0.97x |
| `queue_drain` | ≤0.55x |
| `interval_step` | ≤0.75x |
| `matrix_pipeline` | ≤1.0x |
| `telemetry_stream` | ≤1.02x |
| `pid_control` | ≤1.02x |
| `sweep_sparse` | ≤1.0x (carry winner must not regress) |
| `sweep_mid` | within 2% of current |
| mixed fixture | output multiset exact; per-loop shapes correct |
| Full runtime suite | all MATCH, no >2% regression |
| `cargo test --lib` | green |
| Quick + timing guards | PASS |
| LTO asm | no per-iteration `vmovss` reload for float |

## Risks / mitigations

| Risk | Mitigation |
|---|---|
| Effect context pointer alias blocks SROA (like E1) | `noalias nocapture` on every effect defn; alloca is distinct from `%State` and locals; test interval/matrix at S3 gate |
| FP output changes | guard stays in hot loop; batched inner loop permanently rejected |
| `defn_takes_effect` misses a surface | start from the union of `plain_lets` reached by effect calls; audit with `BRIEV_DIAG_EFFECT_CTX=1` on all benchmarks |
| Exports need effect context | exported defns with plain fields keep the global path until export ABI migration (S5 scope) |
| E1 regressions recur | `noalias` on effect param; alloca distinct from `%State`; test interval/matrix at S3 |

## Rollback

The branch `feat/canonical-effect-abi` is isolated. If S3 fails:

```bash
git switch main
```

or keep the branch and iterate. The interim carry/escaped selection on `main`
is the fallback and remains production-ready.

## Commit log

| Date | Commit | Change |
|---|---|---|
| 2026-10-21 | _this commit_ | plan doc |
| _pending_ | S1 effect context analysis |
| _pending_ | S2 effect context emission (gated) |
| _pending_ | S3 canonical loop + context (gated) |
| _pending_ | S4 mixed per-loop test |
| _pending_ | S5 promote to default |
