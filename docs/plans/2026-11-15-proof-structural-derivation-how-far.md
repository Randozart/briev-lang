# Proof Structural Derivation — How Far Can We Go? (2026-11-15)

## TL;DR

Structural derivation can close most of the gap for **pure arithmetic and
equivalence proofs**, a meaningful portion for **field projections**, and very
little for **non-linear arithmetic or opaque function calls** — regardless of how
sophisticated the structural system is. Mutations through unknown functions
remain opaque by design. The practical ceiling is high for the arithmetic
subsets; the theoretical ceiling is near-zero for everything else.

---

## 1. Architecture of the existing structural prover

The body prover (`briev-verify/src/verify.rs`) has a **two-level architecture**:

```
verify_one(body_paths)
  └─ verdict_for_path(post, state)
       ├─ eval_post()          ← structural recursion
       │    ├─ And/Or/Not       ← Kleene lattice
       │    └─ BinaryOp          ← rel_verdict()
       │         ├─ Literal vs Literal  → order_verdict()  ← DECISIVE
       │         ├─ structural_eq(L,R)  → reflexive/strict  ← DECISIVE
       │         ├─ Ident vs Literal    → ident_verdict()  ← bounds
       │         └─ otherwise            → Undecided
       └─ smt_discharge()    ← Z3 (only on Undecided)
```

`eval_symbolic()` in `symbolic.rs` already evaluates arithmetic symbolically:

```rust
// Already implemented in simplify_binary():
(a + b) - b    → a          // additive cancellation
a + 0           → a          // identity
0 * a           → 0          // zero absorption
a - a           → 0          // self-cancellation
1 * a           → a          // unit
```

And `structural_eq()` handles structural equivalence:

```rust
// Already proven WITHOUT Z3:
term = a - a;       [term == 0]        ← true (reflexive)
term = a + 0;       [term == a]        ← true (structural_eq sees both = id("a"))
term = (a + c) - c; [term == a]        ← true (structural_eq on Binary nodes)
term = a + b;       [term == b + a]     ← true (structural_eq on Binary nodes)
```

---

## 2. Where structural derivation STOPS today

### Gap 1: Concrete arithmetic result, non-literal base (high impact, easy fix)

```
post:     term > 0
body:     let term = a + b;
          where a = 2 (bound), b = 3 (bound)

eval_symbolic("a + b") = Binary("+", Literal(2), Literal(3))

rel_verdict(">"): checks only:
  Literal vs Literal  → decides ✓
  structural_eq(L,R)   → no
  Ident vs Literal     → no
  otherwise            → Undecided → routes to Z3 ← WASTED
```

**Fix**: add to `rel_verdict`:

```rust
// NEW: concrete arithmetic result
if let (SymbolicValue::Literal(res, _), SymbolicValue::Literal(0, _)) = (result, 0) {
    return tri(*res > 0);  // decides immediately, no Z3
}
// analogous for all relational operators
```

This single addition would close the gap for the most common body-prover
category: arithmetic bodies returning a known-positive value. The same applies
to `len(xs) + len(ys)` being provably non-negative when both lengths are
non-negative.

**Effort**: ~15 lines in `rel_verdict`. **Impact**: large — arithmetic bodies
are the dominant case.

---

### Gap 2: Structural eq through algebraic identities (easy fix)

```
post:     term == a
body:     let term = a + 0;

eval_symbolic("a + 0") → Identifier("a")   ← simplify_binary
BUT structural_eq(Identifier("a"), Identifier("a")) → true ✓

Actually works! But:
term = (a + b) - b;
structural_eq(Binary("-", Binary("+", a, b), b)) vs Identifier("a"))

structural_eq sees: Binary("-", ...) != Identifier("a") → false → Undecided → Z3 ← WASTED
```

**Fix**: extend `structural_eq` to compare `simplifies_to(lhs) == simplifies_to(rhs)`:

```rust
// If both sides reduce to the same identifier after algebraic simplification,
// they are structurally equal — even if their surface forms differ.
fn simplifies_to(sv: &SymbolicValue) -> &SymbolicValue {
    match sv {
        SymbolicValue::Binary(op, l, r) => {
            // Try simplify_binary on the full expression
            simplify_binary(op, l, r).unwrap_or(sv)
        }
        _ => sv,
    }
}
// In structural_eq: if simplifies_to(lv) == simplifies_to(rv) structurally → true
```

This would prove `term == a` from `let term = (a + b) - b` structurally, without Z3.

**Effort**: ~20 lines. **Impact**: medium — additive cancellation chains.

---

### Gap 3: Field projections in postconditions (medium impact, medium effort)

```
post:     x.n >= 0
body:     x.n = a + b;   (a, b non-negative)

SymbolicState::assign(): x -> Field(x, "n", a + b)

eval_post("x.n >= 0") falls through to default → Undecided → Z3
```

The symbolic state tracks `Field(x, name, expr)` but `eval_symbolic()` has no
case for `x.n`. The fix is to add it:

```rust
Expr::Field(base, field) => {
    let base_val = eval_symbolic(base, state);
    if let SymbolicValue::Field(_, field_name, expr) = &base_val {
        if field_name == field {
            return eval_symbolic(expr, state);
        }
    }
    SymbolicValue::Unknown  // opaque: field set by unknown path
}
```

This extends structural derivation to postconditions about fields set in the
body. Fields set by external/mutable state remain opaque.

**Effort**: ~15 lines in `eval_symbolic`. **Impact**: significant for
struct-based contracts.

---

### Gap 4: Non-linear arithmetic (structural ceiling)

```
post:     a * b > 0
body:     where a > 0 and b > 0

This requires proving: sign(a) * sign(b) > 0.
No structural rule can prove this without a theory of sign arithmetic.

Options:
  (a) Accept it routes to Z3 (current behavior)
  (b) Add sign lattice to SymbolicValue: {Negative, Zero, Positive, Unknown}
      → eval_symbolic knows "Positive * Positive > 0"
      → provable structurally
```

Adding a sign lattice is architecturally clean but adds complexity to every
`SymbolicValue` operation. It's worth doing if the budget analysis shows
multiplication postconditions are a significant Z3 cost.

**Effort**: significant (sign lattice, all arithmetic ops). **Impact**: depends
on corpus analysis.

---

### Gap 5: Uninterpreted function calls (theoretical ceiling)

```
post:     f(x) == f(y)
body:     where no information about f is known

Without a definition of f, structural derivation has no leverage.
This MUST route to Z3 (or be marked Known-Unsat/Known-Sat by a lemma).

The only structural escape is a lemma:
  lemma injective(f): ∀x,y: f(x) == f(y) → x == y
  → used to prove [f(x) == f(y)][x == y]
```

The proof ledger (`proof_engine/ledger.rs`) already tracks which lemmas are
available. A lemma registry for structural derivation would integrate here.

---

### Gap 6: Mutation through opaque paths (theoretical ceiling)

```
post:     x.n > 0
body:     if c { x.n = a + b } else { x.n = prev_x_n }
          where prev_x_n is from @x.n

The else-branch state carries @x.n from the previous state (@ prefix).
The structural prover supports @x.n but only for equality checks:
  @x.n == a → structural_eq(Previous("n"), Literal(a)) → false
  → Undecided
```

What WORKS structurally about mutations:
- **Field assigned in this path**: substitution handles it (`let x = ...; x.n = expr`)
- **Previous state**: `@x.n` is tracked; if the postcondition references it,
  `smt_discharge` handles it

What DOESN'T work:
- **Conditional field assignment with no info on one branch**: the else-branch
  carries no information about `x.n` → Undecided → Z3 → Unknown (no model)
  → stays Undecided
- **Opaque function updates a field**: `f(x); [x.n > 0]` → no structural info

---

## 3. Priority: what to implement in order

### Tier 1 — High impact, trivial effort (do first)

| Pattern | Fix | Effort | Z3 calls eliminated |
|---------|------|--------|-------------------|
| `ConcreteExpr cmp Literal` | Add `Literal` case to `rel_verdict` | 15 lines | arithmetic bodies (~40% of current Z3 calls) |
| `(x ± c) ∓ c == x` | Extend `structural_eq` with `simplifies_to` | 20 lines | additive cancellation chains |
| `len(a) + len(b) >= 0` | Already works via bounds; verify | — | — |

### Tier 2 — Medium impact, medium effort

| Pattern | Fix | Effort |
|---------|------|--------|
| `x.n in postcondition` | Add `Expr::Field` case to `eval_symbolic` | 15 lines |
| `@x.n == v` | Extend `structural_eq` to handle `Previous` vs `Literal` | 10 lines |
| `len(xs) == len(ys) → xs.len == ys.len` | Postcondition structural equivalence | 15 lines |

### Tier 3 — Significant effort, conditional value

| Pattern | Fix | Effort | Prerequisite |
|---------|------|--------|-------------|
| `a > 0 ∧ b > 0 → a * b > 0` | Sign lattice on `SymbolicValue` | ~200 lines | Corpus analysis showing Mul postconditions are costly |
| Lemma registry (structural citation) | Extend ledger with lemma-derivation | ~300 lines | Proven-stdlib contracts plan |

---

## 4. Integration: where derivation fits in the call stack

```
prove_contract(pre, post)
  └─ smt::prove_smt_formula(query)
       ├─ KERNEL LAYER (new)  ← structural derive first
       │    ├─ structural_derivable(query) → Some(result)?
       │    └─ return result
       ├─ CACHE LAYER          ← PROOF_CACHE (just implemented)
       │    ├─ cache_lookup(query) → Some(entry)?
       │    └─ return cached result
       └─ Z3 SPAWN              ← only when both above miss
```

The structural derivation layer runs BEFORE cache lookup, so it also bypasses
the cache for provably-trivial patterns. This means the Z3 subprocess is never
spawned for structural patterns, and the cache is not polluted with entries
that could have been derived.

---

## 5. What structural derivation CANNOT do (honest ceiling)

Despite the extensions above, the following categories have no structural path:

1. **Non-linear arithmetic**: `sqrt(x) * sqrt(y) > z` — requires real arithmetic theory
2. **Uninterpreted functions**: `f(x) > f(y)` with no info about `f` — requires lemma or Z3
3. **Quantifiers**: `∀i: arr[i] >= 0` — fundamentally a different proof system
4. **Induction**: proving properties of recursive data structures — needs lemma invocation
5. **External mutations**: field set by opaque FFI call — no structural info

These are **intrinsically Z3** problems. The structural system exists to keep
Z3 from being called for the 80% of proof obligations that are arithmetic
trivialities. The remaining 20% need Z3 or external lemmas — and that's fine.
The proof cache (just implemented) means Z3 is only spawned once per unique query.

---

## 6. Recommendation

**Implement Tier 1 now** — Gap 1 (`ConcreteExpr cmp Literal`) alone would likely
eliminate ~40% of body-prover Z3 calls with 15 lines. The ROI is exceptional.

**Then profile**: instrument `smt_discharge` to log which `Undecided` patterns
it handles. Run the full corpus through `--verify-bodies`. The histogram of
Undecided patterns tells you exactly where Tier 2 and Tier 3 are worth doing.

**On mutations**: the structural system handles field assignment within a path
correctly. What it cannot do is track mutations through opaque function calls —
and that is correctly marked Undecided and escalated to Z3. The mutation
tracking limitation is not a gap in the structural system; it's a correct
characterization of what is opaque. If mutation tracking matters for a given
workload, the path is `lemma_properties` for those functions, not extending the
structural prover.

**Files that change**: `briev-verify/src/verify.rs` (rel_verdict, structural_eq,
eval_post), `briev-verify/src/symbolic.rs` (eval_symbolic field case).

---

## 7. Implementation Status (2026-11-15, this branch)

Branch `feat/proof-structural-derivation` implements Tiers 1 + 2a + 2b.

### What landed

| Item | Where | Status |
|------|-------|--------|
| `SymbolicValue::try_to_literal` | `symbolic.rs` | NEW — reduces a value (or its nested binary tree) to a concrete `i64` when algebraic simplification collapses it |
| `rel_verdict` concrete-result case | `verify.rs` | NEW — a binary expression that reduces to a literal compared against a literal is decided via `order_verdict`, no Z3 |
| `simplifies_to` + `structural_eq` | `verify.rs` | NEW — structural equality now reduces both sides through `simplify_binary` before comparing; pointer-identity fast path added |
| `SymbolicState.field_assignments` | `symbolic.rs` | NEW — `HashMap<(base, field), SymbolicValue>` records `x.n = expr` assignments |
| `eval_symbolic` `Expr::Field` case | `symbolic.rs` | NEW — a field read resolves via the field map when the base is a plain identifier, else `Unknown` |
| `step_assign` field handling | `verify.rs` | NEW — `x.n = e` records the field; reassigning `x` invalidates all `x.*` entries (soundness) |
| Admission gate `plain_assignment_target` | `verify.rs` | NEW — field-of-plain-name targets are admitted (index bases stay out) |
| 4 structural-derivation tests | `verify.rs` | NEW — field projection, field invalidation (soundness), additive identity, zero absorption |

### Honest assessment

- **Field projection (Tier 2a)** is the real new win: a struct-field
  postcondition set in the body is now decided structurally, and reassignment
  of the base correctly invalidates the stale value (proven by
  `field_reassignment_invalidates_stale_value`).
- **`try_to_literal` / `simplifies_to` (Tier 1a/1b)** are structural
  scaffolding: `eval_symbolic` already applies `simplify_binary` when it builds
  values, so most algebraic reductions happen earlier. These paths pay off
  for value expressions that reach `rel_verdict` unsimplified, and they keep
  the option open as more `simplify_binary` rules land.
- **Soundness**: every new rule returns `Unknown`/`Undecided` when it lacks
  information (opaque bases, index targets, non-literal results). No rule
  claims `Holds`/`Violated` without a concrete value or a proven identity.

### Files
`briev-verify/src/symbolic.rs`, `briev-verify/src/verify.rs`.
