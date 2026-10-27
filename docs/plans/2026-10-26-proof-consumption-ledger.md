# Proof-consumption ledger

**Date:** 2026-10-26
**Status:** NEXT WORKSTREAM
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

## Phases

1. **Instrument proof production.** Emit a structured record whenever a proof
   obligation is discharged: family, goal, facts used, verdict, and source
   site.
2. **Name consumers.** Map every proof family to a pass/IR transformation.
   `NONE` is a valid recorded value; it flags an optimization gap.
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

## Lesson record

- **cue:** a proof fact is discharged.
- **lesson:** name its codegen consumer; if there is none, record the missing
  optimization instead of declaring the proof complete.
- **evidence:** guard elimination pass; manual removal vs restored source A/B
  produced structurally identical IR; guards previously emitted real branches.
- **confidence:** high.
