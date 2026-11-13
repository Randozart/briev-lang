# Memory reasoning: index, bounds and aliasing in the prover

**Status:** plan. Nothing implemented. Scoped because it is much larger than the
invariant line that preceded it, and because two other features are blocked on it.

## 1. Why this is the next real project

Three things are blocked on the same gap:

| Blocked | Blocked by |
|---|---|
| Parallelisation (§10.8, task 78) | proving iterations write **disjoint** locations |
| Buffer loops (the GPU payoff) | reading and writing `buf[i]`, with `i` proven in range |
| Most of `lib/std`'s numeric code | bodies are buffer arithmetic (`matmul`, `softmax`) |

The invariant work gives the *right shape* of proof for all three — an invariant
that bounds the index — but the prover cannot see memory at all, so there is
nothing to bound.

## 2. What exists, measured

- **The SMT encoder has no array theory.** `briev-verify/src/smt.rs` contains no
  `Array`, `select` or `store`; every value is a 64-bit bitvector.
- **Index is refused**, not approximated: `Expr::Index(..) => "indexing"`
  (`verify.rs:1498`), and an assignment target must be a plain name
  (`verify.rs:1434`), so `buf[i] = v` is rejected too.
- **Length is not static.** It is a *reflection* access — `a.^Length` at runtime,
  `Expr::Reflect` — which is likewise unsupported. So there is no static `len` to
  compare an index against, even in principle.

That combination is why the feature is not a small arm in `expr_reason`: it needs a
memory *model*, and the model has to answer "what is the length of this buffer?"
before it can answer "is this index safe?".

## 3. Options

**A. SMT array theory.** Model each buffer as an SMT `Array` from BV64 index to
BV64 value, with a `len` parameter carried as a symbolic value and a fact
`0 <= len`. `buf[i]` becomes `(select a i)`; `buf[i] = v` becomes `(store a i v)`.
The encoder is ours, and SMT-LIB arrays already define read-over-write, so the
semantics come for free and are standard.
*Cost:* the symbolic state must carry array-valued bindings, the encoder needs a
second sort, and every existing arm assumes one sort. This is the honest option.

**B. Bounded symbolic expansion.** For a buffer whose maximum length is known,
carry N symbolic scalars. Cheap for N small (a handful), useless for real buffers,
and it changes the code's meaning if N is wrong — a wrong translation is a wrong
proof. Only acceptable where a literal bound is written and enforced.

**C. Index-agnostic reasoning.** Prove properties that do not depend on element
*values*: slice disjointness (`accbase + d` never collides with `outbase + d`),
monotonicity of indices, bounds of computed offsets. No array theory needed —
just arithmetic over the index expressions that already exist.
*This is the cheapest useful step*, and it is exactly what parallelisation needs:
disjointness is a statement about *addresses*, not contents.

## 4. Recommendation

**C first, then A.** They answer different questions and C is small:

1. **C — address arithmetic.** Let the prover reason about index expressions as
   ordinary integers. `buf[i]` still cannot be *read*, but `accbase + d` can be
   bounded, compared and proven distinct from `outbase + d`. That unlocks:
   - disjointness for parallelisation (§10.8's most Briev-shaped payoff);
   - the "in bounds" half of buffer safety, once a length exists.
2. **A — array theory**, for element-contents reasoning over `buf[i]`.

B is rejected except where a literal bound is written and checked.

## 5. The soundness boundary, stated up front

These must stay **refused**, never approximated:

- **Aliasing through `Ptr` / `^Ptr`.** Two names may denote the same storage; SMT
  arrays would treat them as distinct and prove something false. Refuse any
  pointer-derived access.
- **Reflection beyond `^Length`.** `^Ptr`, `^^Size` and friends are runtime
  introspection; they carry no static meaning.
- **Out-of-bounds reads that the program relies on.** If the index cannot be shown
  in range, the access is *refused*, not assumed safe — the same rule as
  everything else in this prover.

And the wraparound rule still applies: `Int` is a wrapping 64-bit bitvector, so an
index bound must be established on the index itself, not on a sum.

## 6. Synergy with the invariant work (the reason this is now feasible)

The in-bounds obligation is exactly the shape of thing an invariant already
provides:

```briev
foreach d [0 <= d && d < dlen] in 0..dlen {
    accbuf[accbase + d] = accbuf[accbase + d] * r_ + p_ * value;
};
```

The `[I]` bracket landed in §10.4, its inference in §10.4, the obligations in the
prover — so the machinery to *carry* `0 <= d < dlen` across iterations already
exists. What is missing is only the memory model that consumes it.

## 7. Measured: C carries a 37-loop population

Scanned all **185** `foreach` bodies in `lib/` and classified them by what they
need beyond integer arithmetic:

| class | loops | unlocked by |
|---|---|---|
| index only | **37** | **C** — address arithmetic |
| index + something else (`Float`, `Data`, `String`, `Load#`, `Store#`, `.^`) | 49 | A, and more |
| something else, no indexing | 31 | — |
| neither | 68 | already in the subset |

So **37 loops are unlocked by C alone**: the largest single group after the 68
already supported, and roughly three times the population that guarded-callee
summaries would have reached (about 14). That settles the ordering — build C first.

The 49 index-plus-other loops stay out of reach until the prover also models
buffers or intrinsics, so A is the second step, not the first. The 68 `neither`
loops are the ones the invariant and inference work already covers.

## 8. Sequence

1. Measure (§7) — pick C or A on evidence, not on this note's guess.
2. C: index expressions as integers; disjointness obligation; refusals in §5.
3. A: `Array` sort in the encoder, `select`/`store`, length as a symbolic value.
4. Tests from real kernels: `matmul`'s inner fold and `softmax`'s buffer loop are
   already in `lib/std/numeric.bv`, both currently unprovable.
