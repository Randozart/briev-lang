# Keyword Taxonomy and Ambiguity Keywords — discussion capture

**2026-09-30.** Status: **discussion record — decisions pending.** This
captures a design discussion; it does not yet amend the Golden Rules. It
exists so the decisions are made deliberately, not by drift.

> **Update (2026-09-30, later):** the open questions below were decided in
> `gpu-syntax-decision-record.md` (§6 D1–D27). Read that for the resolved
> syntax; this document remains as the reasoning record of the three
> categories and the resolution rule.

**Companions:** `proof-vs-shape.md` (eternal vs temporal),
`derivation-not-recognition.md` (the compiler's obligation),
`briev-capability-frontier.md` (expressiveness closure),
`briev-vs-cuda-thesis.md` (why proof beats UB), `abv-gpu-doctrine.md`
(one plan, per-vendor projections),
`docs/plans/2026-09-30-hardware-manipulation-expressiveness.md`
(reach every hardware capability — the manipulation surface).
**Feeds:** Golden Rules 2, 3, 22.

---

## 0. Why this document exists

We were designing a GPU "cooperative execution" surface and kept circling
one question: *what may a keyword ever do, and what must the compiler
derive on its own?* The discussion produced (a) a distinction that had been
collapsed, and (b) a resolution rule. Both are recorded here, unresolved.

## 1. The premise: declare intent, derive everything determinate

Briev's compiler reads the whole program — the reactive node DAG, the
contracts, the proofs (disjointness, single-writer, lifetimes,
associativity), the iteration structure, the layout intent. From that it
must derive the execution. The obligation:

> **Every determinate choice is the compiler's to make, and the compiler's
> default must be the fastest code for the declared semantics.** A keyword
> is legitimate only where something is *not* determinate.

The machine form of "not determinate" is **ambiguity**.

## 2. Three distinct categories (do not conflate)

Earlier in the discussion two different things were folded together. They
are separate:

### 2.1 Strategy keywords
`seq`, `vol`, `pack`, `async`, `sync<group>`, `atomic`, `union`, `trap`
(the existing Rule 2 set).

- Express **intended behaviour** the plain efficient codegen would
  otherwise choose differently.
- **May never lead to faster code.** The fastest code for the *same
  semantics* must already be the default; a strategy keyword only
  **constrains** (correctness/intent). "Requiring a keyword to win is a
  failing default."
- They say: *"do it this way (even if slower) because the default would be
  semantically wrong."*

### 2.2 Ambiguity keywords
The category this discussion named. Canonical example: **memory —
`store` or `free`.**

- **Declared at the site of confusion.**
- Used when a **strategy must be chosen** and the **effectiveness of either
  strategy is uncertain and up to intent**.
- They say: *"this is what I meant"* — where the compiler has no
  determinate basis to pick.
- Because the compiler *cannot rank* the strategies, there is no
  "fastest default" to leak; any speed difference is a consequence of the
  author's declared intent.

### 2.3 Intrinsics / fundamentals
Hardware primitives the compiler knows (`Sqrt#`, `Mmap#`,
`SubgroupFAdd#`, …; the target's instruction set). A **separate category**
from both keyword kinds: they name machinery, not behaviour or ambiguity.
Author-facing access to one is legitimate only for a *required hardware
semantics* or as a *retirement-gated derivation gap* — never as a speed
lever.

## 3. The discriminator

| | Strategy keyword | Ambiguity keyword |
|---|---|---|
| Trigger | default codegen would violate intended **behaviour** | compiler must **choose a strategy** it cannot rank |
| Nature | a **constraint** | a **selection** |
| Speed | **never** faster (default already fastest for the semantics) | may differ; no determinate default exists |
| Placement | rides the declaration/modifier | **declared at the site of confusion** |
| Disclosure | explicit, by the author | compiler **warns its default pick**; **errors** when it truly needs the decision |

## 4. The observability razor (from the discussion)

> A keyword is legitimate **iff removing it changes observable
> behaviour**. Speed is never the argument — it is at most a consequence.

Corollaries:
- A keyword whose removal preserves all observable behaviour is a
  **performance hint in disguise** → fix the default; never keyword it.
- A keyword for a choice the contracts already determine is **redundant**
  (the compiler derives it) → bug.

The razor is stated in *observable behaviour*, not performance, so it
survives the "semantic difference happens to be faster" case.

## 5. Ambiguity resolution rule (the warn/error arm)

> **The compiler never resolves an ambiguity silently.**
> - **Benign** (a defensible default exists): it **picks the default and
>   warns**, disclosing the pick and the keyword that would override it.
> - **Material** (no defensible default / the wrong pick is unsound): it
>   **errors**, demanding the decision.

Severity criterion — **soundness**, not taste:
- **Error** iff the compiler cannot pick a behaviour consistent with the
  declared contracts (any choice is arbitrary, or the wrong choice is
  unsound: a race, a use-after-free, an observing reorder). The program is
  under-specified.
- **Warn** iff a defensible default exists but intent may differ.

This merges, without contradiction: Rule 2 (the default), Rule 3 (the
warning discloses it), Rule 22 (the error arm — unclassified eligible
pairs are a hard error).

Illustrative placements (to be decided):

| Ambiguity | Default pick | Arm |
|---|---|---|
| Simultaneity (`async`/`sync<group>`) | none defensible | **error** (Rule 22) |
| Memory ownership (`store`/`free`) | free at scope end | **warn** if sound; **error** if a free would be unsound |
| Volatility (`vol`) | non-volatile | **warn** if the address looks MMIO/IO |
| Atomicity (`atomic`) | non-atomic | **error** if overlapping writes can't be proven safe |

## 6. What this implies for the compiler

- **Everything determinate is derivable.** The only non-derivable thing is
  **ambiguity resolution** — so the set of legitimate keywords is exactly
  the set of observable ambiguities we admit.
- The compiler owes a **general ambiguity analysis**: per class, it
  produces a default pick, a warning, or an error. Existing detectors
  (concurrency gate, volatility ranges, liveness) become class producers.
- "Draw the DAG" answers the cooperation question: parallelism, tiling,
  staging, fusion, reduction trees, barrier placement, memory placement,
  instruction binding are all **derivable** from the DAG + proofs +
  `atomic`/`sync`. There is no need for new GPU scope/shape keywords; a
  missing derivation is a **derivation backlog**, not an expressiveness
  gap.

## 7. Open questions (decisions pending)

1. **Discriminator**: is it confirmed that strategy = behaviour constraint
   (never faster; default fastest) and ambiguity = strategy selection the
   compiler cannot rank, declared at the site, up to intent?
2. **`async`/`sync<group>`/`atomic`**: strategy keywords (required
   concurrency/atomicity behaviour) or ambiguity keywords (concurrency
   strategy selection)? Rule 2 currently lists them as strategy keywords.
3. **Placement**: is "declared at the site of confusion" *definitive* of
   ambiguity keywords, versus strategy keywords riding declarations?
4. **Warn/error scope**: does the warn-the-default / error-when-needed rule
   apply **only** to ambiguity keywords, with strategy keywords simply
   being explicit constraints?
5. **Ambiguity classes**: memory (`store`/`free`) canonical — what others
   (schedule choice, layout choice, accumulator/precision choice)?
6. **Intrinsics/fundamentals as a third category** — confirmed separate?
7. **Documentation home**: this doc, or a section in
   `derivation-not-recognition.md`?

## 8. Non-decisions (explicitly *not* adopted)

- No new GPU scope/shape/strategy keywords. Cooperation is to be derived;
  `atomic`/`sync<group>` cover the ambiguous cases.
- The earlier provisional "ambiguity keyword taxonomy" (which listed
  `seq`/`vol`/… as ambiguity keywords) is **withdrawn** — those are
  strategy keywords.
- A keyword that is faster only because the default was deficient is a
  **bug**, not a feature.

## Numeric ambiguity pair: `approx` / `exact` (2026-11-08, P1)

- **Category:** ambiguity keywords (declared at the site of confusion; never
  speed keywords).
- **Problem:** float add/mul are not associative — reassociating a reduction
  changes rounding vs source-order evaluation. The default is precise.
- **`approx`** (node/txn prefix, and per-fold `approx foreach`): the numeric
  result may deviate within FP rounding; licenses float fold reassociation
  (parallel reduction lane).
  Planned stdlib compression: `approx discards the precise right of this node`;
  consumed by the fold lane, disclosed in diagnostics.
- **`exact`** (node/txn prefix, and per-fold `exact foreach`): asserts the
  precise sequential result is required; suppresses the parallel fold lane;
  pins no contraction. A per-fold declaration overrides the node-level pair;
  `seq` outranks both. An
  assertion, not a requirement (the default is already precise). `exact` +
  *proven* exactness may still use the parallel lane when the derivation tier
  lands (result identical).
- **Both:** contradiction error.
- **Not:** a speed keyword, a contract weakening, or a global `-ffast-math`
  policy. `!>` numeric keys are not the author surface (retirement pending).

## The `seq` contract (2026-11-08)

`seq` asserts the declared order is part of the observable semantics:

- **Declarations:** `seq struct` = declaration-order layout; `seq let`
  array = sequential/predictable layout, compiler layout optimizations
  (16B alignment, vec4/pair retypes) disabled. Layout is observable across
  FFI/raw-storage boundaries.
- **Loops:** `seq foreach` = iteration order is observable — no reordering,
  overlap, or parallelization that changes the order of effects (FFI calls,
  overlapping writes, ordered accumulation, `break`).

No reordering/vectorization is a **consequence**, not the purpose. If the
order is provably unobservable, the declaration is redundant (razor) and is
diagnosed when the proof is available. `seq` never makes code faster;
requiring it to win is a failing default.

Relationship to the numeric pair: `exact` = the numeric result must match
precise source-order evaluation; `approx` = rounding may differ; `seq` = the
effect/iteration order is observable (which also pins float accumulation
order, so a `seq` float fold is not reassociated). `seq` + `approx` can
coexist: order pinned across iterations, per-op contraction still permitted.
