# General Machinery: GPU Vocabulary Retirement Program

**Date:** 2026-09-19
**Status:** approved (supersedes `2026-09-19-flashdecode-emitter.md` — see §8)
**Doctrine:** capability-frontier + layout-contracts; user decision
2026-09-19: **we build general machinery so well tuned that shapes like
fused decode attention emerge from it by sheer efficiency — they are not
recognized by it.**

## 1. The decision

The flash-decode gate (2026-09-19, PASS: 123 µs f32 / 94 µs f16 vs 202 µs
best chain) proved the target shape's worth. The follow-up plan proposed a
`FlashDecodeInfo` frontend recognizer + a PTX template emitter with the
ggml decode layout baked in. Rejected in review: that is application-level
special-casing — a sentence, not vocabulary — and it would have been the
third attention-shaped arm in the tree.

Instead, the gate kernel's advantage **decomposes into four general
mechanisms**, each independently buildable, measurable, and useful beyond
attention:

| gate kernel's advantage | the general mechanism |
|---|---|
| 3 launches → 1; s/o1 never materialized | producer-consumer kernel-chain fusion (dataflow) |
| softmax's `/sum` applied inside pv's loop | deferred-normalizer rewrite (division by a loop-completed Σ distributes over any linear consumer accumulator) |
| pv's serial 4096-j loop latency-bound | warp-sliced reductions (generalizes lane-reduction: j-split across warps + smem merge) |
| softmax 3-pass → online (m,l) | the softmax vocabulary's fused lowering — softmax is already recognized vocabulary; this improves its lowering |

Flash-decode = chain-fusion ∘ deferred-normalizer ∘ warp-sliced-reductions
∘ online-softmax-lowering applied to the natural three-node .abv the
author already writes. The shape EMERGES; nothing recognizes it.

## 2. The two tiers (vocabulary doctrine)

| Tier | Mechanism | In the tree today | Disposition |
|---|---|---|---|
| **1 — property-based** | analysis over dataflow/algebraic properties; `_ => None` fallthrough | serial unroll (landed 17e736b5), lane-reduction, `detect_work_cols`, `detect_batch_shape`, modulo partition, transition-graph proofs, gpu_schedule (epilogue fusion `fusion_applies`, **chain fusion `detect_chain_fusion` Phase 4b**, reuse, array last-use), purity/partition | stays; the program GROWS this tier |
| **2 — recognized vocabulary** | structural matcher on a hand-expansion + dedicated lowering | `detect_row_softmax` (+3 pass matchers + cooperative PTX + SPIR-V Softmax), `detect_reduction` (dot/gemv + cooperative both lanes), `GemmPlan::match_stmts` (+ naive PTX + tiled/coopmat family), **the fused-attention family** (`fused_attention_ptx`, `_mma_ptx`, `_mma_staged_ptx`, `_mma_kv_staged_ptx`, `build_fused_attention_kernel` — ~1400 lines in mod.rs, config-gated dormant) | retires into **declared stdlib composites** (§4); the matcher becomes a verifier/registry entry, then history |

Bootstrap intrinsics (`Max#`, `Exp#`, `Sqrt#`, …) are the sanctioned
exception and stay language-level. None of softmax/dot/matmul exist in
lib/std today — they live only as compiler matchers; §4 fills that gap.

## 3. Milestones

### M1 — warp-sliced serial reductions (general.rs)

A serial foreach-reduce loop in a work-item body (`acc = acc + f[..j..]`)
with trip count N ≥ 512 and N % 4 == 0 lowers to: whole-block work item
(P1 dispatch, `block_per_workitem`, block_threads 128), warp w owns
j ∈ [w·N/4, (w+1)·N/4), per-warp serial accumulation, smem partials
(4 floats), one bar.sync, merged sum, one store. Reuses the
`SerialUnrollPlan` matcher (same body conditions: single accumulator,
loads linear in the item); takes priority over the unroll pass (parallelism
beats MLP; unroll remains for short/singular loops).

*Gate: pv 98 → ≤55 µs, chain ≤ ~155 µs, a_err < 1e-3 both lanes.*

**STATUS (2026-09-19, first pass — measured, disabled):** the naive form
(warp-uniform slices: every lane executes the same j redundantly, loads
are same-address broadcasts) is CORRECT (a_err 7.4e-06) but 4.4× SLOWER
(pv 932 µs vs 212). Redundant-lane compute + 8× sector waste swamps the
parallelism gain. Knob `ptx_warp_slice` ships **disabled**.

**The correct form (next pass):** compose warp-slice(outer j) ×
lane-map(inner d) — lanes own the element dim (coalesced strips, per-lane
partials), warps own j slices, merges are generic operators (+ for sums,
max for max-reductions). For the ONLINE-softmax body (acc·cf + p·v) the
cf-coupling is not + -mergeable: the sliceable form is the deferred/
2-pass body (m pass + acc/l passes), which the deferred normalizer (M2)
and chain fusion (M3) produce from the natural source. The LSE one-pass
form returns as M4's DECLARED merge operator (softmax composite), not as
derived algebra. Forensics: the first build also shipped a missing
back-branch + merge-after-tail-label pair (branch skipped the merge
entirely — dead STS/LDS/BAR in SASS) — fixed; check emitted SASS, not
just PTX text, when a lowering touches control flow.

### M2 — deferred normalizer (algebraic pass)

`out[c] = num_c / den` where `den` is a scalar accumulated in a completed
loop and the consumer is linear in `out` → move the division past the
consumer's accumulation (consumer accumulates unnormalized terms; one
division after). Property-based: linearity of the consumer + single-writer
`den`. Its value is enabling M3 (softmax's normalize must not block the
pv fusion).

*Gate: correctness via M3; standalone test on a mean-then-weighted-sum
fixture.*

### M3 — producer-consumer chain fusion (gpu_schedule Phase 4b generalization)

`detect_chain_fusion` already finds producer→middle→consumer chains with
single-reader proofs (built for GEMM→elementwise→GEMM). Generalize: the
middle need not be an epilogue-scale producer — the softmax node (a
recognized vocabulary form) fuses with its dot producer (qk) and its
linear consumer (pv) into ONE dispatch: sc_j computed in-loop, p_j weighted
into the consumer accumulators, normalization deferred (M2), j-sliced (M1),
LSE-merged. The fused-attention arms are the existence proof; this derives
their structure from the dataflow. Counter protocol (t/r/u) handled as the
fused arms do.

*Gate: the three-node attention chain emits ONE kernel ≤ ~120 µs f32;
launches 3 → 1; a_err < 1e-3 both lanes; m3 harness PASS.*

### M4 — numeric.bv declarations + online softmax lowering

`lib/std/numeric.bv`: softmax (first), dot, matmul declared as composites —
canonical bodies + a registered lowering channel (the layout-contracts
"composition/stdlib owns kernel+layout together" model). The compiler
lowers DECLARED CALLS, not hand-expansions: the cooperative lowering and
the online/fused (m,l)-state lowering become two registered lowerings of
softmax. The declaration also declares the state merge operator (LSE) —
that is what M1's slicing consumes for fused softmax. `detect_row_softmax`
retires when the declaration covers its users; `detect_reduction` and
`GemmPlan` follow in later passes.

*Gate: m3 templates via stdlib softmax; fused path ≤ M3's time; the
hand-PTX gate kernel (123/94 µs) within noise of the emerged path.*

## 4. Retirement ledger (ordered, each behind a perf A/B)

1. **Fused-attention family** (mod.rs ~1400 lines) — superseded by M3+M4;
   remove when the general path ≤ their best config.
2. **`detect_row_softmax` + cooperative softmax** — superseded by M4's
   declared composite (matcher retires; lowering migrates behind the
   registry).
3. **`detect_reduction` (Dot)** — declaration candidate (dot); retire the
   matcher when dot is declared.
4. **`GemmPlan`** — the BLAS declaration case (layout-contracts §BLAS);
   largest effort, last; the matcher's affine-index work migrates into the
   declared composite's verifier.

## 5. Verification discipline (per milestone)

Baseline table before / A/B after (`compare_baseline`), a_err < 1e-3 both
lanes, `cargo test --lib` green, Praetor clean on changed files, commit per
milestone. New lowering = new match arm with `_ => None` fallthrough
intact. fp reassociation accepted under the a_err gate (precedent:
butterflies, unroll).

## 6. Risks

- **M3 counter protocol** — the fused dispatch must compose t/r/u;
  mitigated by mining the dormant fused-attention arms (existence proof)
  and removing them on success.
- **M1 × unroll interaction** — slicing takes priority; composition measured
  later, not promised.
- **fp semantics drift** — deferred normalization stores unnormalized fp32
  products (typically more accurate); gated by a_err.
- **Declaration surface design (M4)** — keep the registry minimal: name +
  lowering id + declared merge operator; do not build a framework.

## 7. Cleanup riding along

- `FlashDecodeInfo` struct edit in accel.rs (uncommitted, superseded
  before commit) — deliberately reverted.
- `2026-09-19-flashdecode-emitter.md` — supersession banner, never
  retro-edited beyond the banner.
- Gate artifacts stay as the oracle (correctness + perf reference for
  every milestone's A/B).

## 8. Supersession

`2026-09-19-flashdecode-emitter.md` (same day): its §3-§5 (FlashDecodeInfo
detector, template emitter, ggml-layout baking) are withdrawn per the
review decision; its runtime-layout findings remain valid and are absorbed
here (disjoint output slots; v proj = host offset; the n_dirty fix,
landed 022075ea).
