# Proof-consumption ledger

**Date:** 2026-10-26
**Status:** ACTIVE — instrumentation landed; guard row complete
**Rule:** proof is an optimization license. A proof that no codegen consumer
uses is a missing optimization, not a completed one.

## Goal

Make every proof family answer three questions:

1. What fact does the proof engine establish?
2. Which compiler pass consumes that fact?
3. What runtime evidence shows the consumption (removed branch/check/load,
   hoist, vectorization, or an explicit no-runtime-effect record)?

Source guards stay in the program. The compiler elides them when proven.

## Ledger schema

| Proof family | Producer | Fact | Codegen consumer | Runtime evidence | Test | Status |
|---|---|---|---|---|---|---|
| Proven dead guard | typechecker guard audit | `cond` true ⇒ `when !(cond)` unreachable | post-typecheck guard elimination | no `guard.then` / `llvm.trap` | `proven_guard_is_elided_from_ir` | DONE |
| Loop range / counter bounds | `foreach_range_facts`, counter induction | `idx ∈ [begin,end)`, `count ≤ idx-begin` | guard elimination; later bounds-check elision | removed guard branches | counter tests | PARTIAL |
| Constructor postconditions | call post-facts | `v.len == n`, matrix dims | guard elimination; bounds/dimension simplification | removed guards in A/B IR | app gate A/B | PARTIAL |
| Constant folding | typechecker constant table | `SYS_*`, literal relations | call gate and guard elimination | guard branches removed | fold tests | PARTIAL |
| Difference/linear facts | difference closure, Fourier–Motzkin | transitive bounds | guard elimination | removed guards | proof tests | PARTIAL |
| Bounds checks at accessors | contract gate | `i < len` | — | no IR consumer identified yet | — | OPEN |
| Alias/lifetime/independence facts | future proof passes | non-overlap, lifetime | vectorization/hoisting/reload removal | TBD | TBD | OPEN |

## Snapshot (2026-10-27)

`BRIEV_PROOF_LEDGER=1` on the recommender:

```text
PROOF_LEDGER family=guard proven=176 lowered=176 unconsumed=0
```

- All recorded assertion-guard proofs are consumed by the post-typecheck
  elimination pass. Non-trap control-flow guards are not treated as assertion
  proofs.
- `ivec_get`, `vec_get`, and `mat_get` IR contains **no runtime bounds check**
  (direct load). The contract proof therefore has no bounds-check consumer;
  this is recorded as `NONE (static contract; no runtime check)`, not as a
  missing optimization.
- Loop/constructor/difference facts currently have an **indirect** consumer:
  they discharge the guards that the elimination pass removes. They do not yet
  drive loop optimization, hoisting, or vectorization.

## Phases

1. **Instrument proof production. DONE for guards.** `BRIEV_PROOF_LEDGER=1`
   reports proven/lowered/unconsumed guard counts. Contract/bounds facts are
   still typecheck-only.
2. **Name consumers. IN PROGRESS.** Guard → elimination is named and asserted.
   Accessor bounds → `NONE (static contract; no runtime check)`. Remaining
   families still need explicit rows.
3. **Enforce consumption for guards.** A proven guard must be lowered; add a
   test that every recorded guard proof disappears from emitted IR.
4. **Audit remaining facts.** For each family with `NONE`, either add a general
   consumer or document why the fact is typecheck-only.
5. **Extend to runtime checks.** Consume proved bounds/dims to remove checks,
   not just guards. Prove the IR delta on the app corpus.
6. **Promote to a compiler diagnostic.** Optional dev mode:
   `proof-ledger` lists unused proofs and uncalled consumers.

## Acceptance

- Every proof family has a producer/consumer/evidence row.
- No proven guard remains in IR unless its consumer is explicitly `NONE` with
  a reason.
- `BRIEV_AUDIT_GUARDS=1` still reports source-level redundancy; the compiler
  elimination pass is the consumer, not manual deletion.
- Full gates, determinism, and IR A/B pass.
- New proofs cannot land without a ledger row and a consumer or `NONE` record.

## Non-goals

- Do not invent an optimization for every fact.
- Do not delete source guards as the optimization.
- Do not unsoundly consume facts with unresolved aliasing/effect questions.
- Do not encode algorithm-specific or type-specific optimizations.

## Validation log

- 2026-10-27: `BRIEV_PROOF_LEDGER=1` on `server.bv`: proven=176, lowered=176,
  unconsumed=0. A/B IR against the manual-removal branch: structurally
  identical.
- 2026-10-27: accessors have no runtime bounds check; guard elimination is the
  only current runtime consumer of proof facts.

## Lesson record

- **cue:** a proof fact is discharged.
- **lesson:** name its codegen consumer; if there is none, record the missing
  optimization instead of declaring the proof complete.
- **evidence:** guard elimination pass; manual removal vs restored source A/B
  produced structurally identical IR; guards previously emitted real branches.
- **confidence:** high.
