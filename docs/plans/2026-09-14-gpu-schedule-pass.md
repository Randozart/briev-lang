# gpu_schedule: the contract-leverage scheduling tier

**Date:** 2026-09-14
**Companion to:** `docs/plans/2026-09-13-research-parity-and-contract-leverage.md`
(Finding 3 — the contract-leverage thesis), `docs/architecture/briev-capability-frontier.md`
(expressiveness closure), `docs/architecture/briev-execution-model.md` (the reactor).

## The thesis

cuBLAS composition structurally cannot fuse: each kernel in a C/CUDA
pipeline is a black box, and every intermediate round-trips HBM.
FlashAttention's IO-awareness wins — 10-20× memory savings, 2-4× faster
than cuBLAS composition — by proving fusion legality and intermediate
liveness that C programs hand-encode per model.

Briev's contracts + DAG prove it automatically. This pass is the
compiler that sees across kernel boundaries: the structural
"faster than C" tier. Its decisions are **proofs the compiler
discharges**, not heuristics. Per expressiveness closure, the same
decisions are expressible in Briev (a user writes the fused graph; the
pass consumes the declarations).

The per-kernel sprint is over (E1-E8: geometry, occupancy, depth,
warp-spec all exhausted; stages=3 shipped at +1%). The path to
"beat CUDA" is the graph level.

## The foundation gap (why Phase 0 is first)

The GPU runner is already the reactor: a pass loop firing nodes whose
preconditions hold, equilibrium = idle (`src/backend/spirv/runner.rs:535-580`).
But two structural gaps block every graph-level optimization:

1. **Cross-kernel ARRAY dataflow is broken.** Each kernel owns a
   separate device buffer seeded once from host; subsequent launches
   upload **scalars only** (`lib/runtime/briev_accel_rt.c:438-499`).
   Producer kernel A's array writes live only in A's buffer; consumer B
   reads stale seeded data. Two chained kernels cannot exchange an
   array today.
2. **Node ordering is author-written `phase` scalars.** The compiler's
   `CausalGraph`/`DependencyGraph`/`transition_graph` are consumed by
   the LLVM backend only; the GPU runner sees no dependency knowledge.
   Every launch is followed by a full sync (`briev_dev_cuda.c:372`).

Fix #1 (Phase 0) is the natural one: **one shared device buffer = the
full state projection** — the LLVM `%State` model. The projection
layout is already name-sorted and shared across kernels
(`runner.rs:91-104, 471-489`); the baked `a_off/b_off/y_off` immediates
already index into it. Producer→consumer array flow becomes automatic
D2D within one allocation.

## Phases

| Phase | What | Contract proof | Gate |
|---|---|---|---|
| **0** | Shared device state: one allocation = the full projection; every launch targets it; arrays persist; scalars upload dirty (unchanged) | residency is per-program, not per-kernel (`analyze_resident_safety`) | a 2-kernel ARRAY chain (GEMM → elementwise) correct on-device |
| **1** | Node DAG: per-node read/write buffer sets (`accel.read_buffers/write_buffers` + `transition_graph.write_set` + `CausalGraph.reads`) → topo-ordered runner dispatch; edge A→B iff `writes(A) ∩ reads(B) ≠ ∅` or WAW | edge existence = the causal overlap; edge strength = `entails(post(A), pre(B))` | a 2-independent-node graph runs without author phase scalars |
| **2** | Sync elimination: sync only at data/WAW edges; independent nodes launch back-to-back (the existing `launch_resident_batch` dead code is now called) | independence = no edge ∧ `region` independence ∧ `concurrency_gate::xor_overlap` | independent kernels batch; measured launch-overhead drop on the attention-decode bench |
| **3** | Buffer reuse: extend `global_lifetime.free_after` to arrays (R/W split); a dead buffer's slot reassigned to a later producer | last-consumer liveness | a graph with a dead intermediate reuses its slot (state size shrinks) |
| **4** | Fusion: epilogue fusion first (elementwise consumer whose only edge is the GEMM y, intermediate proven dead → one kernel); then FlashAttention-class multi-GEMM (QKᵀ → softmax → ×V as one fused kernel) | dead-intermediate proof: written by A, read only by B, B consumes it | fused attention-decode correct + faster than the 3-kernel composition |

## The standing benchmark: attention decode (3-node chain)

The recurring correctness + performance gate:

```
node qk : GEMM  (Q@Kᵀ → S, M×N)          [S = q·kᵀ]
node sm  : row-op (softmax(S) → P, M×N)   [P = softmax rows of S]
node pv  : GEMM  (P@V → O, M×N)           [O = p·v]
```

- Phase 0: qk → sm → pv runs correct end-to-end on-device (array
  flow through S and P).
- Phase 2: the three launches sync only where needed; measured
  launch-overhead drop.
- Phase 3: S's slot reused by O (S dead after sm).
- Phase 4: sm's epilogue fuses into qk; then pv fuses → one kernel.

Shapes for the gate: 512×512×512 decode (attention head), then
1024/2048 seqlen. Compare vs the same graph through the llama.cpp
cuBLAS path (composition).

## The contract surface the pass consumes

All decisions are frontend-computed and land in `AnalysisResults`
(frontend-driven dispatch rule; the dead `fusable_pairs` field pattern
is fixed here — populated, not re-derived):

- **Independence**: `CausalGraph` proven edges (`entails`,
  `causality.rs:288`) + `region` independent regions
  (`src/analysis/region.rs:98`) + `concurrency_gate::xor_overlap`.
- **Read/write sets**: `accel.read_buffers/write_buffers/scalar_ins`
  (buffer-name level) + `transition_graph.write_set`
  (guarded-write-safe, `:1426`) + `CausalGraph.reads`. Missing: a
  per-node read set on `ReactorNode`, guarded-write capture in
  `collect_assigned_identifiers` — added in Phase 1.
- **Last-use**: `global_lifetime.free_after` extended to arrays with an
  R/W split (currently heap-only, no read-vs-write distinction).
- **Fusion legality**: dead-intermediate proof (buffer written by A,
  read only by B, B's post consumes it) — new in Phase 4.

## Process rules

- House protocol per phase: `cargo test --lib` green; on-device
  correctness gate; measured A/B vs the composition baseline; commit per
  verdict (Rule 20 negatives stand as ledger entries).
- No heuristics: every decision has a proof (structural or Z3).
- The generated runner stays the reactor (equilibrium semantics
  unchanged); only the ORDER, SYNC, LAYOUT, and FUSION are
  compiler-driven.
- Config knobs per feature, default off until its gate passes
  (`ptx_gpu_schedule` umbrella + per-feature flags).

## Undo

Phase 0: the runner can emit the historical per-kernel buffers behind a
knob. Phases 1-4 are additive passes into AnalysisResults; the runner
falls back to declaration-order + per-launch sync when the schedule is
absent. The f32/CPU paths are byte-identical throughout.