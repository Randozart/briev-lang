# Derivation, Not Recognition — the compiler's obligation

**2026-09-30.** Status: foundational doctrine.
**Authority:** fed by Golden Rules 15, 19, 23, 24; Rule 2 (maximum
efficient default); Rule 9 (tests or it doesn't exist).
**Companions:** `proof-vs-shape.md` (eternal vs temporal),
`briev-capability-frontier.md` (expressiveness closure),
`briev-vs-cuda-thesis.md` (why the whole-program contract surface beats
the kernel-as-unit model), `abv-gpu-doctrine.md` (§4 one plan, per-vendor
projections), `keyword-taxonomy.md` (strategy vs ambiguity keywords vs
intrinsics — what a keyword may ever do),
`docs/plans/2026-09-30-kernel-plan-and-per-target-lowering.md`
(the machinery this governs).

---

## 0. The one sentence

**The programmer declares what they want the outcome to be; the compiler
derives the code — from facts and proofs — and never from recognising an
algorithm.**

## 1. The division of labour

Briev is a programming language and a compiler. Its design premise is that
the programmer declares **intent** — the outcome, the contract, the shape
of the *computation* — not the precise micro-shape of the machine code.
From a rich declaration the compiler receives a large body of data
(contracts, proven invariants, iteration structure, layout intent,
algebraic laws, ordering intent) and is obliged to turn **all of it** into
the optimal program. Hand-written C, by contrast, surfaces almost none of
this to its compiler; what a C programmer cannot state, the C compiler
cannot use.

The obligation that follows is exact:

> **Every declared fact must be consumed by some pass.** A fact the
> compiler *has* but no pass *uses* is unrealised capability — the
> compiler's cleverness becomes a floor instead of a ceiling.

## 2. Recognition is the cardinal sin; derivation is the job

Both words contain "shape". They are opposites.

- **Recognition (forbidden).** The compiler branches on *what the
  algorithm is*: a `softmax` arm, an attention matcher, a Google-flavoured
  "this op sequence means GEMM" rule, a fixed emission tied to one named
  algorithm. Capability then depends on which shapes we happened to
  enumerate; the language's ceiling is the compiler author's imagination
  on a given day. This is Rule 23, and the algorithmic twin of type-name
  matching (Rule 15/19).

- **Derivation (required).** The compiler reads the declared structure and
  its proofs and **invents** a codegen shape — tile, stage depth, split
  factor, vector width, fusion, dispatch geometry, load path — from
  structure and laws. Emitting code *requires* choosing a shape; choosing
  it from *proofs of the declared structure* is the entire point. A chosen
  shape is a **conclusion**, not a **match**.

The seam is mechanical, and it is checkable:

> A pass derives iff you can delete every algorithm from the program and
> the pass still fires on the *structure* that remains. A pass recognises
> iff its firing depends on identifying which algorithm the structure
> encodes.

## 3. The declared data inventory

The programmer's declaration is the compiler's raw material. Each fact
license a transform a C compiler cannot *prove* and therefore must decline.

| Declared fact | Proof it licenses | Transform it funds |
|---|---|---|
| `[pre][post]` contracts | bounds, termination, algebraic laws | unroll, reorder, split, strength-reduce |
| disjoint work-item counter (`accel [i<N][i==N]`) | writes are disjoint | no barriers; vector lanes; auto-parallel |
| single-writer / single-reader intermediates | no aliasing | fusion; buffer reuse; store fusion |
| linearity / lifetimes (`spec`, ownership) | non-overlap, liveness | staging, scheduling, slot reuse |
| iteration structure (`foreach`, fold shapes) | the reduction's topology | reduction synthesis (tree, warp-slice, split) |
| algebraic laws of the fold (assoc/comm) | reassociation legal | split reductions, tree reductions |
| layout intent (`spec`), index affine forms | memory order | coalescing, swizzle, rasterisation |
| directives (`seq`, `vol`, `async`, `sync<g>`, `atomic`, `union`, `trap`) | real ordering/aliasing intent | correctness/embedding codegen — never speed |

Nothing in this table names an algorithm. Every row is a *property of the
declared computation*, and every read of it is a derivation.

## 4. Proof, not undefined behaviour — why this can exceed hand-C

LLVM's deepest lever is UB: poison, `dereferenceable`, `argmem` claims the
optimizer is *told* to assume. A wrong claim is a miscompile. Briev's lever
is **proof**: `[pre][post]` and the borrow/linearity discipline *establish*
what LLVM merely assumes. A proven transform may be applied where a C
compiler must conservatively refuse, and it is applied **automatically**,
per program, from the whole DAG — not scaffolded by hand and validated by
testing. That is the structural advantage of a language whose unit is the
reactive node DAG with contracts over one whose unit is the opaque kernel
(`briev-vs-cuda-thesis.md`).

## 5. The obligation, stated as law

1. **Totality over the declared surface.** For *any* accel/kernel body the
   language admits, the compiler has a path to the optimum. A naive
   fallback is a correctness **floor**, tracked as **debt with a
   retirement gate** — never a permanent ceiling, never "unsupported".

2. **No keyword to win.** The efficient path is the default the model
   picks (Rule 2). Directives exist for intent/correctness the plain
   efficient codegen cannot express — never to unlock speed.

3. **Generality over enumeration.** Every optimisation is a rule over
   *structure + proofs*. If a shape needs a different strategy, the
   **general** pass learns to detect and emit it — not a new match arm in
   the backend (Rule 24).

4. **A compiler gap must never be a capability ceiling.** Where a declared
   fact has no exploiting pass yet, the author side (composite /
   metaprogramming / `Asm#`) must already be able to reach the optimum
   (`briev-capability-frontier.md`).

## 6. The acceptance test (the ceiling bar)

> Delete every algorithm from stdlib. A researcher writes the **year-two
> algorithm** in Briev, declaring only intent, structure, and contracts. It
> reaches the hardware ceiling with **zero compiler changes**.

Pass ⇒ the compiler derived from structure and proofs; the language was
expressive enough to declare. Fail ⇒ either the language under-declares
(expressiveness gap) or the compiler under-derives (derivation gap). Both
are bugs to name, not limitations to accept.

## 7. Enforcement — the recognition gate

Doctrine a reviewer must remember is doctrine that decays. Make it
mechanical.

- **Plan construction** may branch only on *structural facts and proofs*
  (reduction topology, single-reader intermediates, divisibility, proven
  disjointness) — never on a name or an op sequence that only one algorithm
  produces.
- **Lowerings** may select instructions from the target profile and the
  plan's ops — never recognise an algorithm.
- **Review/CI invariant:** any site that (a) matches a string against a
  builtin/algorithm name, (b) enumerates a fixed op sequence as "the
  pattern for X", or (c) emits a special path keyed to a named algorithm,
  is a **gate failure**. Existing pattern matchers are admissible only when
  demonstrably *fact derivations* (e.g. the deferred-normalizer detector is
  a reduction-topology proof; the GEMM matcher is a flattened-2D affine
  shape), and each carries a **Rule 24 retirement gate**: it retires when
  the general machinery drives the declared composite to its numbers.

## 8. The coverage ledger

"Smart enough to optimise everything declared" must be auditable, not
asserted. Maintain a standing ledger keyed on **(declared construct ×
proof class) → exploiting pass → test**, e.g.:

| Construct | Proof class | Exploiting pass | Test |
|---|---|---|---|
| `foreach` fold over a range | assoc/comm fold | reduction synthesis (tree / warp-slice / split) | … |
| `accel [i<N]` map body | disjoint counter | vectorisation, barrier elision | … |
| GEMM affine 2-D flat form | no-alias SSBO, static M/N/K | tile/stage/mma synthesis + cost model | … |
| single-reader intermediate | no aliasing | chain fusion / store fusion | … |
| … | … | … | … |

A construct with no row, or a row with no exploiting pass, or a pass with
no test, is an open compiler gap. The ledger is the concrete form of the
obligation in §5.1.

## 9. The language/compiler splitThe **language** owns the temporal — algorithm shapes, canonical bodies,
composites, metaprogramming (`proof-vs-shape.md`). The **compiler** owns
the eternal — proofs, proof-licensed rewrites, general lowering, the
derivation machinery. The programmer declares through the *language*; the
compiler derives in the *machinery*. The machinery (`KernelPlan` and its
per-target lowerings) is **internal** — a derivation target, never a
user-facing catalogue and never itself a declared shape.

## 10. What this means for in-flight work

- `KernelPlan` is the compiler's record of the shape it **invented** for
  the declared intent. Every op and rewrite must be reachable from declared
  facts; none may key on an algorithm name.
- Naming honesty: `KernelShape` / `from_shape` use "shape" for *structural
  facts*; they should read `KernelFacts` / `plan_from_facts` to stop
  inviting the sin.
- Detectors now in the backend (`detect_deferred_region`, the GEMM
  structural matcher) are admissible **as fact derivations** and each must
  carry a Rule 24 retirement gate.
- The strategic prize is **fact-exploitation passes** — turning already
  proven disjointness, associativity, lifetime, and bounds into
  vectorisation, fusion, split, reordering, rasterisation. The KernelPlan
  lowerings are the substrate those passes write into.

## 11. Worked example — the fused-attention retirement (2026-10-01)

The cleanest measured instance of the doctrine: recognition lost by
10×, derivation won by 2.77×, and the retirement gate fired exactly as
designed.

- **Recognition (the loser):** the `fused_attention_*` family — four
  hand-written literal kernels (scalar / mma / staged / kv-staged
  rungs) behind a GEMM→softmax→GEMM chain matcher in `ptx/mod.rs`.
  Measured 10× SLOWER than the composition (0.752 vs 0.073 ms @512² —
  the 16-row m-tile gives zero Kt/V reuse across m-tiles); gated OFF
  since `0229d9e2`; never emitted again.
- **Derivation (the winner):** the user declares `softmax_fused!` in
  the STDLIB; the compiler derives the lowering from facts — the
  deferred-region detector, split-K (`ReduceTree::Split`), the q hoist,
  and the fused online form. Each improvement is GENERAL: the hoist
  helps any deferred softmax at any geometry; the online form computes
  the dot once for any smem-resident acc; the split machinery was
  already shape-parametric.
- **The numbers:** decode composite 200.6 µs baseline → **72.5 µs**
  (target 125 beaten; ggml ~58 now 1.25× away). Correctness at or above
  the pre-change bars (CUDA lane 0.00e+00 — the rescale algebra composes
  with the combine exactly).
- **The retirement:** Rule 24's gate fired on measurement
  (`a7871a27`, ~1300 lines deleted, the family's Praetor rows gone).
  The chain-fusion *idea* (producer epilogue feeds the middle on-chip)
  survives generically in the SPIR-V lane's `try_emit_chain_fusion` —
  if the launch boundary ever shows in a profile, the lever is
  available without resurrecting named kernels.

Moral: the recognition bet tied attention performance to which shapes
the compiler author happened to hand-write; the derivation path
improved 2.77× in one session without touching a named kernel, because
every lever acted on the FACTS (j-invariance, pass structure, slice
geometry) rather than the algorithm name.
