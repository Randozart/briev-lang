# `$txn` Convergent Flavor — fix and keep (C6)

**2026-09-22.** Decision on `$txn` after C1–C5 proved the compute-and-emit
model. The original plan asked: adopt `$txn` as the convergent-loop flavor
or delete it, "decided once `foreach`-over-rest is proven." C1–C5 proved
it — and the answer is **keep, as the unknown-count loop**.

## Why keep (the defining case)

The user's question — *"what if the total elements are unknown?"* — is the
decisive distinction:

| Construct | Iteration count |
|---|---|
| `foreach` over rest (C1) | known at call site (arg-list length) |
| `EmitNode$` in a `foreach` (C5) | known (the unrolled list) |
| `$txn` convergent loop | **NOT known** — repeats until `[post]` holds, decided by the computation at compile time |

There is NO comptime `while` in the fold. A `$defn` composite cannot
express "repeat until converged" — `foreach` needs a finite list. `$txn`
is the only mechanism for compile-time convergence:

```
$txn build_all(mem: Int) [mem < TARGET][mem >= TARGET] {
    mem = mem * 2;        // how many doublings? unknown until it converges
};
```

## Scope (confirmed with the author)

- **Top-level `$txn` declaration** — KEEP (already works:
  `parse_compile_time_txn`, `definitions.rs:3496`).
- **Inline INVOCATION** — KEEP (call `$txn name(args)` from a `$(Stage)`
  block; `eval_compile_time_fn` → `FnDef::Txn`, `eval.rs:328`).
- **Inline DECLARATION** — REMOVE. `Statement::InlineTxn` + the
  `$txn`-statement parser (`parse_inline_txn`, `statements.rs:542`) are
  deleted. `$txn` declares at top level only; a body never declares one.

## Implementation

1. **Remove inline declaration**
   - `src/ast/top.rs`: delete `Statement::InlineTxn(Transaction)` +
     the diff/ordering guard arm.
   - `src/parser/statements.rs`: delete `parse_inline_txn` and the
     `check_identifier("$txn")` dispatch arm.
   - Strip `Statement::InlineTxn` arms from every consumer: reactor,
     dataflow, annotator, llvm (helpers/mod/dispatch), capabilities,
     interpreter, macros/selection, display, canonical.
   - `dispatch.rs` `emit_inline_txn_body` is the `inline` MODIFIER path
     (a different feature — the `inline` annotation on a txn, not the
     `$txn` statement) — it STAYS.

2. **Allow `...` rest on `$txn`**
   - Remove the C6 placeholder rejection in
     `parse_compile_time_txn` (`definitions.rs:3520`).
   - `FnDef::Txn` carries the rest name (already on `Transaction`? add
     `variadic_param` to `Transaction` or to `FnDef::Txn`'s stored struct —
     check how `$defn`'s rest is threaded and mirror it).
   - The convergent evaluator binds trailing args as the rest list and
     iterates until convergence.

3. **Tests** (stage-evaluator level, `src/macros/eval.rs` or a dedicated
   module):
   - `$txn` converges: `[x < 10][x >= 10] { x = x * 2; }` → terminates.
   - `$txn` timeouts: a non-converging loop errors with the max-iter
     message.
   - `$txn` with `...` rest: converges over an unknown-size set.

4. **Emission wiring (stretch)**: verify a `$txn` body can carry
   `EmitNode$` (convergent topology of unknown size). If the stage
   evaluator's `NavValue::TopLevel` path reaches `expand_top_level`'s
   hoisting, document the seam; otherwise leave as a follow-up note.

## Docs
- `docs/plans/2026-09-22-unified-metaprogramming-layer.md` — C6 entry
  updated from "adopt or delete" to "keep as the convergent flavor".
- SPEC §18 — document `$txn` top-level-only declaration + convergent
  semantics (repeat-until-`[post]`, max-iteration bound).
- `docs/architecture/macro-system.md` — the `$txn` section corrected
  (no inline declaration).

## Undo
Restore `Statement::InlineTxn` + `parse_inline_txn` from git history if a
future use for inline declarations appears.