# `checkfree` — the subscript-bounds obligation

**2026-11-20.** Landed on `main` as `03f3d6c48` … `278c0b9cc` (wave-6 §7.14,
Goose-borrowed). Owner-selected design: **guard-based discharge**. Supersedes
the first attempt, whose P1 rested on a false premise (recorded in §1).

## 1. Why the first design was abandoned

The original plan discharged a subscript from the `IndexIn` type alone
(`u32 in xs`). Implementing it exposed a soundness bug in the index type as
landed (`4d0a8d7f5`):

```briev
checkfree defn pick(xs: List<Int>) -> Int {
    let i: u32 in xs = xs.Count#();   // i == 3 for a 3-element list
    term xs[i];                        // subscript at the BOUND, not an index
};
```

`brievc check` answered **OK** — the obligation certified it — and running it
printed ` 00`: an out-of-bounds read, no trap.

`Count#()` is the element **count**; valid indices are `0 .. count-1`. A value
constructed from it ranges `0 ..= count`, so it is a **bound**, not an index,
and `xs[bound]` is one past the end. Arithmetic on it degrades to the plain
carrier (`i - 1` is `Int`), so the neighbour form could not be discharged
either. The implementation was reverted rather than shipped; SPEC §15.4a was
corrected in `5fc20f822` and the obligation re-scoped to guards.

The lesson: an obligation that can certify an out-of-bounds read is worse than
no obligation, because it converts a runtime bug into a false proof.

## 2. The design that landed

A `checkfree` defn must prove every subscript in its **body** from the facts
that dominate it. Two halves, both required:

- **F1 — upper bound.** A dominating fact `<idx> < <count(coll)>` (or
  `<idx> <= <count(coll)> - 1`, plus the mirrored `>`/`>=` spellings),
  matched **structurally** on the index expression as written.
- **F2 — lower bound.** A dominating fact `<idx> >= 0` (or `0 <= <idx>`), **or**
  `<idx>` of type `IndexIn(coll)` for the same collection — its construction
  rule gives `0 ..= count`, supplying the lower half and only the lower half.

A literal index into a fixed `Vector<_, N>` is the other discharge. A literal
into a **dynamic** collection is rejected: the length is not statically known,
so a small number proves nothing.

### What it reuses, rather than reinvents

| Piece | Where |
|---|---|
| guard-fact scoping (`facts_of` push/pop on the body only) | `src/typechecker/mod.rs` `Statement::Guarded` arm |
| `fallthrough_facts` (the negation after a terminating guard) | same arm |
| `callgate::facts_of` — the actual fact semantics | `src/proof_engine/callgate.rs` |
| exhaustive expression traversal (`ast::visit::walk_stmts`, no `_` arm) | `briev-verify/src/ast/visit.rs` |

The obligation is therefore a **consumer** of the proof engine, not a second
opinion about it: every future improvement to guard facts (interval reasoning,
new fact shapes) upgrades `checkfree` for free. That is why this is the healthy
long-term choice rather than a bespoke analysis.

### Cost of a structural proof, stated plainly

The guard must name the exact index expression. `when i < xs.Count#() { xs[i] }`
discharges; the last-element read needs `when i - 1 < xs.Count#() { xs[i - 1] }`.
`when i > 0` alone does **not** suffice — it says nothing about the upper end.
Interval reasoning that would make it suffice belongs in the fact engine (where
it generalizes every consumer), and is a recorded follow-up.

## 3. Invariants and the tests that pin them

| # | Invariant | Test |
|---|---|---|
| 1 | `checkfree` on anything but a `defn` is an error | `checkfree_rejects_non_defn_target` (parser; verified via `brievc check`) |
| 2 | A subscript with no dominating N-bound is rejected | `checkfree_rejects_unbounded_subscript` |
| 3 | `0 <= i` + `i < xs.Count#()` discharges `xs[i]` | `checkfree_accepts_guarded_subscript`, `tests/tier1/checkfree_ok.bv` |
| 4 | A guard bounding `i` against `ys` does not discharge `xs[i]` | `checkfree_rejects_mismatched_collection_guard` |
| 5 | An upper bound ALONE is not a proof | `checkfree_rejects_missing_lower_bound` |
| 6 | `i > 0` alone does not discharge `xs[i - 1]` | `checkfree_rejects_lower_bound_only_guard` |
| 7 | A guard's fact does not leak past its block | `checkfree_rejects_guard_out_of_scope` |
| 7a | A NON-terminating guard records no fact for its successor, even when the negation would prove the missing half | `checkfree_rejects_fact_from_nonterminating_guard` |
| 7b | A terminating guard's negation DOES dominate its successor | `checkfree_accepts_fallthrough_negation` |
| 8 | `IndexIn` supplies F2 only, never F1 | `checkfree_index_type_supplies_lower_bound` |
| 9 | A subscript nested in a call argument is still seen | `checkfree_sees_nested_subscripts` |
| 10 | A literal into a dynamic collection is rejected; into a fixed Vector accepted | `checkfree_rejects_literal_on_dynamic_coll`, `checkfree_accepts_vector_literal` |
| 11 | Defns without the modifier are unaffected | `checkfree_is_opt_in` + full `cargo test --lib` |
| 12 | Emitted IR is byte-identical with and without the annotation | `tests/checkfree_emission.rs` |
| 13 | The word is documented (vocab gate) | `vocab::tests::master_syntax_reference_covers_every_keyword_and_intrinsic` |

**Mutation evidence (three axes).** Dropping F1 breaks the mismatched-collection
and index-type reject tests. Dropping F2 initially broke *nothing* — the suite
stayed green — which is why `checkfree_rejects_missing_lower_bound` exists; with
it in place, dropping F2 fails. Making the fallthrough facts unconditional (the
one-line "simplification" that looks harmless) initially broke nothing either;
`checkfree_rejects_fact_from_nonterminating_guard` now catches it. A test that
cannot distinguish a missing rule is not a test of that rule.

**Interpreter parity.** `brievc test tests/tier1/checkfree_ok.bv` pins the same
golden (`8|-1`) as the native path, and a deliberately wrong golden is caught
(Rule 5: the interpreter is the reference).

## 4. Scope boundaries (documented, not implied)

- **Intra-procedural** — a call is not walked into; the callee answers for its
  own subscripts.
- **Body only** — contract `[pre]`/`[post]` subscripts keep the contract
  prover's own fact set; the body's guards do not dominate them.
- **`foreach` needs no proof** — it never subscripts.
- **Analysis-only** — no AST change reaches the backend, so no IR delta and no
  runtime cost. Lowering the annotation into a runtime bounds check would make
  the promise a pessimization *and* hide that the proof failed.

## 5. Follow-ups (not part of this landing)

1. **Interval reasoning over the index** in the fact engine —
   `i > 0` + `i <= count` ⇒ `xs[i-1]` safe. Generalizes every consumer.
2. **A true index type** (`in` = index, `upto` = bound, Goose's split) needs a
   total construction source for a growable collection, which does not exist
   without a fallible constructor.
3. **`checkfree` on generic defns** after monomorphization — the obligation is
   currently checked on the declared body only.

## 6. Gate record

`cargo test --lib` 3964 passed · `scripts/gate.sh quick` pass ·
`benchmarks/ll_determinism_check.sh --quick` ALL DETERMINISTIC · tier1 corpus
(golden-pinned) pass · Praetor clean on every new function.
