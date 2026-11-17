# Generics / variadic verification — investigation findings

**Status:** PLAN (investigation done; no code changed).
**Question:** what is behind the ~2007 skip lines carrying
*"generic or variadic declaration has no scalar arithmetic model"*, and is any of
it cheaply closable?

## 1. Where the gate is and what it does

`briev-verify/src/domain.rs:109` — `unsupported_declaration` rejects any
`Definition`/`Transaction` whose `type_params` are non-empty or whose
`variadic_param` is set:

```rust
let generic = !d.type_params.is_empty() || d.variadic_param.is_some();
if generic {
    return Some("generic or variadic declaration has no scalar arithmetic model".into());
}
```

This is an **early exit**: it fires before parameter/output type checks, before
body admission, before any call-gate consideration. So a generic declaration is
removed from the candidate set wholesale — its body is never examined, and any
caller that refers to it via a value summary is likewise skipped.

## 2. What is actually behind the count (measured, not assumed)

### 2a. The declaration inventory

The 2007 lines are 46 generic defns + 2 variadic decls, but the per-line weight
is disproportionate because the bucket is *re-provenant*: each generic's callers
also skip. Distinct declarations by file:

| file | generic | variadic |
|---|---|---|
| lib/std/iterator.bv | 25 (t/d) | 0 |
| lib/std/hashmap.bv | 8 | 0 |
| lib/std/array.bv | 5 | 0 |
| lib/std/option.bv | 4 | 0 |
| lib/std/result.bv | 4 | 0 |
| lib/std/meta.bv | 0 | 1 |
| lib/std/posix/syscall.bv | 0 | 1 |

So 46 of 48 are type-parameterised stdlib defns; the 2 variadic are different
animals. A permissive re-scan of all of `lib/` adds 6 more in
`lib/ffi/mappers/template.bv` (a template, not a compilation unit) — **the real
inventory is 52 declarations, not ~2000.**

### 2b. Why the count says 2007: the prelude re-counts it ~80 times

The report emits **one skip line per declaration** (`report.skipped.push(...)` in
`briev-verify/src/verify.rs`), and the corpus runner counts one skip per line
(`scripts/verify_bodies_corpus.sh:142`). So 2007 skip lines cannot come from 52
declarations unless the same declaration is examined once per file.

It is. `plugins/parsed/prelude-native.bv` injects imports into **every** native
compilation unit, at `$(Parsed @ highest)`, and `std/iterator.bv` is one of them:

```briev
// 2026-10-06 (BUGS.md "List<T> + List<T> silently miscompiles"):
// `List + List` elaborates to a call to the stdlib `iter_chain` …
Import$("std/iterator.bv"),
```

`lib/std/iterator.bv` holds **25** generic declarations. The corpus is 92 files;
2007 / 25 = **80.3** files per declaration — i.e. the ~80 native units in the
corpus, which is exactly the shape. (`prelude-web.bv` does the same, which is why
the number is not exactly 25 × 92.)

The other 27 declarations corroborate the reconstruction arithmetically: array 5 +
hashmap 8 + option 4 are counted once (their own file only — no importer), and
result's 4 are counted ~12 times (12 importers), giving ≈ 2000 + 21 + 48 ≈ 2069
against the reported 2007 — a gap explainable by the handful of corpus files that
are not native units. `pf_exp ×~90` is the same phenomenon, already documented.

**Measured confirmation (0.3.0 census, 2026-11-17).** With per-entry attribution the
full `lib/std` corpus reports the same raw totals as 0.2.0 (2 216 proven / 101
unproven / 13 308 skipped) and an own-surface split of **40 proven / 6 unproven /
404 skipped**. 97% of the skip surface is imported code counted per unit, exactly
as the arithmetic predicted.

**So the "2007-line generics bucket" is 25 declarations re-examined ~80 times.**
The effort estimate implied by the number was wrong by nearly two orders of
magnitude — in the direction of *much smaller*.

This is also a measurement-integrity issue *for prelude-sourced buckets*. It is
verified for generics and it does **not** explain every bucket: the Float bucket
is 638 (value-summaries table) / 822 (later count) lines, and `float_fmt.bv` — the
Float prelude file — declares only 2 Float-typed functions, so Float skips cannot
be duplication; they come from many files' own declarations. The safe statement is
the narrow one: **a skip line means (declaration × compilation unit), so any
bucket whose declarations are prelude-injected must be divided by the corpus size
before it is read as program surface.** Buckets whose declarations are file-local
(Float, tuple results, per-file intrinsics) are unaffected.

**Process trap:** because `iterator.bv` is prelude-injected, editing it is inert
until `brievc` is rebuilt — the same trap already recorded for `cast_lanes.bv`.

## 3. Three populations, three different costs

### 3a. Iterator family (25) — the real mass

`lib/std/iterator.bv` is a pattern: a generic `defn iter_*(…)` whose body is
exactly `term iter_*_loop(…)`, plus a generic `txn iter_*_loop` that walks a
`List<T>`. Their posts are all about `.Count#()`:

```briev
defn iter_map<T, U>(list: List<T>, f: T -> U) -> List<U> [true][term.Count#() == list.Count#()] {
    term iter_map_loop(list, f, [], 0);
};
txn iter_map_loop<T, U>(list: List<T>, f: T -> U, result: List<U>, i: Int)
    [i < list.Count#()][i == list.Count#()] -> List<U>
{
    result <- f(list[i]);
    i = i + 1;
    term result;
};
```

**Even if the generic gate opened, these would still be skipped** — by three
further, independent gates, in order:

1. **`List<T>` type** — the resolver requires `Type::Custom`, so an
   `Applied("List", [...])` parameter is `Unsupported` before the generic gate
   even if the generic gate were lifted.
2. **`f` — a function-typed parameter** — `T -> U` is not a scalar model.
3. **Indexing / `<-` mutation / `f(...)` call** — the body's statements are outside
   the walk.

So the iterator family is **a theory layer, not a gate flip**. Closing it means a
collection model (`List` length + element semantics), a function-value model, or
both.

### 3b. Option / Result (8) — the Bool-accessor split

`is_some<T>`, `is_none<T>`, `is_ok<T,E>`, `is_err<T,E>` are **Bool-returning**
guards whose bodies are typically `match opt { … }` — the typed-union/discriminant
model the prover does not have. These are *not* scalar-arithmetic; their contracts
(`[term == true || term == false]`) are already Bool. They are a gate fix *only*
if `match` on an `Option<T>`/`Result<T,E>` is admitted, which in turn needs a
handled-union model.

`unwrap<T>`, `unwrap_or<T>`, `unwrap<T,E>`, `unwrap_err<T,E>` return `T`/`E` and
trap on failure — they are recovery control flow, and their bodies could only
prove with an exception/`Result` semantics.

### 3c. HashMap (8) — mostly opaque methods, two `Count#()`-shaped accessors

`new_map<K,V>`, `with_capacity<K,V>`, `len<K,V>`, `is_empty<K,V>`,
`contains_key<K,V>`, `get<K,V>`, `insert<K,V>`, `remove<K,V>`.

Two of them have bodies that are *already* length-only — the same shape phase 2b
modelled for `String`:

```briev
defn len<K,V>(map: HashMap<K,V>) -> Int    { term map.Count#(); };
defn is_empty<K,V>(map: HashMap<K,V>) -> Bool { term map.Count#() == 0; };
```

So a `HashMap` opaque handle + non-negative `Count#()` atom (exactly the phase-2b
machinery, keyed on a symbolic base) would admit these two bodies with **no new
theory**. The rest are unmodelled method calls (`map.contains(key)`, `map.get(key)`,
`map.remove(key)`) or a boxed mutation (`map.insert((k,v))`), which need real value
semantics.

**But see §3e: none of the 8 declares a postcondition**, so admitting any of them
proves nothing today.

### 3e. The decisive measurement: 16 of 46 have no postcondition at all

A `proven` obligation is a *contract clause* discharged. A declaration with no
postcontract contributes **zero** proven obligations no matter how good the model
is — and because interprocedural facts come only from *declared* contracts
(`post_facts_for_term`), its callers gain nothing either.

Measured across the five files:

| | count | which |
|---|---|---|
| generic decls **with** a postcontract | 30 | all of `iterator.bv` + `array.bv` |
| generic decls **with no contract** | 16 | all of `option.bv` (4), `result.bv` (4), `hashmap.bv` (8) |

Every one of the 30 with a contract is a `List<T>`/`Vector<T,N>` loop body
(`result[j] = arr[i];`, `f(arr[i])`, `acc = f(acc, arr[i])`) — indexing, element
mutation, and a function-*value* parameter. `array_*` additionally depend on a
const generic `N`, so even `Vector`'s length is a type-level parameter.

That inverts the usual way to read this bucket: the 16 cheap-looking declarations
are worth nothing, and the 30 that carry contracts are the expensive ones.

### 3d. Variadic (2) — mostly not prover-relevant

- `lib/std/meta.bv` `$defn execute_many(...calls: Expr)` — a **compile-time**
  metaprogram, not a candidate.
- `lib/std/posix/syscall.bv` `export defn syscall(nr: Int, ...args: Int[<=6]) -> Int`
  — the variadic here is a *bounded* list of `Int` arguments; its body calls
  through `SysCall#`, which is opaque anyway.

Neither is a prover target.

## 4. Conclusion

The "generational" hypothesis — that some large fraction is a single cheap gate —
is **false**. Splitting the 2007 lines:

| population | count (decls) | what it actually needs |
|---|---|---|
| iterator family | 25 | `List` collection model + function values + indexing |
| Option/Result | 8 | `match`/handled-union model |
| HashMap | 8 | opaque `Map` category model |
| variadic | 2 | no prover value |

Every one of the three code families needs a **type-category model** — not an
admission tweak. The pipeline admits a scalar or an opaque handle; none of these
is admitted, and none of the bodies is scalar-shaped even if it were.

And the ceiling is low regardless: **16 of the 46 have no postcondition**, so a
perfect model for them still yields 0 proven. The 30 that do carry contracts are
precisely the loop bodies that need the most new theory. Phase D's realistic
yield today is **zero proven obligations**, whatever the gate does.

## 5. Recommendation

**Defer code entirely; keep it as a design reference.** The note already exists in
the roadmap's "known gaps" line. What this investigation adds:

1. **Restate the number.** Phase D is *25 prelude declarations*, not 2007
   obligations — see §2b. Any future estimate must not be sized off the skip
   count.
2. **Fix the census presentation, not the prover.** Every bucket in the skip table
   is inflated by prelude re-verification. A report-side change — label each skip
   with its origin (`own` / `imported` / `prelude`) and let the summary aggregate
   distinct declarations — would make the census mean what the roadmap reads it as,
   without touching a single verdict. This is the highest-value item the
   investigation produced, and it is a *reporting* change only.
3. The cheapest honest verification *capability* is unrelated: a `List<Int>`
   **length model** (parallel to the `String` length atom in phase 2b) would admit
   `List<Int>` parameters/returns and their `.Count#()` posts — that covers the
   `coll_*` helpers and the counting accessors, not the iterator bodies. It is
   still a model addition, not a gate flip.
3. If the goal is only "shrink the skip count," the honest path is a **skip-reason
   split**: change the diagnostic so generic-variadic is its own bucket (it already
   is), and let the model doc state "generic iteration and handled-union bodies are
   unmodelled categories," so the number is understood rather than attacked.

## 6. What would change the recommendation

- If a `List<Int>` length model lands (phase 2b-style), the iterator `defn`s'
  **contracts** (`term.Count#() == list.Count#()`) become expressible — but their
  **bodies** still index and mutate, so the defns remain skipped. The win is in
  *callers*, not the family itself.
- If `match` on an opaque handled union were admitted, Option/Result guards become
  gate-fixable. No union model exists in the prover today.

Neither is a small change. Both are worth their own design notes when the time
comes.

## 7. Suggested follow-up (no prover code)

1. Update `docs/plans/2026-11-16-verification-roadmap.md` §6 with the measured
   split and the prelude re-count (§2b) — *done, 2026-11-17*.
2. Propose, separately, a **skip-origin label** in the report so the census counts
   distinct declarations per origin. That is a small reporting change with a large
   effect on how every number in the roadmap is read. Requires BUILD mode and its
   own PR; it must not change a verdict.
3. Leave Phase D deferred. When it is revisited, the unit of work is 25
   declarations, and only 30 of 52 carry contracts at all.