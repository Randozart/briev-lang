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

> **Superseded — see §9.** A buffer cannot currently be a parameter of a verifier
> candidate at all, so neither C nor A is reachable as written. Kept because the
> reversal is the useful part.

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

## 9. Correction: indexed storage is never an `Int`

Tried to build the first C slice — accept a write-only store into `buf[expr]` — and
the typechecker refused the fixture outright:

```
error: type errors:
  invalid operation 'indexed assignment requires writable storage; declare op
  SetAt(index, value) on the collection or use a mutable pointer or array' on type Int
```

So in Briev a **buffer is never an `Int`**. Indexed storage is either a collection
that declares `op SetAt(index, value)`, or a mutable pointer / array. Both are
non-integer types, and the prover admits a candidate only when every parameter is
an integer type (`signature_facts` / `width_of`). A buffer therefore cannot appear
in a verifiable body at all — which is the real blocker, upstream of both C and A.

**Two consequences, both corrections to what precedes this section.**

1. **The ordering is wrong.** C cannot be exercised before a buffer *type* is
   admitted into the prover's world. That admission is A's prerequisite, not a
   later refinement of C. The first step is "let a buffer-shaped parameter into a
   candidate", not "teach the encoder `select`".
2. **The 37-loop figure is misleading**, for exactly the reason the 8/3/154 figure
   was earlier: it counted loops in `$defn` / collection bodies, which according to
   SPEC 10.7 are **not verifier candidates**. Indexing in those bodies resolves to
   `SetAt` at compile time and never reaches the prover. So the payoff of C is not
   37; it is the subset of those that would become candidates once buffers are
   admitted — unknown, and worth measuring before any code.

**What was reverted.** The write-only-store erasure (drop `buf[i] = v` when the
postcondition never mentions `buf`) was implemented, built and then removed: it is
sound, but no Briev source can reach it, so it is untestable against real code and
would be dead weight. That is the "tests or it does not exist" rule applied to
itself.

**Revised first step.** Measure how many declarations take a buffer/pointer
parameter *and* carry a contract — that is the population a memory model could
serve. If it is a handful, the honest conclusion is that memory reasoning is not
where the value is and the invariant line should be declared finished.

### Result: 104, so it is not a handful

Measured over `lib/` + `examples/` (1,462 top-level declarations):

| filter | count |
|---|---|
| top-level declarations | 1,462 |
| non-integer parameter | 699 |
| ... and a contract | 353 |
| **buffer-shaped** parameter (`Data`, `Ptr`, `&`, `*`, `List`, `Map`, `Set`, `Buffer`, `Vec`, `Array`) | 221 |
| **... and a contract — the memory population** | **104** |

The looser rows are the shape of a measurement error worth keeping in view:
`Bool`, `String` and `Expr` inflate 104 to 353. Only the last row is the
population a memory model would serve.

**104 against 68.** The invariant line already covers 68 in-subset loops; a working
memory model would roughly double the verifiable surface, and it is the only path
to the GPU-shaped code in `lib/std/numeric.bv`. That is material, not a handful, so
memory reasoning is worth building for and this gate passes.

What the number does **not** say is that an array theory is the next thing to
write. Those 104 declarations first have to be *admitted*, and the blocker is that
the prover takes integer parameters only. The next question is therefore narrower
and much cheaper than `select`/`store`:

> What is the smallest admission that lets one buffer-shaped parameter into a
> candidate, without pretending to model its contents?
