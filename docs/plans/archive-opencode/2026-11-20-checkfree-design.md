# checkfree — the bounds-proof obligation (wave-6 W6-2 follow-up)

> **STATUS 2026-11-20: the P1/P2 design below is HISTORICAL — it was unsound.**
> P1 rested on the false premise that an `IndexIn` value licenses a subscript;
> `Count#()` is the element COUNT, so it ranges `0..=count` and certifies an
> out-of-bounds read. Evidence + the three resolutions are in the STOP record at
> the end of this file; SPEC §15.4a was corrected in `5fc20f822`.
>
> **The live design is “Confirmed design — guard-based discharge” immediately
> below. Everything from `## Syntax` to the STOP record is superseded.**

---

## Confirmed design — guard-based discharge (owner-selected 2026-11-20)

A `checkfree` defn must **prove every subscript in its body** from the facts
that dominate it. There is no new type spelling and no new proof machinery:
the obligation is discharged by the SAME guard-fact engine the call-site
contract gate already trusts.

### The mechanism it reuses (all verified in source)

| Piece | Where | What it gives us |
|---|---|---|
| `Statement::Guarded` fact scoping | `src/typechecker/mod.rs:3267` | pushes `callgate::facts_of(cond)` into `ctx.known_facts` for the body only, `truncate`s on exit — a real dominance analysis, already shipped |
| `prove_with_constants(&goal, ctx)` | `src/typechecker/mod.rs:2172` | the single discharge entry: folds constants, adds `known_facts` + inline call posts, calls `prove_precondition` |
| `prove_conjunct_core` first line | `src/proof_engine/callgate.rs` | `facts.iter().any(\|f\| f == goal)` — **structural equality proves**, so a guard written over the index expression discharges by identity |
| `Expr::Index` in `infer_expression` | `src/typechecker/mod.rs:1289`, fn at `1099` | the single type funnel; `obj_ty` and `idx_ty` are already inferred there — one hook site, sees every subscript that reaches type checking |
| `xs.Count#()` shape | `src/parser/expressions.rs:378` | `Expr::MethodCall(Identifier("xs"), "Count#", [], None)` |

### The rule — two halves, both required

For a subscript `coll[idx]` inside a `checkfree` defn body:

* **F1 — UPPER BOUND.** A dominating fact of the shape
  `<idx> < <count(coll)>` or `<idx> <= <count(coll)> - 1`, where
  `<count(coll)>` is `Expr::MethodCall(Identifier(coll), "Count#", [], _)`
  (plus the `Count#(coll)` call spelling) and `<idx>` matches the index
  expression **as written**, structurally.
* **F2 — LOWER BOUND.** `<idx> >= 0` / `0 <= <idx>` structurally, **or**
  `<idx>` is a value of `IndexIn(coll)` for the **same** collection — the
  §15.4a construction rule gives `0 ..= count`, which supplies the lower
  half (and only the lower half; this is exactly the correction).

Both hold ⇒ discharged. Otherwise an error naming the collection, the failing
half, and the guard to write. **Everything else is rejected**, including a
literal index into a dynamic collection: its length is not statically known,
so a small literal proves nothing. That is the difference between checkfree
and a lint.

### Consequence worth stating plainly

Because F1 matches the index expression structurally, the guard must name the
**exact expression you subscript with**:

```briev
checkfree defn total(xs: List<Int>) -> Int {
    let n: u32 in xs = xs.Count#();
    let mut acc: Int = 0;
    when 0 <= n {                       // F2
        when n < xs.Count#() {          // F1 — names `n` exactly
            acc += xs[n];
        };
    };
    term acc;
};
```

The last-element read is `when i - 1 < xs.Count#() { … xs[i - 1] … }` — the
guard names `i - 1`, which is expressible and true given `i <= count`. It is
NOT dischargeable from `when i > 0` alone, because that says nothing about the
upper bound. Interval reasoning that would make `i > 0` + `i <= count`
suffice for `xs[i-1]` is a **recorded follow-up**, not part of this landing.

### Why this is the healthy long-term choice

It adds **no new proof theory** — it consumes the existing fact engine, so
every improvement to guard facts (interval reasoning, atom definitions, new
fact shapes) upgrades checkfree for free. It keeps the eternal (a proof
obligation over a fact) and leaves the temporal (which guard spelling) to the
author. And it costs zero runtime: no AST change reaches the backend, so the
emitted IR is byte-identical with or without the annotation.

### Invariants (each pinned by a named test)

| # | Invariant | Test |
|---|---|---|
| 1 | **INV-CF-ONLY-DEFN** — `checkfree` on anything but a `defn` is an error | `checkfree_rejects_non_defn_target` |
| 2 | **INV-CF-REJECT-UNBOUNDED** — a subscript with no dominating N-bound is rejected, naming the fix | `checkfree_rejects_unbounded_subscript` |
| 3 | **INV-CF-ACCEPT-GUARDED** — `when 0 <= i` + `when i < xs.Count#()` discharges `xs[i]` | `checkfree_accepts_guarded_subscript` + `tests/tier1/checkfree_ok.bv` |
| 4 | **INV-CF-NO-WRONG-COLL** — a guard bounding `i` against `ys.Count#()` does not discharge `xs[i]` | `checkfree_rejects_mismatched_collection_guard` |
| 5 | **INV-CF-UPPER-NEEDED** — `when i > 0` alone does not discharge `xs[i - 1]`; the guard must name the index expression and the upper bound | `checkfree_rejects_lower_bound_only_guard` |
| 6 | **INV-CF-DOMINANCE** — a guard's fact does not leak past its block; a subscript after the block is rejected | `checkfree_rejects_guard_out_of_scope` |
| 7 | **INV-CF-INDEX-TYPE-F2** — an `IndexIn(coll)` index supplies F2 (lower half) but still needs F1 | `checkfree_index_type_supplies_lower_bound` |
| 8 | **INV-CF-EXHAUSTIVE** — a subscript nested in a call argument / list / tuple / match arm / cast / lambda is still seen (no `_ =>` escape) | `checkfree_sees_nested_subscripts` |
| 9 | **INV-CF-DYNAMIC-LITERAL** — a literal index into a dynamic collection is rejected | `checkfree_rejects_literal_on_dynamic_coll` |
| 10 | **INV-CF-OPT-IN** — defns without the modifier are unaffected by anything above | full `cargo test --lib` |
| 11 | **INV-CF-NO-EMISSION-DELTA** — annotating a corpus program `checkfree` produces a byte-identical `.ll` | `checkfree_does_not_change_emission` |
| 12 | **INV-CF-DOC** — the word appears in the master syntax reference (vocab gate) | `vocab::tests::master_syntax_reference_covers_every_keyword_and_intrinsic` |

### Named tests

- Typechecker unit tests (8) driven through the existing `check(&str)` helper:
  `checkfree_rejects_non_defn_target`, `checkfree_rejects_unbounded_subscript`,
  `checkfree_accepts_guarded_subscript`,
  `checkfree_rejects_mismatched_collection_guard`,
  `checkfree_rejects_lower_bound_only_guard`,
  `checkfree_rejects_guard_out_of_scope`,
  `checkfree_index_type_supplies_lower_bound`,
  `checkfree_sees_nested_subscripts`,
  `checkfree_rejects_literal_on_dynamic_coll`.
- `tests/tier1/checkfree_ok.bv` + `.golden` — a passing program on both engines.
- `checkfree_does_not_change_emission` — compile one corpus program with and
  without the annotation, assert the `.ll` is identical.
- `vocab::tests::master_syntax_reference_covers_every_keyword_and_intrinsic`.

### Stage order (GRIND)

1. **Parser** — contextual `checkfree` identifier in a modifier run (the
   12-line hook already proven to work), plus the non-defn rejection.
2. **Context + hook** — a `checkfree: bool` on `TypecheckContext`, set while
   elaborating a `checkfree` defn body and restored after; the `Expr::Index`
   arm at `mod.rs:1289` consults it.
3. **Matcher** — `subscript_proof(receiver, index, facts)` implementing F1/F2
   over `ctx.known_facts`, with an exhaustive match over the fact shapes.
4. **Tests** — the 9 unit tests above, red-first for the reject cases.
5. **Fixture** — `tests/tier1/checkfree_ok.bv` + golden.
6. **Docs** — SPEC §15.4a obligation subsection (replacing the corrected
   sentence's forward reference) + `docs/reference/MASTER-SYNTAX-REFERENCE.md`
   row.
7. **Gates** — `cargo test --lib`, daily smoke, `ll_determinism_check.sh
   --quick`, `scripts/gate.sh quick`; push to `main`; poll CI.

### Scope boundary (explicit, documented not implied)

- **Intra-procedural.** A call is not walked into; the callee answers for its
  own subscripts.
- **Body only.** Contract `[pre]`/`[post]` subscripts are governed by the
   contract prover with its own fact set (the guards of the body do not
   dominate a contract).
- **`foreach` needs no proof** — it never subscripts.
- **Interval reasoning over the index** (`i > 0` + `i <= count` ⇒ `xs[i-1]`
  safe) is a follow-up: it generalizes every guard fact, so it belongs in the
  fact engine, not in checkfree.


**Confirmed by the owner.** Completes the Goose §7.14 borrow: index types
(landed `4d0a8d7f5`) let a subscript carry its bound; `checkfree` is the
obligation that forces a function to USE that, or be rejected.

## Syntax (prefix modifier — reuses the existing mechanism)

```briev
checkfree defn total(xs: List<Int>) -> Int {
    let i: u32 in xs = xs.Count#();
    let mut acc: Int = 0;
    foreach k in 0..((xs.Count#() - 1)) {
        acc = acc + xs[k];
    };
    term acc + xs[i - 1];
}
```

`checkfree` is a **contextual identifier**, not a lexer keyword — exactly
the `unbounded` precedent (helpers.rs:221: no lexer token is added, the
word stays usable as an identifier). It opens a modifier run and lands in
`Definition.modifiers` through the existing `parse_modifier_prefixed`
Defn arm. **Zero new grammar productions.**

## Semantics

A `checkfree` defn must have **every** subscript in its body statically
in-bounds. A subscript `coll[idx]` passes only when:

- **P1 — index-typed**: `idx` is an identifier whose declared type is
  `IndexIn(coll)` and `coll` is the same collection by name (the §15.4a
  construction rule guarantees the bound); or
- **P2 — static length**: `coll` is a `Vector<_, N>` (or fixed-size array)
  and `idx` is a non-negative decimal literal `< N`.

Everything else — a plain `Int` index, a computed index, an index typed
against a DIFFERENT collection, an index-typed value into a differently
named collection — is an error naming the site, the collection, and the
fix. A literal index into a *dynamic* collection (`xs[0]` on a
`List<Int>` parameter) is **rejected**: the length is not statically
known, so a literal proves nothing. That is the whole point of checkfree —
it is a proof obligation, not a lint.

`foreach x in coll` needs no check (in-bounds by construction) and
contains no `Index` node.

## Invariants (each pinned by a named test)

| # | Invariant | Test |
|---|---|---|
| 1 | **INV-CHECKFREE-ACCEPT-IDX** — a defn whose only subscripts use a matching index type compiles | `checkfree_defn_accepts_index_typed` (typechecker unit test) + `tests/tier1/checkfree_ok.bv` |
| 2 | **INV-CHECKFREE-REJECT-PLAIN** — a plain `Int` index in a `checkfree` defn is an error naming the collection | `checkfree_defn_rejects_plain_int_index` |
| 3 | **INV-CHECKFREE-REJECT-LITERAL** — a literal index into a dynamic collection is rejected (static prove-ability, not "small number") | `checkfree_defn_rejects_literal_on_dynamic_coll` |
| 4 | **INV-CHECKFREE-REJECT-WRONG-COLL** — an index typed for `ys` used on `xs` is rejected | `checkfree_defn_rejects_mismatched_collection` |
| 5 | **INV-CHECKFREE-VECTOR-LITERAL** — a `Vector<_, N>` subscript with a literal `< N` is accepted | `checkfree_defn_accepts_vector_literal` |
| 6 | **INV-CHECKFREE-EXHAUSTIVE-WALK** — the body walk visits every expression position, so a subscript nested inside a call argument / list / tuple / match arm / cast / unary op is still checked (no `_ =>` escape) | `checkfree_defn_rejects_nested_subscript` — the index sits inside `Print#(helper(xs[bad]))` |
| 7 | **INV-CHECKFREE-ONLY-DEFN** — the modifier on `node`/`txn`/`let` is an error (like `accel` on a defn) | `checkfree_rejects_non_defn_target` |
| 8 | **INV-CHECKFREE-OPT-IN** — defns without the modifier are unaffected (the whole existing lib suite still passes) | full `cargo test --lib` |
| 9 | **INV-CHECKFREE-DOC** — the word is in the master syntax reference (the vocab gate asserts keyword coverage) | `vocab::tests::master_syntax_reference_covers_every_keyword_and_intrinsic` |

## Named tests

- `tests/tier1/checkfree_ok.bv` + `.golden` — a passing program (index-typed
  last-element read), both engines.
- Typechecker unit tests (6 of them) driven through the existing
  `check(&str)` helper in `src/typechecker/mod.rs`'s tests module.
- `vocab::tests::master_syntax_reference_covers_every_keyword_and_intrinsic`
  (existing gate, extended by the doc row).

## SPEC

New subsection under §15.4a: "`checkfree` — the proof obligation", stating
P1/P2, the rejection of literal-on-dynamic, and that it is opt-in. Plus a
row in `docs/reference/MASTER-SYNTAX-REFERENCE.md`.

## Scope boundary (explicit)

The obligation is **intra-procedural**: a call to another defn is not
walked into. That is honest and matches Goose (per-specialization,
inlining-time); calling an unproven helper from a checkfree defn is
allowed but does not discharge the helper's own subscripts. Recorded in
the SPEC text so the boundary is documented, not implied.

---

## STOPPED AT THE SOUNDNESS GATE — W6-2's index type is a BOUND, not an INDEX

Implementing the obligation exposed a **soundness bug in the landed index
type** (`4d0a8d7f5`), and shipping `checkfree` on top of it would certify
out-of-bounds reads as PROVEN. Work reverted; the finding is the
deliverable.

### Evidence

```briev
checkfree defn pick(xs: List<Int>) -> Int {
    let i: u32 in xs = xs.Count#();   // i == 3 for a 3-element list
    term xs[i];                        // subscript at the BOUND, not an index
};
```
`brievc check` → **OK** (the obligation discharged it). Running it on
`[7, 8, 9]` printed ` 00` — an out-of-bounds read, no trap. `checkfree`
certified it.

### Why

`Count#()` is the **element count** `len`, and the valid indices are
`0 .. len-1`. `Count#()` itself is out of range by exactly one. So an
index type constructed from `Count#()` is a **bound** (`0..=len`), and a
subscript through it is *not* provably in range. SPEC §15.4a currently
claims it is.

Aggravating factor (found in the same probe): arithmetic **degrades** to
the plain carrier (`i - 1` is a plain `Int`), so the canonical last-element
read `xs[i - 1]` cannot be discharged either. Both the direct form and the
neighbour form are unsound or unprovable — which means the type as landed
cannot discharge any subscript.

### The three candidate resolutions (owner decision)

1. **Split bound from index (Goose's own split).** Goose has two types:
   `u32 in a` = INDEX `0..len-1` and `u32 upto a` = BOUND `0..len`.
   Briev would need the same: `upto` stays constructible from `Count#()`
   (usable as a loop bound and in arithmetic), and `in` becomes a true
   index that needs a total construction source. On a growable collection
   there is none — an empty collection has no valid index — so `in` needs
   a fallible constructor (`Result`) or an `upto`-then-guard flow.
2. **Guard-based discharge (keep one keyword).** `checkfree` accepts a
   subscript only when a dominating `when` guard establishes
   `expr < coll.Count#()` (and `expr >= 0`). This reuses the existing
   guard-fact machinery (`ctx.known_facts` + the prover) and makes the
   obligation sound without new type spelling. Cost: a real
   guard-domination analysis, which is a session-sized piece, not a
   modifier tweak.
3. **Narrow the type to what it can prove.** Keep `u32 in xs` as a bound,
   and *remove* the SPEC §15.4a claim that a subscript through it is
   in-bounds. The type then means "a value known to be ≤ Count#()" — still
   useful (loop bounds, `when` guards) but not a subscript licence.

### Recommendation

**Option 2** for the obligation, plus **Option 3's** wording correction
immediately: the §15.4a sentence "Such a subscript is provably in-bounds"
is false today and must not stay in the spec while the mechanism is
unbuilt. Options 1 and 2 compose — a true `in` index type is the better
end state, and the guard analysis is needed for either.

### What was kept from this attempt (reverted, re-usable)

The parser hook was a 12-line, zero-new-grammar change (a contextual
identifier in `starts_modifier_run` + the prefix collection loop,
mirroring `unbounded`), and the walker correctly reused
`ast::visit::walk_stmts` (the one exhaustive visitor, no `_` arm) so a new
`Expr` variant cannot escape the obligation. The analysis collapsed to
`subscript_verdict(receiver, index, types)` with P1/P2. All of it is
sound-by-construction *except* the false premise P1 rested on.
