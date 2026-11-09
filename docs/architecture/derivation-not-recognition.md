# Derivation, Not Recognition — the compiler's obligation

**2026-09-30.** Status: foundational doctrine.
**2026-11-06 amendment:** idiom recognition codified (§12) — derivation
remains the obligation; recognition of intent-shaped structure with a
hardware consumer is a licensed accelerant, five gates apply, and
silicon literally built for an algorithm licenses algorithm-level
matching to THAT silicon (§12.6).
**2026-10-30 application:** destination ownership is proof-derived, not
shape-recognized. Destination/alias/lifetime facts come from semantic value
graphs and ownership analyses; backend lowering consumes
`AnalysisResults.destination_ownership` through shared helpers. It must not
recognize specific algorithms or source type names to choose a destination.
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
| hand-written `acc = acc ⊕ buf[k]` foreach (§12 W3/W3.1/P1) | exact associativity: Int wrapping add/mul and min/max (any order); **Float only via the `approx` declaration**, or `exact` to pin sequential (gate 3, 2026-11-08) | reduction synthesis (warp-slice, `SubgroupIAdd#`/`IMul#`/`SMin#`/`SMax#` int; `FAdd#`/`FMul#`/`FMin#`/`FMax#` float) — detection `detect_idiom_fold`, lane `fold_idiom_lane`; why-not: `fold_idiom_outcome` (names `approx`/`exact`) and the detection near-miss disclosure (`diagnose_idiom_fold`); retires when the general lowering reaches warp-slice parity (W3 parity gate) | `fold_idiom_tests`, `from_shape_sum_idiom_plans_cooperative_reduce`, `test_sum_idiom_lowers_subgroup_iadd`, `test_float_fold_approx_lowers_fadd_and_exact_pins`, why-not unit test, PTL gates `g0_sum`/`g0_minmax`/`g0_fsum_approx`/`g0_fsum_exact` |
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

---

## 12. Idiom recognition (2026-11-06): derivation-first, with a licensed accelerant

**2026-11-06 amendment (operator decision).** Recognition is the cardinal
sin (§2) — and the language has always drawn a line inside "recognising":
the shape recognition in §3 (reduction topology, affine index sets) and
§7's admissible matchers (the GEMM affine shape) are derivation. This
amendment names the third category and binds it. **The primacy rule is
absolute: general derivation is the obligation; idiom recognition is an
accelerant on the same facts, feeding the same machinery, retiring when
derivation catches up.**

### 12.1 The three categories

| Category | Recognises | Status | Test |
|---|---|---|---|
| **Shape recognition** | structural facts of the declared computation (reduction topology, affine index sets, disjoint counters) | doctrine-clean — this IS derivation (§2) | delete the composite; the pass still fires on structure |
| **Idiom recognition** | a language-level structural idiom whose semantics a hardware primitive CONTAINS (associative reduce → subgroup reduction; dot-accumulate → DPAS; transpose → register-shuffle path) | permitted under the five gates (§12.2); frontend plan input only | the primitive's semantics ⊇ the idiom's semantics; proof-licensed; disclosed; retired on general parity |
| **Algorithm recognition** | op names, op sequences, magic constants, identifiers | banned, unchanged (§2, Rule 23) — except §12.6 | none — no license outside the silicon carve-out |

The litmus (operator, 2026-11-06): **idiom recognition matches
intent-shaped structure that maps to hardware semantics; algorithm
recognition matches cargo-culted sequences that would live silently in
the code forever and cause heisenbugs.** The idiom inventory (reduce,
scan, transpose, map-chain, accumulate) is decades-stable language
structure; algorithm fashion rotates. An idiom-matched lowering passes
the year-two test (§6); an algorithm match cannot.

### 12.2 The five gates (all mandatory, every instance)

1. **Structure, not name.** The matcher consumes derived structural facts
   (single accumulator, associative op, affine reduction axis, op-set
   closure). Never identifiers, composite names, numeric constants, or
   op-sequence order.
2. **Hardware-consumer test.** Fire only when a target primitive's
   semantics CONTAIN the idiom's semantics. No primitive → no
   recognition; the shape alone is never a reason, the mapping is.
3. **Proof license (Rule 25).** Every semantic delta the lowering makes
   (reassociation, precision) is licensed by the program's
   contract/proof state. Unlicensed → the general lowering, which stays
   the default floor.
4. **Disclosure, both directions.** Firing is a frontend plan decision,
   disclosed (`reduce-idiom (assoc add, f32) → reduction synthesis
   (warp-slice)`). Non-firing is diagnosable with the named failed fact.
   A silent matcher is the violation — Briev is, first and foremost, an
   honest compiler.
5. **Retirement gate + ledger.** Every matcher enters the coverage
   ledger (§8) with its general-path parity check and retires when
   general derivation reaches parity (Rule 24). A matcher that beats the
   general path permanently is a failing default (Rule 2) — fix the
   default.

### 12.3 Primacy of derivation (the operator's constraint)

Idiom recognition is an **accelerant on the same facts the general pass
consumes** — it routes into the general machinery's plan, never around
it:

- the matcher's output is a plan input (the frontend decides once,
  disclosed); the backend never grows a recognition arm (Rule 6,
  frontend-driven dispatch);
- the matcher's lifetime is measured parity debt, not privilege —
  **always try to derive general shape optimization first and most**;
  every hour spent on a matcher is an hour owed to the general pass, and
  the ledger's parity check is the invoice;
- the end state is §6: the matcher fires because the structure is
  derivable; when derivation catches up, the matcher is deleted and
  nothing regresses.

### 12.4 What stays banned

Op-sequence matching ("A-then-B-then-C is attention"), magic constants,
identifier/name matching, backend recognition arms, undisclosed firing,
ceiling-by-coverage — with exactly one licensed exception, §12.6. The
GatherElements receipt (intel-backend-harvest §0.2) and the
fused-attention retirement (§11) are the standing evidence: hand-curated
tables and named kernels fail in the field; derived machinery wins.

### 12.5 Enforcement

The recognition gate (§7) extends: every idiom matcher carries a ledger
row (idiom → structural facts → hardware consumer → diagnostic → parity
test → retirement condition) and a why-not diagnostic path. A matcher
without the row is a gate failure; a §12.6 matcher **without a
registered `#` intrinsic** is a gate failure (§12.6, Model B). First
instance: the reduction idiom (plan
`docs/plans/2026-11-06-idiom-recognition-doctrine.md`, W3).

### 12.6 The silicon carve-out: hardware literally built for the algorithm

Algorithm-level matching has exactly one license: **the consumer is
silicon literally designed to execute that algorithm.** A fixed-function
attention unit, a tensor core's MMA datapath, a hardware softmax block,
an AES/CRC engine — the vendor etched the algorithm into the die; the
datasheet is public, immutable semantics. Matching the algorithm to its
designed silicon is therefore not cargo cult: there is nothing silent
and nothing rot-prone, because the hardware cannot change out from under
the matcher.

The license is narrow, and the other gates still apply:

- **Literal-build test:** the consumer's public, documented semantics
  must BE the algorithm (an attention block, not "a block that happens
  to help attention"). A programmable unit whose current driver library
  is tuned for an algorithm does NOT qualify — that is catalog
  recognition (the library ships updates; the die does not).
- **Proof license:** the numeric contract still licenses precision — a
  fixed-function fp16-internal softmax fires only when the contract
  allows.
- **Disclosure, ledger, retirement:** as §12.2(4,5). Retirement means:
  when general derivation on programmable silicon reaches the
  fixed-function unit's numbers, the matcher retires.
- **Consumer identity:** the match targets that silicon's own interface
  (its driver/ABI). The carve-out never licenses a software kernel
  emission "because the algorithm is popular."
- **Intrinsic-anchored routing (2026-11-06, operator Model B):** a §12.6
  match is legal only through a **registered `#` intrinsic** naming the
  silicon unit (`SoftmaxFixed#`-class: consumer, die semantics, and
  precision in one registry entry — Rule 3's `#` marker is the disclosure
  surface for hardware special treatment). The registry entry IS the
  license record: the literal-build test becomes a reviewable registry
  property. The compiler may auto-select the intrinsic's lowering when
  the gates pass; the author may also call the intrinsic to pin intent —
  both paths disclose identically (`attention → SoftmaxFixed# (NPU
  fixed-function, fp16)`). Rule 2 holds: the intrinsic is disclosure +
  fallback + ledger anchor, never a required keyword; on absent silicon
  the capability matrix (capabilities.rs) withholds the lane and general
  derivation runs, with why-not stating so.

Consequence for in-flight work: the PTL node's NPU (fixed-function AI
blocks, enumerated by OpenVINO alongside CPU/GPU) is a legal
algorithm-match target under this carve-out; the programmable iGPU is
not — its datapaths are the §12.1 category-2 case.

**Scope note:** the intrinsic disclosure requirement attaches to §12.6
only. §12.2 idiom recognition keeps diagnostic disclosure — its
consumers (`GroupNonUniform*`-class ops) are compiler-internal lowering
inside the general machinery, not author-reachable hardware units;
making idioms intrinsics would break the §12.3 primacy rule.
