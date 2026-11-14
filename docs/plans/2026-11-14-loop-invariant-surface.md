# Loop invariants in Briev — optional bracketed surface

**Status:** design. Approved in principle by Randy (2026-11-14): *optional*,
*bracketed*, and applying to **foreach, nodes and txns** — because all three are
loops.

## 1. The premise, checked

Briev is rich in invariants as a *concept*, but has no loop invariant:

| Construct | Meaning | Covers loops? |
|---|---|---|
| `[pre][post]` on `defn`/`txn`/`node` | obligation about one execution | no — no notion of "each pass" |
| `hold` law on a type | holds in every state of that type | no — a state predicate, not an inductive hypothesis |
| `check` / `gate` | assertion recorded as a path fact | no — a fact, not something to prove |

`grep -c invariant src/lexer.rs` = **0**. Nothing can state *"holds at entry and
is preserved by each pass"*.

## 2. All three constructs really are loops

- **`foreach`** — iterates a list or range. Bounded by construction, which is why
  an invariant is *optional*: without one the trip count is purely dynamic.
- **`node`** — SPEC §"entry loop": *"The node itself is a loop: the body runs
  repeatedly until the postcondition (goal) is met."* Its `[post]` **is** the
  loop goal, and the compiler already requires the goal to be provably reachable.
- **`txn`** — SPEC §18.4 *"the convergent loop"*.

And they share one AST node: `parse_node` returns a **`Transaction`**
(`parser/definitions.rs:1221`), so nodes and txns need no separate handling.

## 3. Surface

Reuse the bracket convention — it already means "property about this construct"
(`[pre][post]`, bounds `let r_pu[2]`). No new keyword.

**foreach** — the invariant sits between the iteration clause and the body, where
no contract exists today, so it is unambiguous:

```briev
foreach i in 0..n [acc == i && i <= n] {
    acc = acc + 1;
};
```

**txn / node** — the invariant is a *third* contract slot:

```briev
node entry1 [beginprogram && startingnumber == 1][done][step < 100] {
    step = step + 1;
    term;
};
```

Ordering rule, stated so it cannot be read two ways: the invariant bracket is
**third**. An author who wants an invariant but no postcondition writes the post
slot explicitly (`[pre][true][I]`), because a second bracket is always the
postcondition. This is checkable in `parse_contract` and needs no new token —
a third `[` is currently a syntax error, so nothing existing changes meaning.

## 4. Semantics — the three obligations

The while-rule, already implemented and proved in Counsel
(`src/check/build.rs::loop_contracts`, and the lab's `count` / `optimal_step_cycle`
kernels):

| obligation | antecedent | goal |
|---|---|---|
| `_init` | `pre ∧ locals == initialisers ∧ i == lo` | `I` |
| `_preserve` | `pre ∧ I ∧ i ∈ [lo, hi)` | `I` after the body and `i = i + 1` |
| `_exit` | `pre ∧ I ∧ i ≥ hi` | `post` |

All three are loop-free, so today's path walk reaches them without new prover
machinery — the work is emitting them, not reasoning about them.

**Bound-safety is the author's obligation.** `Int` wraps at 64 bits, so an
invariant must bound the values it reasons about *individually* and survive the
growth step. `acc >= 0` alone is not preserved (it can wrap); `acc <= K` is not
preserved by `acc = acc + i` with unbounded `i`. `acc == i && i <= n` with `n`
bounded does both. This must be documented next to the syntax, or authors will
write invariants that are true but unprovable.

## 5. Storage — the real cost, measured

Any struct-field addition is a large mechanical diff:

| Struct | `{`-literal sites |
|---|---|
| `Contract` | **165** |
| `Transaction` (nodes + txns) | **179** |
| `Statement::Foreach` | **201** |

Options:

1. **Fields** — `Contract.invariant: Option<Expr>` and
   `Statement::Foreach.invariant`. Cleanest semantics, ~366 mechanical edits,
   compiler-guided.
2. **Existing metadata** — `Transaction.metadata` already exists
   (`HashMap<String, PropertyValue>`), so txn/node invariants cost **zero** struct
   edits; `foreach` has no metadata, so it still needs a field (201 sites).
3. **New wrapper node** — `TopLevel::LoopInvariant { property, inner }` is
   additive but forces every `TopLevel` consumer to unwrap it; the risk is silent
   mishandling rather than a compile error, which is worse than a big diff.

**Recommendation: option 1, but only after the measurement below.** A field is
the honest representation; the cost is mechanical, not conceptual. Option 2 saves
the txn/node half at the price of a channel that is invisible to the type system
and easy to drop in a pass that rebuilds a `Transaction`.

## 6. Gate before building: does it pay?

The census says `foreach` is **not** in the top eight blockers
(`Load#` 29, unsupported expression 18, `coll_min` 8, …), and 223 `foreach`
appear in `lib/` with predominantly **dynamic** bounds (`0..rank`, `0..n`,
`0..dlen`). So the surface only pays if authors can actually write invariants
that discharge.

**Measure first:** take ~20 of the 223 loops, hand-write candidate invariants in
scratch (no language change), and discharge them with the *existing* prover by
encoding the three obligations as separate defns. Count how many prove.

The cluster is already located: `lib/std/numeric.bv` alone carries **20**
dynamic-bound `foreach` loops (`collections.bv` 10, `json.bv` 3, `cast_lanes` 2,
`cast_portable` 1). The 20-loop sample is therefore one file — no selection
argument, and a result that speaks for the densest real code in the stdlib.

- If ≥ ~10 of 20 discharge → build the surface (option 1), then annotate.
- If ~2 of 20 → the loops are not yet provable artefacts; the cost is in the
  code, not the syntax, and the feature should wait.

This is the same discipline that killed integer `match` (5256 textual hits, 0
real cases).

## 7. Sequence

1. **Measurement cluster** (§6) — no language change, reuses `briev-verify`
   directly. Produces the number that decides everything below.
2. If it pays: `Contract.invariant` + `Statement::Foreach.invariant`, parser
   reads the third/second bracket respectively, prover emits the three
   obligations.
3. Tests: a provable foreach invariant, a provable node/txn invariant, a wrong
   invariant failing at the obligation it breaks (entry vs preserve vs exit), an
   invariant-free loop unchanged, and a wrap-unsafe invariant refused.
4. Docs: SPEC section beside `[pre][post]`, plus the bound-safety rule.

Nothing here is implemented yet. Everything above is measured or read from the
source, except §6, which is the point.

## 8. `:=` considered, and rejected for this

`:=` exists — `Token::ColonEq`, live in `parse_derivation_block` — but it means
something else: *"`:=` introduces compile-time derivation / synthesis examples or
a reference implementation"*, used with `$defn`/`$txn`.

That is a **compile-time** meaning: derive an implementation from a specification.
A loop invariant is a **verification-time** assumption carrying an inductive
obligation. Overloading one token with both would conflate "derive me from this"
with "assume me and prove me preserved" — and the two fail in opposite directions.
A wrong derivation is a wrong program; a wrong invariant is a failed proof.

Brackets stay. They already mean "property about this construct" (`[pre][post]`,
bounds like `let r_pu[2]`), and an invariant is exactly a property about the loop.

## 9. What this unlocks

Ordered by value, not novelty. Items 1–3 need the surface; item 4 is what makes
the surface affordable; items 5–6 are where verification starts paying for itself.

**1. Total correctness, not partial.** Everything proved today is conditional on
termination. A *variant* clause (decreasing, bounded below) reuses the same three
obligations to give termination too: `[I] variant [hi - i]`. The standard next
rung, and nearly free once the surface exists.

**2. Convergence in `node`/`txn` becomes a proof, not a shape check.** The SPEC
already demands the goal be "provably reachable: a counter comparison whose body
advances the counter toward the bound". Today that is a pattern match on source
text. With an invariant and a variant it becomes an obligation the prover
discharges — a heuristic replaced by a proof, inside the compiler.

**3. Bounds safety inside loops.** `tile * 32 + lane < n` is unprovable without a
carried fact; with an invariant it is routine. That is the PTX / SPIR-V payoff,
where the bounds are the interesting property.

**4. Invariant *inference* (Houdini).** The blocker for the whole feature is that
223 stdlib loops would each need a hand-written invariant. Generate candidate
conjuncts from the body (`acc == i`, `i <= n`, `0 <= i`, `x == k`), keep the
weakest subset that is inductive, and most loops need no annotation at all. Pure
prover work, no syntax — the highest-leverage item on this list.

**5. Verification that buys speed.** Briev already refuses to reassociate a fold
unless the compiler proves it exact, and already diagnoses a redundant `seq` when
the order is provably unobservable. An invariant showing iterations are
independent (each writes a distinct index) *is* that proof — so the compiler could
auto-parallelise a provably-independent loop instead of requiring the author to
assert `approx`. Verification stops being a tax and starts earning the strategy
keyword. This is the most Briev-shaped idea here: the SPEC's rule that a strategy
keyword "may never make code faster" and must be justified by proof has a natural
dual — proof *earns* the parallelisation.

**6. Proof-carrying library.** Annotate `lib/std` once and consumers inherit the
facts. It also closes a known prover gap: a callee whose body is *proved* correct
needs no value summary to be trusted.

Two smaller ones worth keeping in view: **counterexample → test generation** (a
failed obligation already yields a concrete model; emit it as a `#[case]`), and
**ghost variables** (an invariant often needs a counter that exists only in the
proof — `ghost processed: Int`).

## 10. Implementation notes, measured

**Storage is free.** `Annotation { name: String, value: Option<Expr> }`
(`ast/top.rs:832`) is Briev's generic annotation channel — the same
`Vec<Annotation>` carrying `seq`/`approx`/`exact` — and it holds an optional
expression. There are **zero** `Annotation::` match sites, so the invariant rides
in an existing field:

- `Statement::Foreach.modifiers` (exists);
- `Transaction.modifiers` (exists, and nodes *are* transactions).

So the "~366 mechanical edits" in §5 is obsolete: **zero struct edits**. An
invariant is exactly what that channel is for — an annotation about a construct,
carrying a condition.

**Position must move.** `foreach i in 0..n [I] { }` is ambiguous: the list is
parsed by `parse_expression` and postfix `[` is Briev's index operator, so
`0..n [I]` parses as indexing the range. The unambiguous slot is between the
binder and `in`, before any expression is parsed:

```briev
foreach i [acc == i && i <= n] in 0..n {
    acc = acc + 1;
};
```

For `txn`/`node` the third-contract-slot rule stands — after `[pre][post]` the
parser expects a terminator, so a third `[` cannot be anything else.

**The 20-loop sample was unmeasurable.** `lib/std/numeric.bv`'s 20 dynamic-bound
loops are all `$defn` metaprogramming over `Float`, buffers and `Max#`/`Exp#`
intrinsics; a body must be *inside the integer subset* before an invariant can
matter. Classifying all 221 `foreach` in `lib/` by body suitability leaves **55**
with subset-clean bodies. The invariant's payoff therefore depends on annotating
code that is already in-subset — not on the raw loop count. That reframes task 75
from "count provable loops" to "annotate the 55 and see", and it lowers the
expected payoff accordingly.

## 11. Open items

Status after `9f1df3450` — `foreach` invariants shipped, SPEC §10.4 written.
Three remain, ordered by what they unlock.

**1. `node` / `txn` invariants — DONE.** Third contract slot, entry +
preservation obligations (guard `¬post`), no progress obligation yet. See §10.5.
Original note retained below for the reasoning.
`parse_node` returns a `Transaction`, and both are loops (entry loop / convergent
loop), so one implementation serves both. Unlike `foreach` there is no index
range: the loop runs until the postcondition holds, so the obligations are
entry / preservation / **progress**, not entry / preservation / exit. That needs a
decision on what the third slot means for convergence — almost certainly `I`
plus a variant, since the SPEC already demands the goal be *"provably
reachable"*. Storage is free again: `Transaction.modifiers` (same
`Annotation` channel, zero struct edits).

**2. Invariant inference (Houdini).** The annotation cost blocks scale: 221
`foreach` in `lib/`, 55 with subset-clean bodies, each needing a human-written
invariant. Generate candidate conjuncts from the body (`acc == i`, `i <= n`,
`0 <= i`, `x == k`), keep the weakest subset that is inductive — init and
preserve both hold. Pure prover work, no syntax. This is what turns the feature
from "provable if you annotate" into "usually proved without annotation".

**3. Invariant-licensed parallelisation — BLOCKED, and not on invariants.**
The idea was that a proved independent-iteration invariant is exactly the proof
the SPEC demands before reassociating a fold, making `approx` derivable rather
than asserted.

It cannot be built yet, because the prover **cannot see memory at all**:

```
verify.rs:1434  if !matches!(lhs, Expr::Identifier(_)) {
verify.rs:1435      return Some("an assignment whose target is not a plain name")
verify.rs:1498  Expr::Index(..) => Some("indexing")
```

`out[i] = ...` — precisely the shape whose iterations are independent — is
rejected before any invariant is consulted. Independence is a statement about
*disjoint writes*, so it needs index, bounds and aliasing reasoning first: a
different and much larger piece of work than everything in this document.

So the ordering was wrong: parallelisation is downstream of memory reasoning, not
of invariants. Recorded so it is not attempted again from this end.

Two smaller ones, not scheduled: **counterexample → test generation** (a failed
obligation already yields a concrete model; emit it as a `#[case]`), and **ghost
variables** (an invariant often needs a counter that exists only in the proof —
`ghost processed: Int`).

### Known limits of what shipped

- Only `foreach`. `node`/`txn` still take no invariant.
- The bracket is required to sit between the binder and `in` (see §10); a
  post-iterable position would collide with indexing.
- There is no `variant` clause, so **termination is not proved** — the obligations
  give partial correctness only, as before.
- ~~Nothing infers invariants~~ — inference shipped (`e04fada37`), so an
  unannotated loop is proved when a candidate conjunct is inductive.

### The stdlib payoff is currently zero, and the cause is candidate coverage

Measured after shipping: running the verifier over `lib/std/numeric.bv`,
`collections.bv`, `json.bv`, `cast_lanes.bv`, `cast_portable.bv` and `hashmap.bv`
yields **no loop obligations proved and no loop skipped as a `foreach`** — the
obligation machinery is never reached.

The reason is not the inference:

| `lib/std/numeric.bv` | count |
|---|---|
| plain `defn` | **0** |
| compile-time `$defn` | **6** |

`contract_target` considers `Definition`, `Transaction`, `Export` and `SyncGroup`
— **never `CompileTimeDefn`** — and `collections.bv`'s declarations are members of
collection types. So the loops are real but not *candidates*, and a whole-unit run
still reads `proven=8 / skipped=154` with the same top reasons as before (`Load#`
29, unsupported expression 18).

**The next real blocker is verifier candidate coverage** — `$defn` bodies and
member `defn`s — a wider surface than any invariant work, and one that would
unlock the annotation payoff the invariant surface exists to collect.

### The tractable next step instead: a `variant`

`foreach` terminates by construction (a bounded range), but `node`/`txn` do not:
SPEC §10.5 states plainly that no progress obligation is proved, so their
termination is unproved and "provably reachable" remains a shape check.

A `variant` clause closes that, and needs **no** memory reasoning — it is
arithmetic over the same locals the prover already tracks:

```briev
node converge [pre][done][x <= n] variant [n - x] {
    x = x + 1;
};
```

Two further obligations, both in the existing subset:

| Obligation | Assumed | Required |
|---|---|---|
| bounded — `<fn>_variant_bound` | `pre ∧ I ∧ ¬post` | `0 <= V` |
| decreases — `<fn>_variant_step` | `pre ∧ I ∧ ¬post` | `V` after one pass is strictly less than `V` before |

Storage is again free (`Annotation { name: "variant", value }`), and it would
replace the shape check on `beginprogram` reachability with an actual proof — the
same "heuristic became a proof" payoff that motivated the whole loop-invariant
line. It is a **new surface**, so it needs Randy's approval before it is built.
