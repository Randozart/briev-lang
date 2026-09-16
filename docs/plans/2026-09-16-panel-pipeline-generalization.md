# Panel-pipeline generalization — the staged-pipeline lesson as a reusable capability

**2026-09-16.** The goal: reach CUDA parity (or beyond) through **compiler
engineering**, not per-benchmark tuning. This plan generalizes the single
biggest performance lesson — the smem-staged tensor-core pipeline (our
35.5 TF @4096³ GEMM, 84.5% of cuBLAS) — into a reusable codegen module,
applies it to the fused attention kernel, drives the strategy choice from
analysis, and validates the pattern across several `.abv` kernels.

## Purpose and context

The GPU effort has produced two kinds of results, and they are NOT the
same thing:

1. **Compiler engineering** (general, analysis-driven, reusable):
   - `gpu_schedule` — node DAG, topo ordering, dead-intermediate fusion,
     buffer reuse (greedy interval allocation), frontend-driven dispatch.
   - The PTX emitter (shape-parameterized kernels).
   - Measured win: the fused attention kernel beats the 2-kernel
     composition it replaces (+3%), detected by a *general* chain-fusion
     proof (any GEMM→elementwise→GEMM, operands derived at codegen).
2. **Fine-tuning / pattern-matching** (specific, per-benchmark):
   - The E-series config search (`mw/nw/warp_mh/stages/subgroups/
     fill_prefetch/bsmem_pad`) — hand-A/B'd for 4096³. cuBLAS does the
     same (its own algo search); it is not a compiler transformation.
   - The fused-kernel emitter is a hand-built *template* (mma + S-on-chip
     + scale) — the detection is general, the codegen is pattern-shaped.

**The honest gap:** at the fused kernel's supported shapes, cuBLAS is
~10× faster. The fused kernel's load path (direct per-thread fragment
loads from global) is exactly what the tensor GEMM's *staged* pipeline
fixes. An initial staged attempt (full-width Q staging) was 3.9× slower
because it crushed occupancy and serialized with per-step barriers — the
wrong staging, not the wrong idea.

**The point of this plan:** turn the staged pipeline from a tuned kernel
into a reusable compiler capability, with the strategy chosen by
analysis. That is the test of "are the wins from compiler engineering?".

## The competitor's mechanism (decompiled, 2026-09-16)

Captured the JIT'd cuBLAS HGEMM (cuda-gdb, sm_86) — a CUTLASS kernel:
`cutlass_80_wmma_tensorop_f16_s161616gemm_f16_32x32_128x2_nn_align8`
(CTA tile 32×32, K-tile 128, 2-stage pipeline, 16-byte alignment).
Full 1600-instruction SASS analysis shows the mechanism:

- **2-stage ring** — `ULOP3.LUT UR25, UR25, 0x1` (stage counter XOR 1).
- **Predicated in-flight fills** — 4× `LDG.E.LTC128B.128` (@P0–P3), one
  per pipeline slot, INTERLEAVED with the mma in one stream (the fills
  for stage k+1 overlap the mma of stage k).
- **ldmatrix fragments** — `LDSM.16.MT88.4` (transposed B) + `M88.4` (A).
- **Small tile / high occupancy** — 32×32 CTA, 128 threads (4 warps).
- **16-byte coalesced fills** with L2-sector caching.

Our tensor GEMM already ships ALL of it (the E5a stage ring, cp.async
fills, ldmatrix — the 35.5 TF path), but coupled to one kernel's layout
(`gr/mhr/bsmem_buf` in `src/backend/ptx/tensor.rs`).

## The generalization

### Step 1 — Reusable panel-pipeline module (`src/backend/ptx/pipeline.rs`)

Parameterized primitives, **behavior-preserving** (the tensor GEMM's
cubin stays byte-identical):

- `emit_panel_fill` — coalesced 16-byte global→smem fill (rows×cols×f16,
  alignment).
- `emit_ldmatrix_fragments` — the A/B fragment loads (trans/non-trans,
  the swizzle).
- `emit_stage_ring` — the multi-stage ring (stage modulo, fill-stage
  wrap, `BAR.SYNC.DEFER_BLOCKING`).
- `emit_mma_cluster` — the mma body (register blocking as a parameter).

The tensor GEMM's E5a functions migrate onto these. ONE mechanism
serves every kernel shape — the compiler-engineering move.

### Step 2 — The fused kernel on the shared pipeline

Rebuild the fused kernel's load path with the module, using the
CUTLASS-verified profile: a **small 2-stage ring** (NOT the full-width
staging that was 3.9× slower). Q/Kt/V flow as small panels; the S' tile
stays on-chip. Target: the fused kernel's load-path gap closes — it
beats the composition by more than +3% and approaches cuBLAS.

### Step 3 — Analysis-driven strategy selection

The frontend (`AnalysisResults`) picks the codegen strategy from shape
evidence: **direct vs staged vs pipelined**, the stage count, the tile,
the register blocking — derived from smem budget / occupancy / K-depth.
No hand-tuned config. The "maximum efficient default" made real.

### Step 4 — Cross-kernel validation (the "does the pattern hold" test)

Compile and benchmark a few OTHER `.abv` kernels through the same
pipeline machinery:
- a plain f16 GEMM at several shapes (128³ / 512³ / 2048³ / 4096³),
- the fused attention (the chain-fusion case),
- a GEMM + elementwise + GEMM with a DIFFERENT middle (bias-add) to prove
  the chain detection + pipeline are general, not attention-bound,
- a rectangular GEMM (M≠N≠K) for the tiling generality.

Each: correct (maxrel ≤ 1e-2) and the pipeline chosen by analysis, not a
knob. The ledger records Briev vs cuBLAS per shape.

## Verification

1. Tensor GEMM: byte-identical cubin at 35.5 TF (the refactor is a no-op).
2. Fused kernel (pipelined): beats the composition; gap to cuBLAS narrowed.
3. The other `.abv` kernels: correct + pipeline-selected-by-analysis.
4. `cargo test --lib` green; Praetor on changed files; the on-device
   harnesses (the existing C checks).

## Honest framing

This is the step that turns the biggest win (the staged pipeline) from a
*tuned kernel* into a *reusable compiler capability*. The decompile gave
the exact profile (2-stage, small tile, in-flight fills) to generalize.
Parity-or-beyond is the yardstick; the analysis-driven selection is what
makes it a compiler property, not a per-benchmark hack.

## Docs

- This plan.
- Update `docs/plans/2026-09-15-gpu-schedule-phase4b-fusion.md` on the
  staged-rung findings.
- `docs/architecture/agent-reference.md` §6 if a new standing principle
  emerges (e.g. "the staged pipeline is the general load path").
- BUGS.md for defects found.