# PARKED PROPOSAL — tail expressions, when-expressions, P-gated term

**2026-10-15, late session.** Decisions BELOW WERE MADE TIRED and are
recorded as a PROPOSAL, not a commitment. Re-confirm or override with a
rested mind before implementing anything in this file. Nothing in flight
depends on it: dir.bv landed with the current-safe idiom (guard-the-body),
and bare-`term` semantics are untouched.

## Context

Investigating the dir.bv print_all crash surfaced that bare `term;` means
"convergence checkpoint — continue" everywhere (interpreter eval.rs:2447,
backend emit_stmt.rs:1782), while the maintainer's mental model is:

> Term means return if, and only if the postcondition is met at the point
> term is called.

## Proposal 1 — P-gated term (semantic)

`term` = evaluate the enclosing item's postcondition at the term site:
met ⇒ return; unmet ⇒ checkpoint (continue / body re-runs).

| Context | Today | Under the rule |
|---|---|---|
| `term <v>` in a callable txn | store → gate → done/loop | unchanged (the gate IS the rule) |
| `term <v>` in a defn | unconditional return (static P) | unchanged |
| bare `term;` in a callable txn | no-op, falls to body end | jump to the gate now (`br %post`) |
| bare `term;` in a defn | no-op, continues | return (no P ⇒ trivially met) |

Open sub-decision (unconfirmed): bare `term;` in a defn WITH a real
postcondition — defns emit no runtime gate, so the reading was "return;
P is the static contract's job". Needs a rested yes/no.

Implementation sketch (if confirmed): interpreter tracks the enclosing
item's contract during eval; `Term(None)` evaluates P (unresolvable P ⇒
unmet ⇒ checkpoint). Backend: defn bare-term emits a real terminator
(keep the member-frame queue_drain workaround); txn bare-term emits
`br label %post`. Warning text + ssa/emit_stmt rationale comments
rewritten. Tests: interpreter defn-guard-returns, txn mid-body P-gate,
unmet-P-continues; E2E = print_all as originally written.

## Proposal 2 — tail expressions (syntax)

Rust-style: the body-final expression without a semicolon is the return
value — pure parser desugar to `Statement::Term(Some(e))` (zero
downstream changes; P-gating applies identically).

```
defn lshr64(x: Int, k: Int) -> Int {
    when k == 0 { term x; };
    (x >> k) & ((1 << (64 - k)) - 1)     // ≡ term (...);
};
```

- Body-final position only; mid-body missing `;` stays an error
- Value tail in a Void body ⇒ typecheck error (Rust `()` discipline)
- `term` SURVIVES as the early-return/convergence keyword (guards, txn
  bodies); tails are the normal-case sugar; `term!`/`term! ->`/
  `endprogram` unchanged

## Proposal 3 — when-expressions (syntax, bigger)

Tired-session answer included guard-block tails (making `when` an
expression, Rust-`if`-style). This is a real language step (expression
position for `when`, value-typed arms, exhaustiveness questions) —
explicitly deferred even if Proposals 1–2 are confirmed. dir.bv's
`print_all` idiom (`when i < n { … }` guarding the body) works without
it.

## Rejected along the way (kept for the record)

- `resident;` bare keyword — duplicates `when true { reside; };`
- `declare` top-level unifier — loses to existing spellings (see
  lifecycle-laws plan); the real fix is the StateDecl parser hygiene
- Runtime settle-exit — changes semantics; the lifecycle warning carries
  the information instead

## Landing order (when confirmed)

1. Proposal 1 (P-gated term) — interpreter first, then backend, warning,
   docs
2. Proposal 2 (tails) — parser desugar + typecheck Void rule + docs
3. Flip dir_smoke's print_all to the natural idiom (becomes the
   regression test)
4. Proposal 3 — separate plan doc if/when confirmed
