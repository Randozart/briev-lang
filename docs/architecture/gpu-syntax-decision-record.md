# GPU Syntax & Hardware-Manipulation — Architecture & Decision Record

**2026-09-30.** Status: **authoritative design record.** Long-form. This
document preserves the entire design thread — the philosophy, the problem
analysis, the architecture, every decision with its alternatives and
rationale, worked examples, invariants, roadmap, and open items. It
supersedes the provisional/open items in `keyword-taxonomy.md` and
`2026-09-30-hardware-manipulation-expressiveness.md` where they conflict.

**Companions:** `derivation-not-recognition.md` (the compiler's
obligation), `keyword-taxonomy.md` (the three-category reasoning),
`2026-09-30-hardware-manipulation-expressiveness.md` (the five-layer plan),
`2026-09-30-kernel-plan-and-per-target-lowering.md` (the machinery the
syntax lowers through), `proof-vs-shape.md`,
`briev-capability-frontier.md`, `briev-vs-cuda-thesis.md`,
`abv-gpu-doctrine.md`, `gpu-backend-strategy.md`,
`2026-09-30-general-reduction-split.md`. **Feeds** Golden Rules 2, 3, 22,
23, 24.

---

## 0. How to read this document

- **Part I (§1–§3)** is *why*: the philosophy and the starting point.
- **Part II (§4–§5)** is *what*: the problem (what GPU code needs that
  Briev cannot say) and the architecture that answers it.
- **Part III (§6–§8)** is *how*: every syntax decision (D1–D27) and
  architectural decision (A1–A10) in full, plus worked examples.
- **Part IV (§9–§13)** is the *contract*: invariants, roadmap, open
  items, glossary, references.

Status tags used: **DECIDED** (settled here), **OPEN** (recorded, not
yet decided), **DEBT** (agreed direction, tracked with a retirement gate).

---

# Part I — Foundations

## 1. Purpose and scope

Briev is a programming language and a compiler. The design premise that
governs everything in this document:

> **The programmer declares intent; the compiler derives the code.** The
> programmer declares *what they want the outcome to be* — not the precise
> machine shape. From a rich declaration the compiler receives a large body
> of data (contracts, proven invariants, iteration structure, layout
> intent, algebraic laws, ordering intent) and is obliged to turn **all of
> it** into the optimal program. Hand-written C surfaces almost none of
> this to its compiler; what a C programmer cannot state, the C compiler
> cannot use.

This document answers two questions that follow:

1. **What syntax**, if any, must Briev add so a programmer can *declare*
   the hardware realities it currently cannot? (Part III.)
2. **What must the compiler derive on its own**, and what is *legitimately*
   a keyword? (Part I, §2.)

It is scoped to the **GPU/hardware surface** (`.abv`, and by extension
`.ebv`/`.sbv`), with NVIDIA PTX and SPIR-V/Vulkan as the first targets
(§6 D27). CPU/LLVM lowerings are out of scope but the layering (§5)
generalizes to them.

The audience is compiler implementers and future contributors who must
know *why* each syntactic form exists, so they neither re-litigate it nor
add a keyword that violates the doctrine.

## 2. The philosophical foundations

This section is the reasoning record. Every later decision is a
consequence of these principles, and each is stated with the reasoning
that produced it (including the corrections made along the way).

### 2.1 Derivation, not recognition (the cardinal sin)

**Principle.** The compiler derives code from **facts and proofs**. It
must never branch on *which algorithm* the structure encodes. Matching a
named algorithm — a `softmax` arm, an attention matcher, a hardcoded op
sequence only one algorithm produces — is the **cardinal sin** (Golden
Rule 23, elaborated in `derivation-not-recognition.md`).

**The mechanical seam.** A pass *derives* iff you can delete every
algorithm from the program and the pass still fires on the *structure*
that remains. A pass *recognizes* iff its firing depends on identifying
which algorithm the structure encodes. Concretely: a matcher that fires on
a reduction-topology pattern is deriving; a matcher that fires because
"this is what a softmax looks like" is recognizing — and a structure
shared by many algorithms (matmul is also conv, stencil, attention) is a
*structural class*, not an algorithm.

**Why it matters here.** GPU hardware is increasingly specialized around
*computations* (tensor cores for matrix multiply, systolic arrays,
matrix extensions). The naive reading is "the compiler must recognize GEMM
to use the tensor core." That reading is the trap: it makes capability
depend on which shapes the compiler author enumerated. The doctrine's
answer (§2.8, §5.4) is that such hardware capabilities become
**fundamentals** the compiler binds *by structural requirement*, and many
algorithms bind to the same fundamental. Recognition stays at the
*algorithm* level and stays forbidden; binding to a hardware fundamental
is derivation.

**The escape that keeps it honest.** Anything the compiler cannot derive
must be expressible by the author (`briev-capability-frontier.md`,
expressiveness closure). Otherwise the compiler's cleverness is a ceiling,
not a floor.

### 2.2 Three keyword categories (kept strictly apart)

A recurring confusion — corrected in this discussion — is folding distinct
things under "strategy keyword." There are **three categories**, and any
new syntax must declare which it is.

**2.2.1 Strategy keywords.**
`seq`, `vol`, `pack`, `async`, `sync<group>`, `atomic`, `union`, `trap`
(Rule 2's set). They express **intended behaviour** the plain efficient
codegen would otherwise choose differently. **They may never lead to
faster code**: the fastest code for the *same semantics* must already be
the default; a strategy keyword only **constrains**. "Requiring a keyword
to win is a failing default" (Rule 2). They say: *"do it this way (even if
slower) because the default would be semantically wrong."*

**2.2.2 Ambiguity keywords.**
Canonical example: memory **`store`/`free`**. They are **declared at the
site of confusion** and resolve a **strategy choice whose effectiveness is
uncertain and up to intent**. Because the compiler cannot rank the
strategies, there is no "fastest default" to leak; any speed difference is
a consequence of the author's declared intent, not a default in disguise.

**2.2.3 Intrinsics / fundamentals.**
Hardware primitives the compiler knows (`Sqrt#`, `Mmap#`, `SubgroupFAdd#`,
`Barrier#`, …; the target's instruction set). A **separate** category: they
name machinery, not behaviour or ambiguity. Author-facing access is
legitimate for a *required hardware semantics* or as a *retirement-gated
derivation gap* — never as a speed lever.

**The discriminator.** Strategy = a *constraint* where the default would
violate intended behaviour (never faster). Ambiguity = a *selection* the
compiler cannot rank, declared at the site, up to intent (may differ in
speed, but there is no determinate default). Intrinsic = a primitive.

### 2.3 The observability razor

> A keyword is legitimate **iff removing it changes observable
> behaviour**. Speed is never the argument — at most a consequence.

Corollaries:

- A keyword whose removal preserves all observable behaviour is a
  **performance hint in disguise** → the default must be fixed, never
  keyworded.
- A keyword for a choice the contracts already determine is **redundant**
  (the compiler derives it) → bug.

The razor is phrased in *observable behaviour*, not performance, so it
survives the tricky case below.

### 2.4 "What if the semantic difference is faster?" — resolved

The dangerous case: an ambiguity/strategy keyword selects an observably
distinct behaviour that happens to be faster. Is that "the default leaked
as a keyword"?

**Resolution.** If the behaviour is **observably distinct**, the compiler
*could not* have chosen it (choosing it would have changed what the
program does). The author's declaration supplies a permission or choice
the compiler must not assume; the speed is incidental to a genuinely
different program. So it is **legitimate**. The only failure mode is a
keyword that restates behaviour the compiler *could and should* have
chosen on its own — removal changes nothing observable, only speed.

Two sub-cases the razor's single test subsumes:

- **Observable and *determined* by contracts**: the compiler derives it, so
  removing the keyword changes nothing → by the test, a bug.
- **Observable and *undetermined***: the compiler cannot assume it → the
  keyword is legitimate.

### 2.5 Preconditions are the scheduling mechanism

The decisive realization of the discussion:

> A node's `[pre]` decides **when it fires**. The reactor derives order,
> parallelism, and synchronization from the guards plus the
> independence/ordering proofs; Rule 22 forces the concurrency
> classification (`async`/`sync<group>`) exactly where simultaneity is
> ambiguous. **The temporal schedule is therefore already expressed** — by
> guards and classification.

Consequences:

- There is **no need for a "schedule" surface**. The reactor *is* the
  scheduler (see `briev-execution-model.md`).
- The `chain`/`into` sugar (core, `2026-09-22-core-chain-into.md`) is
  **guard-erogonomics**, not a scheduler — it desugars to nodes whose
  preconditions are ANDed from `into` conditions. It adds no scheduling
  capability.
- What remained under the old label "schedule/shape" is actually three
  different things: **scheduling** (preconditions — exists),
  **shape** (intra-node codegen strategy — derived; ambiguity-declared),
  and **execution model** (structural contracts — primitives/declarations).

This reframe resolved the earlier "where does the schedule surface live?"
fork by dissolving it.

### 2.6 The DAG-derivation principle

> Given the whole-program DAG plus the proofs already available
> (disjointness, single-writer, lifetimes, associativity) **and**
> `atomic`/`sync`, the compiler derives: parallelism, tiling, staging,
> fusion, reduction trees, barrier placement, memory placement,
> instruction/fundamental binding, and scheduling.
>
> **Everything determinate is derivable. Only ambiguity resolution is
> not.**

This is the answer to "draw the DAG — what can we not derive?": we cannot
derive the *resolution of an ambiguity* (a choice among observably
distinct behaviours the program does not determine). Everything else is a
**derivation backlog** (expressible, not yet optimized), not an
expressiveness gap.

### 2.7 The ambiguity resolution rule (soundness criterion)

> The compiler **never resolves an ambiguity silently.**
> - **Benign** (a defensible default exists): it **picks the default and
>   warns**, disclosing the pick and the keyword that would override it.
> - **Material** (no defensible default / the wrong pick is unsound): it
>   **errors**, demanding the decision.

Severity is by **soundness**, not taste:

- **Error** iff the compiler cannot pick a behaviour consistent with the
  declared contracts (any choice is arbitrary, or the wrong choice is
  unsound: a race, a use-after-free, an observing reorder). The program is
  under-specified.
- **Warn** iff a defensible default exists but intent may differ.

This merges three rules without contradiction: Rule 2 (the default), Rule
3 (the warning discloses it), Rule 22 (the error arm — unclassified
eligible pairs are a hard error). Errors are never silenceable; warnings
are policy-controlled (OPEN, §11).

### 2.8 Determinable vs undeterminable

The dividing line for "may a keyword exist":

- **Determinable** (the compiler can decide from the DAG + proofs): the
  default must be the fastest; a keyword may only *constrain behaviour*
  (strategy) — never improve speed.
- **Undeterminable** (the compiler cannot rank the choices): an
  **ambiguity** the author resolves at the site by intent.

### 2.9 Closure (expressiveness)

> *The compiler's chosen optimum must never be more powerful than the
> language. If the compiler can do X, a user must be able to build X in
> Briev.* (`briev-capability-frontier.md`.)

The acceptance test: **delete every algorithm from stdlib; a researcher
writes the year-two algorithm in Briev, declaring intent + contracts +
(for unrecognized shapes) structure; it reaches the hardware ceiling with
zero compiler changes.** Pass ⇒ the compiler derived from structure and
proofs and the language was expressive enough. Fail ⇒ either the language
under-declares (expressiveness gap) or the compiler under-derives
(derivation gap). Both are bugs to name, not limitations to accept.

## 3. What Briev already has (the starting point)

### 3.1 The dialects

`.bv` is the main language (LLVM/CPU reference). `.abv` is **pure GPU**
(SPIR-V/PTX kernels). `.ebv` is Electronics Briev (LLVM/CIRCT). `.sbv` is
Silicon (CIRCT/MLIR). `.bad` is a separate assembly dialect that never
enters the `.bv` pipeline. `accel` (the offload keyword) is **`.bv`-specific**
— this was a correction: the GPU surface is `.abv`, not `accel`.

### 3.2 The `.abv` algorithm-only surface

`.abv` source is deliberately GPU-syntax-free. A GEMM *is* a naive
matmul; a flash-decode attention *is* two nested `foreach` loops with
`Max#`/`Exp#`. The author writes the algorithm; the compiler derives the
kernel. This is the intended beauty and a property to preserve.

- **Work-item model**: a counter (`let i: Int = 0;`) plus a counted loop
  contract `[i < N][i == N]`; `accel` marks it a parallel map. Disjointness
  of writes is proven when the write is **affine in the counter**
  (`a[i]`, `a[i*8]`): the affine proof gives slot-disjointness across
  work-items. Cross-work-item *writes* are rejected as "not affine in i".
  *Shared reads are permitted.*
- **Reactor**: nodes/txns fire by preconditions; the DAG is derived.

### 3.3 The intrinsic surface (inventory)

Already exposed (partial list, 173 intrinsics total):

- **Work ids / hierarchy**: `GetGlobalId#`, `GetLocalId#`, `GetGroupId#`,
  `GetNumGroups#`, `GetGlobalSize#`, `WorkgroupSize#`, `Dims#`.
- **Synchronization**: `Barrier#`, `Fence#`.
- **Subgroup / warp**: `SubgroupFAdd#`, `SubgroupFMax#`, `SubgroupFMin#`,
  `SubgroupBallot#`, `SubgroupBroadcast#`, `ShuffleDown#`, `ShuffleXor#`.
- **Atomics**: `AtomicAdd#/Sub/Or/And/Xor/Xchg/Cas`, `AtomicLoadN#`,
  `AtomicStoreN#`, `AtomicLoad#`, `AtomicStore#`.
- **Memory**: `Malloc#`, `Free#`, `Load#`, `Store#`, `Copy#`,
  `VolatileLoad#`, `VolatileStore#`.
- **Math / misc**: `Max#`, `Min#`, `Exp#`, `Sqrt#`, `Fma#`, `Fabs#`, …

`lib/std/gpu.bv` wraps the SPIR-V built-ins as `defn`s over these
(`get_global_id`, `barrier`, …).

### 3.4 The proof machinery

- **Contracts** `[pre][post]` on `defn`/`node`/`txn`/`asm` (mandatory on
  node/txn/asm). Omission ≠ `[true][true]` (explicit tautology is
  invalid).
- **Inline guards** `[cond] stmt;` and convergence gates `[cond];`.
- **`check <expr>;`** — a liveness assertion: proved at compile time
  (eliminated), disproved (compile error), or emitted as a runtime
  assertion; a successful check becomes a known fact.
- **`axiom`** prefix — postconditions taken on authority (ledgered;
  policy `allow|warn|deny` in `config/axioms.dbvl`).
- **`lemma_properties`** — a *closed, parse-validated vocabulary* of
  optimizer-exploitable properties an **op binding** may declare
  (`commutative` today). This is the existing algebraic-law facility.
- **Concurrency gate** (`concurrency_gate.rs`, Rule 22): per node pair,
  `sat(pre_A ∧ pre_B)` + `xor_overlap(read/write sets)`; eligible pairs
  must be classified with `async`/`sync<group>`.
- **Proof engine**: `check_satisfiable`, the axiom table, range/provenance
  analyses.

### 3.5 The tier synthesis (recognition debt)

SPEC §9.8: the SPIR-V backend selects a kernel form from the *proven
shape* — Tier 0 flat work-item, Tier 1 cooperative row, Tier 2 tiled GEMM,
Tier 3 cooperative matrix (tensor), Tier 4 planned native. "Recognition
conditions are exact… any body not matching a tier lowers to the plain
work-item kernel (always correct)." This is a **recognition-based**
delivery with a correct fallback. Under §2.1 it is **DEBT**: the tiers are
interim structural recognizers to be migrated to general derivation (with
a coverage ledger and Rule-24 retirement gates). See §7 A10, §10.

### 3.6 The `Asm#` facility

Two modes (Family F):

- **Abstract**: `Asm#("OpName", ops…)` lowers per target through
  `config/asm-lowering.dbvl` — a **data-driven** table
  (`OpName: arity; "target:template"; …`). Adding an op is a **data
  change**, so the expressiveness-closure property applies to the asm
  surface itself.
- **Raw**: `Asm#("raw", template, ops…)` — the author owns the fallout.

`SysCall#` already proves the emitter.

### 3.7 Frontend-driven dispatch

Structural decisions are computed once in the frontend
(`AnalysisResults`) and consumed by backends — e.g. `loop_shapes`,
`swan_songs`, `gpu_schedule`, `gpu_strategy`. The backend consumes
decisions; it does not make them
(`docs/plans/2026-07-31-frontend-driven-dispatch.md`). The cost model
`gpu_strategy::select(m,n,k,hw) -> Option<{tile_m,tile_n,stages}>` is the
shape chooser; config knobs (`ptx_tensor_stages`, `ptx_tensor_force_mw/nw`,
`ptx_gemm_grid_order`) already override it.

---

# Part II — The Problem and the Architecture

## 4. What GPU code needs that Briev cannot (yet) say

### 4.1 The reference-kernel exercise

To separate **assumption** (the compiler derives the machine) from
**genuine absence** (no Briev program expresses it), we took representative
high-performance kernels from other languages and tried to express them in
current Briev.

| Kernel | Expressible today? | Compiler *assumes/derives* | Genuinely **missing** |
|---|---|---|---|
| GEMM / CUTLASS | yes (`gemm.abv`) | tile, stage, tensor, vec4, dispatch, smem staging | explicit tile/stage/fragment/swizzle/split; TMA/wgmma primitives |
| FlashAttention FA2 | yes (online/2-pass) | warp-slice, deferred normalizer, reduction tree | explicit split-K factor; staging depth; persistent |
| Triton softmax | yes | reduction tree, warp-slice | (schedule optional) |
| GEMV | yes (`gemv.abv`) | tree, vec4 | — |
| Stencil / conv (halo) | yes | tiling, vec4 | explicit schedule |
| Affine gather / transpose | yes (`gather_8.abv`) | coalescing, vec4, swizzle | explicit swizzle control |
| Histogram (atomic bins) | yes w/ `AtomicAdd#` | — | shared/block-local binning; ordering scopes |
| Parallel scan (Blelloch) | **no** (only a sequential fold) | — | the parallel structure (shared scratch + phased barriers) |
| Warp-specialized pipeline | **no** | — | warp roles, `mbarrier`, TMA, clusters |
| Persistent kernel | **no** | — | the persistent-CTA model |
| Grid-wide cooperation | **no** | — | grid sync |
| Non-affine permutation scatter | proof **rejected** | — | an injectivity proof |
| Dynamic parallelism / multi-GPU | no | — | execution model |

**Notes on the hard cases.**

- **Parallel scan.** A scan is a sequential dependency, not a
  commutative/associative *fold* the compiler can re-tree. The parallel
  (Blelloch) form needs **shared scratch plus phased barriers** (upsweep /
  downsweep). Briev had *no shared-memory declaration* and no way to
  express the phased structure — a genuine gap spanning execution model
  (§5.2) and shape/coordination.
- **Non-affine scatter.** The affine proof (write affine in the counter)
  cannot establish disjointness for `dst[perm[i]]` where `perm` is an
  arbitrary permutation. The compiler is conservative and rejects; there
  was no way to supply the injectivity proof.
- **Persistent / grid / warp-specialized.** These are execution-model
  contracts a node/precondition model does not express; CUDA expresses
  them with cooperative launch, clusters, and hand-written warp roles.

**The test the exercise yields:**

> A kernel is **"assumed"** if Briev's algorithm-only form lowers
> correctly (fast or slow). It is **"genuinely missing"** if *no Briev
> program — even a slow one — expresses it.*

### 4.2 The gap taxonomy

Three kinds of gap, with different remedies:

1. **Derivation gap** — expressible, not yet optimized (e.g. a shape the
   tiers don't recognize). Remedy: general derivation (L5), never a
   keyword.
2. **Primitive gap** — hardware operation absent from the registry/table
   (e.g. TMA, wgmma, cluster ops). Remedy: a **data** addition to
   `asm-lowering.dbvl` / the registry (L1).
3. **Semantic gap** — no Briev program at all (scan's phased structure,
   persistent, grid sync, non-affine proof). Remedy: the syntax in Part
   III.

### 4.3 The critique that shaped the doctrine (and the responses)

An external critique raised the "sufficiently smart compiler" concern.
The responses, which the doctrine now contains:

- **"ASIC trap": hardware specialized around algorithms ⇒ the compiler
  must recognize GEMM.** Response: hardware capabilities become
  **fundamentals**; binding to a fundamental *by structural requirement*
  is instruction selection, not algorithm recognition; many algorithms
  bind to the same fundamental (§2.1, §5.4).
- **"NP-hard search."** Response: the doctrine uses a **cost model over a
  bounded candidate set**, not exhaustive search; heuristics over
  structure are allowed; only algorithm-name keying is forbidden.
- **"The year-two bar is impossible."** Response: the author declares
  **intent + contracts + structure** (composites/metaprogramming), and the
  compiler derives codegen — not "compiler invents FlashAttention from a
  bare contract." The repo has measured evidence (deferred-region emitter,
  chain fusion) of deriving attention structures.
- **"The doctrine contradicts itself by naming GEMM."** Response:
  correct — the row must be phrased by **structure + proof**, and target
  *requirements* live in the target profile (a fundamental's requirement),
  separate from programme structural facts. The reference exercise (§4.1)
  is the concrete acceptance corpus.

## 5. The architecture: five layers

The architecture separates **who declares what** into five layers, from
hardware up to derivation. Each layer's syntax is fixed in Part III.

### 5.1 L1 — Primitive layer (every instruction/feature reachable)

The raw machine. **Not** a keyword surface: fundamental **types** for
units, the **`Asm#` abstract lowering table** (data), and the **intrinsic
registry** (ergonomic named intrinsics). Coverage is audited against each
target ISA; missing rows are **data additions**, not Rust changes.

### 5.2 L2 — Execution model (the realities)

Structural contracts the DAG does not determine: grid-wide sync,
persistent/grid-stride, warp specialization, clusters/DSMEM, dynamic
parallelism, multi-GPU, memory-ordering scopes. Expressed in Part III as
**scope blocks** + `sync<…>` + the `persistent` modifier.

### 5.3 L3 — Shape (codegen strategy)

CUTLASS exists because the tensor path must be hand-shaped. Briev derives
the shape by default; where the cost model is **uncertain**, the author
declares it as an **ambiguity** (node parametric modifiers). Not a
scheduling surface (§2.5) — shape is intra-node codegen structure.

### 5.4 L4 — Fundamental binding

The target profile carries **fundamentals** (instruction + structural
requirement); the compiler binds structures by requirement. Explicit
binding is type-level (§6 D11/D13/D21) for required semantics or a
derivation gap (retirement-gated).

### 5.5 L5 — Derivation (the default win)

The whole-program DAG + proofs give Briev fusion, proof-licensed
transforms, scheduling, and per-shape/per-device selection CUDA cannot
discover. Migrating the §3.5 tiers to general passes is where "beat CUDA
by default" is won.

### 5.6 Layer matrix

| Layer | What it expresses | Mechanism | Derive vs declare |
|---|---|---|---|
| L1 primitives | hardware operations/units | types; `Asm#` table; registry | binding derived; explicit for required semantics/gap |
| L2 execution model | cooperation/lifetime/launch | `scope<…>`, `sync<…>`, `persistent` | derive; declare when required |
| L3 shape | tile/stage/fragment/swizzle/unroll/split | node parametric modifiers | derive; declare as **ambiguity** when uncertain |
| L4 fundamentals | which hardware unit | fundamental types + requirements | derive by requirement; declare for semantics/gap |
| L5 derivation | the whole machine | general passes over DAG + proofs | always derived (the default) |

---

# Part III — Decision Record

Each decision below gives **context · problem · alternatives · decision ·
rationale · grammar · semantics · examples · interactions**.

## 6. Syntax decisions (D1–D27)

### D1 — Shape declarations are node-level parametric modifiers. **DECIDED**

**Context.** Explicit shape control is the CUTLASS-class escape (L3). It
must be declared only where the cost model is uncertain (§2.8).

**Alternatives.** (i) Node-level parametric modifiers; (ii) loop-level
clause; (iii) a `schedule { … }` block; (iv) metaprogramming `$defn`
schedules; (v) chain-carried.

**Decision.** **Node-level parametric modifiers**, reusing the existing
order-free modifier run (`accel node`, `seq node`, `sync<g> async node`).

**Rationale.** The modifier mechanism already exists
(`parse_modifier_prefixed`), is order-free, parametric (`sync<g>`), and
disclosed. It adds no new grammar form — only new names. One schedule per
node matches `.abv` kernels (usually one dominant loop).

**Grammar.** `<shape-modifier>* <other-modifiers>* node NAME [pre][post] { … }`.
See D14 for the shape-modifier names.

**Semantics.** The declared shape is a **decision** threaded
`AST/annotations → AnalysisResults → backend` (frontend-driven dispatch,
A7). The backend validates legality (divisibility, smem/regs limits).

**Interactions.** D2 (precedence), D14 (vocabulary), A7 (threading), §9.1
(the multi-node-scope rule).

### D2 — Precedence: source > config > model; override + redundancy warning. **DECIDED**

**Context.** A declared shape must interact with the cost model's default
and with existing config overrides.

**Alternatives.** (i) declaration always wins (silent); (ii) declaration is
advice the model may reject; (iii) declaration wins with a **redundancy
warning** when the model was certain; error when illegal.

**Decision.** **(iii) override + redundancy warning.** Source declaration
> config override > model default. If the model was certain (the declared
shape equals or is dominated by the derived one), the compiler emits a
*"redundant declaration"* warning disclosing the default. If the declared
shape is illegal, it **errors**.

**Rationale.** The doctrine's "default must win where certain" (§2.8) is
surfaced as a warning rather than silence, so the author learns the
default was already optimal (no hidden "default leaked" case). Legality is
the compiler's duty; invalid shapes error rather than miscompile.

**Interactions.** D1, D9 (persistent uses the same pattern).

### D3 — Execution-model features are declared with scope blocks. **DECIDED**

**Context.** L2 realities (cooperation, lifetime, launch) are not
expressible by preconditions.

**Alternatives.** (i) node modifiers; (ii) intrinsics only; (iii) scope
blocks; (iv) config/profile.

**Decision.** **Scope blocks.**

**Rationale.** A scope is a *region* that owns **shared state** and
synchronization — a grouping/definition construct (`{}`), not a property
of a single node. It can nest (`grid ⊃ cluster ⊃ workgroup ⊃ subgroup`,
D16) and can wrap several nodes (D24).

### D4 — Synchronization extends `sync<…>`. **DECIDED**

**Context.** In-kernel barriers are needed; the language already has
`sync<group>` (node classification).

**Alternatives.** (i) a new keyword family; (ii) extend `sync<…>`; (iii)
bare statement keywords.

**Decision.** **Extend `sync<…>`.**

**Rationale.** One keyword for "synchronization," parameterized by scope
via `<>` (a compile-time-specialization load — honest). Avoiding a second
synchronization vocabulary.

**Interactions.** D7 (two domains), D15 (scope words).

### D5 — Scope-block form: `scope<kind[, size]> { … }`. **DECIDED**

**Alternatives.** (i) `scope<kind[, params]> { … }`; (ii) keyword-per-kind
(`workgroup { … }`); (iii) `scope { kind …; }`.

**Decision.** **(i) unified `scope<kind[, size]> { … }`.**

**Rationale.** `<>` carries the specialization (kind + size), `{}` the
region — delimiter-honest (Rule 21), uniform, and it nests naturally.

**Grammar.** `scope<workgroup>` | `scope<workgroup, 256>` |
`scope<subgroup, 32>` | `scope<cluster, 2>` | `scope<grid>`.

### D6 — `persistent` is a separate lifetime contract. **DECIDED**

**Decision.** Persistent is a **lifetime axis**, not a cooperation-scope
kind; it does not appear inside `scope<…>`. (See D9 for its form.)

**Rationale.** "Who cooperates" (scope) and "how long the CTA lives"
(lifetime) are orthogonal; conflating them overloads `scope<…>`.

### D7 — `sync<…>`: one keyword, two domains. **DECIDED**

**Decision.** `sync<group>` = node classification (Rule 22);
`sync<subgroup|workgroup|cluster|grid>` = in-kernel barriers. The `<>`
value distinguishes the domain; both documented.

**Rationale.** Minimal vocabulary; the semantics ("synchronize these
members") is consistent across domains. (OPEN: whether to rename the
node-classification value, §11.)

### D8 — Dedicated `shared` declaration. **DECIDED**

**Column.** Shared state is the **medium of cooperation** — the memory
that `sync<…>` orders and the scoped-sharing proof covers. It is not
merely a placement, so unlike global/register (derived), it earns a
declaration.

**Alternatives.** (i) `let` + `spec Space: Shared`; (ii) a dedicated
`shared` declaration; (iii) derive all placement.

**Decision.** **Dedicated `shared` declaration** (form D23).

### D9 — Persistent: derived by default; `persistent node` to require. **DECIDED**

**Context — why persistent at all.** A normal launch pays per-dispatch
overhead (submission ~7 µs + fence-wake ~33 µs measured on this device
class), wave-quantization tail effects, and per-kernel fences plus HBM
round-trips in a pipeline. A **persistent** kernel launches exactly enough
CTAs to fill the machine once and has each loop over many tiles/work-items,
which (1) amortizes launch/teardown, (2) eliminates the tail, (3) keeps
state resident across tiles, (4) enables **megakernel** fusion of a whole
pipeline with on-chip intermediates, and (5) enables iterative/cross-tile
algorithms (CG, work-stealing, recurrent decode). It is how the fastest
modern kernels are structured. CUDA cannot express it *automatically* (its
unit is the launch); Briev's whole-program DAG can.

**Alternatives.** (i) always author-declared; (ii) derived by default +
`persistent node` to require; (iii) a `persistent { … }` block.

**Decision.** **(ii) derived by default**, with a **`persistent node`**
modifier to *require* it. Redundant requirement warns (D2 pattern).

**Rationale.** The megakernel/pipeline win should be **automatic** where
beneficial (the default is the compiler's job); the modifier exists for
the case where the author wants to *require* a resident-CTA structure the
cost model may not choose (a declared intent). It is a lifetime contract,
orthogonal to scope (D6).

### D10 — `shared` is legal only inside a `scope<…>` block. **DECIDED**

**Decision.** `shared` outside a scope block is an error.

**Rationale.** The enclosing scope **names the cooperating set** and
bounds the buffer's lifetime; without it, sharing has no defined scope and
the compiler cannot prove scoped sharing.

### D11 — Fundamental binding is type-level. **DECIDED**

**Context.** The compiler binds structures to fundamentals by requirement
(L4); an explicit binding is the escape for required semantics or a
derivation gap.

**Alternatives.** (i) a `bind … to …` clause; (ii) `<>` on the op; (iii)
new fundamental **types/hashwords**; (iv) `Asm#` only.

**Decision.** **(iii) new fundamental types** (not a modifier/clause).

**Rationale.** The repo *already* binds the tensor path by operand **type**
(`Float16` fields → `VK_KHR_cooperative_matrix`); making it a first-class
fundamental type generalizes an existing mechanism. `bind` collides with
the electronics persist-tighten keyword; a clause is a new grammar form.

### D12 — Broad shape-modifier vocabulary. **DECIDED**

**Decision.** First-class: `tile`, `stage`, `unroll`, `split`, `vector`,
`swizzle`, `fragment` (+ `pipeline` folded into `stage`). See D14 for
parameters.

### D13 — A fundamental type names the unit (`Tensor<…>` / `Mma<…>`). **DECIDED**

**Rationale.** Units are semantic operands, so they are types
(fundamentals-as-types) — not annotations (`#Hashword`) and not physical
metadata (`spec`).

### D14 — Shape-modifier names and parameters. **DECIDED**

- `tile<M, N>` — CTA tile rows×cols.
- `stage<N>` — software-pipeline depth (`stage` = the ring size; `pipeline`
  folds here).
- `unroll<N>` — loop-body replication factor.
- `split<N>` — split-reduction factor (the split in
  `2026-09-30-general-reduction-split.md`).
- `vector<N>` — vector width (e.g. `vector<4>` for a 4-wide load).
- `swizzle<Xor | N>` — shared-memory swizzle pattern (`Xor` = the classic
  XOR-by-chunk; `N` a width).
- `fragment<mXnXk>` — register-fragment shape.

**Rationale.** The divisions carry one meaning each (Rule 21): `tile`/
`stage`/… are the *names*; `<>` carries their compile-time parameters.

### D15 — Scope words: `subgroup`/`workgroup`/`cluster`/`grid`. **DECIDED**

**Decision.** Portable, vendor-neutral terms; NVIDIA `warp` ≡ `subgroup`.

**Rationale.** Briev is multi-vendor; `Subgroup*#` intrinsics already use
the portable term; `warp`/`block` would bake one vendor's vocabulary in.

### D16 — `workgroup` is implicit; others require the scope. **DECIDED**

**Decision.** `sync<workgroup>` works with no `scope` block (implicit
default). `shared` and `sync<subgroup|cluster|grid>` require the matching
`scope<…>`. `sync<grid>` must be inside `scope<grid>`.

**Rationale.** Workgroup is the natural GPU default cooperation unit;
higher scopes (cluster/grid) and non-default cooperation are *declared*
because they change the launch/lifetime contract.

### D17 — Non-affine disjointness is licensed by `injective`. **DECIDED**

**Context.** The affine proof rejects `dst[perm[i]]` (permutation
scatter), though the writes are disjoint.

**Decision.** The author supplies an **injectivity** fact; the grammar is
D19. Until supplied, the compiler stays conservative.

### D18 — Ambiguity surfacing is diagnostics-only. **DECIDED**

**Decision.** No mandatory annotation; the compiler **warns its default
pick** / **errors when it needs a decision**, naming the resolving keyword
(e.g. `store`/`free`). Errors never silenceable; warnings policy-controlled
(OPEN, §11).

### D19 — `injective`/laws live in the extended `lemma_properties`. **DECIDED**

**Context.** Briev already has a closed, parse-validated vocabulary of
optimizer-exploitable properties on op bindings (`lemma_properties:
commutative;`), plus `check` and `axiom`.

**Alternatives.** (i) extend `lemma_properties`; (ii) a `check` assertion
at the site; (iii) a contract clause; (iv) `axiom`-authorized.

**Decision.** **(i) extend `lemma_properties`** with `injective`,
`associative`, `idempotent`, … (one facility for injectivity proofs *and*
algebraic laws; data-driven; parse-validated; a gap becomes a data change
where possible).

### D20 — Laws attach both to the op binding and (optionally) the fold. **DECIDED**

**Decision.** The **op binding is canonical** (`op Add` carries
`associative, commutative`); a **fold may locally assert/rely** on a law
(e.g. to license a split) when local facts matter.

**Rationale.** Laws travelling with the operation is the general form (any
fold of `Add` may be split); a local assertion covers cases the operation
alone cannot (constrained domains).

### D21 — `Tensor<elem, M, N, K>`. **DECIDED**

**Decision.** Element type plus the fragment/instruction shape, e.g.
`Tensor<Float16, 16, 8, 16>`. Compiles to the target's matching
mma/coopmat fragment; the target profile says which shapes exist (L4).

### D22 — `scope<…>` size is optional and derived by default. **DECIDED**

**Decision.** `scope<workgroup, 256>` declares a size; `scope<workgroup>`
lets the cost model derive it. (OPEN: is a declared size a hard
requirement or a hint the model may override with a remark — §11.)

### D23 — `shared name: T[N];`. **DECIDED**

**Decision.** A dedicated leading keyword (like `let`/`const`), legal only
inside a `scope<…>` block (D10). The bound is a compile-time expression;
the scope bounds the lifetime.

### D24 — `scope<…>` has two roles: intra-node region and multi-node fusion. **DECIDED**

**Decision.** A `scope<…>` may appear as a **statement in a node/txn
body** (a region) *and* as a **top-level block wrapping multiple nodes**,
which the compiler fuses into **one kernel**.

**Rationale.** The multi-node role is the author-directed **megakernel**:
phases of one dispatch share the scope's memory. It unifies the reactor
(phases), the scope (shared + sync), and fusion (now *declared* in
addition to *derived* by `gpu_schedule`, A9).

### D25 — Fused phases: preconditions sequence; `sync<…>` barriers; `shared` spans; fusion proven. **DECIDED**

**Decision.** Phase order is by **preconditions** (the reactor is the
scheduler, §2.5); `sync<…>` marks where a barrier is required; `shared`
declared in the scope **persists across phases**. Fusion is **proven**
(legal only where the phases' read/write sets permit), never assumed.

**Rationale.** Keeps scheduling = preconditions (no new scheduler);
requires the compiler to *prove* fusion legality (correctness first); the
scope owns the shared lifetime.

### D26 — Primitive-addition path is layered. **DECIDED**

**Decision.** Fundamental **types** for units; the **`Asm#` abstract
lowering table** (`config/asm-lowering.dbvl`, data-driven) for raw
operations; the **intrinsic registry** for ergonomic named intrinsics.
Adding a hardware primitive is *usually a data row*, not Rust.

**Rationale.** The closure property (`briev-capability-frontier.md`)
applies to the asm surface; named intrinsics exist where ergonomics merit
it.

### D27 — First target scope: NVIDIA PTX (sm_80/86/90/100) + SPIR-V. **DECIDED**

**Decision.** Audit and cover the active targets first; AMD/Intel later as
projections of the same plan.

## 7. Architectural decisions (A1–A10)

- **A1 — One frontend plan, per-target projections** (doctrine §4). The
  `KernelPlan` (`2026-09-30-kernel-plan-and-per-target-lowering.md`) is the
  target-independent substrate; PTX and SPIR-V are lowerings. Frontends are
  never detached; the syntax in Part III lowers *through* the plan.
- **A2 — Per-lane kernel projections** (`RunnerKernel.owner`/`domain`, the
  "(B)" refactor): one node may have different kernel sets per lane. This
  absorbs the tensor dual-image special case and the split; it is the
  runner-level realization of "one plan, per-target projections."
- **A3 — Fundamentals catalog on `TargetProfile`** (instruction +
  structural requirement) and binding by requirement in the lowerings
  (L4). Explicit binding is type-level (D11).
- **A4 — Preconditions are the scheduler**; no new scheduling construct.
  `chain`/`into` remain guard-erogonomics (§2.5).
- **A5 — Ambiguity is a first-class compiler concept**: a general analysis
  producing default/warn/error per class; existing detectors (concurrency
  gate, volatility ranges, liveness, cost-model margin) are class
  producers. Required by D2/D9/D18.
- **A6 — Cost model exposes uncertainty**: `select` must surface the
  ranked candidates / the gap between the top two, not just one shape, so
  the compiler can warn honestly (implementation item).
- **A7 — Declared shapes/contracts are threaded AST/annotations →
  `AnalysisResults` → backend** (frontend-driven dispatch).
- **A8 — `lemma_properties` is the law/injectivity facility** (D19);
  extend its closed vocabulary, not a new declaration form.
- **A9 — Multi-node scope = authored fusion** (D24); the derived
  `gpu_schedule` chain fusion becomes the automatic form of the same
  thing. Reconciliation rule when both apply: OPEN (§11).
- **A10 — Retirement ledger**: every handwritten cooperative emitter
  (deferred region, warp-slice, lane-reduction, cp.async pipeline) is an
  **existence proof** with a Rule-24 retirement gate that the general
  machinery (L5) drives the declared composite to its numbers.

## 8. Worked end-to-end examples

### 8.1 Multi-node fusion — an authored megakernel

```briev
scope<workgroup, 256> {
    shared tile: Float[128 * 16];        // spans the phases (D25)
    node load    [i < T][i == T] { /* fill tile */ }
    node compute [i < T][i == T] { /* consume tile; sync<workgroup> where needed */ }
}
```
The compiler fuses the two nodes into **one kernel** (D24); the
preconditions sequence the phases; the fusion is **proven** from the
read/write sets (D25). `tile` is shared across phases.

### 8.2 Single kernel — shape on the node, tensor via the fundamental type

```briev
tile<128,256> stage<3> unroll<4> persistent node gemm [i < M*N][i == M*N] {
    let a: Tensor<Float16, 16, 8, 16>;   // binds the tensor fundamental (D13/D21)
    /* naive matmul body; the compiler derives the rest */
    scope<subgroup> { sync<subgroup>; }   // in-kernel barrier (D4/D15)
}
```
`tile`/`stage`/`unroll` ride the node (D1/D14); `persistent` is the
lifetime modifier (D9); `Tensor<…>` binds the unit (D13/D21); the body
stays algorithm-only.

### 8.3 Grid cooperation + shared staging

```briev
scope<grid> {
    scope<workgroup, 256> {
        shared s: Float[4096];
        sync<workgroup>;
    }
    sync<grid>;                            // requires scope<grid> (D10/D16)
}
```

### 8.4 Non-affine scatter licensed by a law

```briev
// The index map declares its injectivity; the compiler consumes it (D17/D19).
// lemma_properties: injective;   (on the op/defn producing perm)
foreach i in 0..N { dst[perm[i]] = src[i]; }   // writes proven disjoint
```

### 8.5 Persistent megakernel pipeline

```briev
persistent node decode [i < STEPS][i == STEPS] {
    // one resident CTA processes many steps; qk→softmax→pv fused as a scope
    scope<workgroup> {
        shared kv_tile: Float[...];
        node qk  [..][..] { … }
        node pv  [..][..] { … }
    }
}
```

---

# Part IV — Contract, Roadmap, Open Items

## 9. Consistency invariants

1. **No keyword required for an optimum the compiler can derive** — the
   default must win where certain (D2/D9); declarations are ambiguity
   escapes or required-semantics escapes.
2. **Scope/phase order**: `grid ⊃ cluster ⊃ workgroup ⊃ subgroup`
   (D15/D16); `shared` and non-workgroup `sync` require their scope.
3. **Multi-node scope ⇒ one kernel**; phase order by preconditions
   (D24/D25); fusion proven.
4. **`sync<…>` has two domains** — node classification vs in-kernel — one
   keyword, documented (D7).
5. **Fundamental binding is type-level; shape is modifier-level; execution
   model is scope/type-level** (D1/D3/D11).
6. **No recognition**: every new pass keys on structure + proofs.
7. **Closure**: every capability is reachable; a gap is a bug.

## 10. Implementation roadmap

The syntax lowers *through* the `KernelPlan` machinery. Recommended order
(the syntax is the design contract; the substrate is `KernelPlan`):

1. **KernelPlan Phase 2** (`2026-09-30-kernel-plan-and-per-target-lowering.md`):
   `GpuLowering` trait + per-lane projections (A2) + absorb the tensor
   dual-image case + delete the S1 `split` field.
2. **Split as `ReduceTree::Split`** (device-validate `5a`); add the
   `split<N>` modifier later.
3. **L1 primitive audit** (D26/D27): diff the intrinsic registry +
   `asm-lowering.dbvl` against PTX/SPIR-V; fill via data rows.
4. **L2 scope blocks** (D3–D10, D15–D16, D22, D24–D25): parser →
   `AnalysisResults` → backend; the runner/desc mapping (OPEN §11).
5. **L3 shape modifiers** (D1/D2/D14) + the cost-model uncertainty signal
   (A6) + redundancy warnings.
6. **L4 fundamentals as types** (D11/D13/D21) + the `TargetProfile`
   catalog (A3).
7. **Laws/injectivity** (D19/D20): extend `lemma_properties`; consume in
   the reduction/split and access-disjointness analyses.
8. **Tier → derivation migration** (A10): retire the SPEC §9.8
   recognizers with coverage-ledger + retirement gates.
9. **Governance**: recognition gate, orphan-fact gate, keyword-observability
   gate, coverage ledger.

## 11. Open items (recorded, not dropped)

1. **OPEN — shape on a multi-node scope**: D1 puts shape on the node; D24
   fuses several nodes into one kernel with one shape. Rule: shape
   modifiers on the node for single-node kernels, on the `scope<…>` for
   fused multi-node kernels. (Refines D1/§9.1.)
2. **OPEN — exact `lemma_properties` vocabulary**: which laws
   (`associative`, `commutative`, `idempotent`, `injective`, `distributive`,
   `monotone`, `bounded`, …), their arities/forms, and validation.
3. **OPEN — ambiguity warning policy**: per-module control
   (`!> ambiguity: allow|warn|deny;`)? Errors never silenceable.
4. **OPEN — cost-model uncertainty signal (A6)**: the exact margin /
   calibration-range test that turns a shape choice into a warning.
5. **OPEN — multi-node scope → runner/desc mapping (A2/A9)**: how fused
   phases map to one `RunnerKernel`, block geometry, shared layout, and
   the launch.
6. **OPEN — `sync<group>` vs `sync<workgroup>`**: rename the
   node-classification value (`nodes`/`peer`) or keep the dual domain?
7. **OPEN — `scope<…>` size semantics**: declared size = hard requirement
   or a hint the cost model may override with a remark?
8. **OPEN — `Tensor<…>` validation**: allowed elements, shape
   divisibility, interaction with `spec`.
9. **OPEN — shape-modifier parameter grammar** beyond D14 (`swizzle<…>`
   semantics, `fragment<mXnXk>` layout space, `vector<N>` ceilings).
10. **OPEN — dynamic parallelism / multi-GPU**: in scope (L2) but the
    declaration form is undecided.
11. **OPEN — authored vs derived fusion** (`scope` vs `gpu_schedule`):
    the reconciliation rule when both apply.

## 12. Glossary

- **Derivation / recognition** — §2.1.
- **Strategy / ambiguity / intrinsic keyword** — §2.2.
- **Observability razor** — §2.3.
- **Precondition (guard)** — the node firing condition; the scheduler.
- **Stage** — one buffer slot in a software-pipeline ring; `stage<N>` sets
  the depth (prefetch distance `N−1`), trading shared memory for latency
  hiding.
- **Fundamental** — a hardware capability the compiler knows, bound by a
  structural requirement; named by a fundamental *type*.
- **Scope** — a cooperation region (`scope<subgroup|workgroup|cluster|grid>`).
- **Shape** — intra-node codegen structure (tile/stage/fragment/…), as
  opposed to *scheduling*.
- **Ambiguity** — an observably-distinct choice the program does not
  determine; resolved by a keyword at the site, or warn/error.
- **Closure** — expressiveness closure (§2.9).

## 13. Cross-references

- `derivation-not-recognition.md` — the obligation; the recognition gate;
  coverage ledger; acceptance test.
- `keyword-taxonomy.md` — the three-category reasoning (its open questions
  are decided here, §6).
- `2026-09-30-hardware-manipulation-expressiveness.md` — the five-layer
  plan (L3 renamed *shape*; forks resolved here).
- `2026-09-30-kernel-plan-and-per-target-lowering.md` — the machinery.
- `2026-09-30-general-reduction-split.md` — the split (declarable as
  `split<N>`, licensed by `associative`).
- `proof-vs-shape.md`, `briev-capability-frontier.md`,
  `briev-vs-cuda-thesis.md`, `abv-gpu-doctrine.md`,
  `gpu-backend-strategy.md`, `briev-execution-model.md`,
  `2026-09-22-core-chain-into.md`.
- Golden Rules 2, 3, 22, 23, 24.
