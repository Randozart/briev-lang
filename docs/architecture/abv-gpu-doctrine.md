# The .abv GPU Doctrine — One Program, Peak Everywhere, Briev-Owned Codegen

**2026-09-04.** Codifies the strategic direction locked by the user
("for Briev, the answer is always a"): a single `.abv` program runs at
peak performance on every probed device, and every byte of generated
machine code is the compiler's own. This document is the durable
statement; `docs/plans/2026-09-04-beyond-coopmat.md` is the staged
campaign that realizes it.

Read together with: `gpu-model.md` (borrowing-not-barriers thesis),
`backend-contracts.md` (analysis-once + capability matrix),
`benchmark-strategy.md` (anti-overfit doctrine, VERDICT discipline),
`spec/SPEC.md` §9.8 (kernel synthesis tiers).

---

## 1. The doctrine

1. **One language, one program.** The author writes intent: a counted
   loop over a proven-disjoint counter (`accel` + `[i < N][i == N]`,
   SPEC §9.7). No GPU vendor appears in the source. No strategy
   keywords, no per-device pragmas, no tile sizes. The program is
   portable by construction.

2. **Peak is the compiler's job.** The compiler carries ALL machine
   knowledge: tile geometry, pipeline depth, fragment allocation,
   barrier placement, dispatch shape. It derives what CUDA programmers
   hand-write — per device probe, per shape bucket, per program
   (`M`, `N`, `K` are compile-time constants in `.abv`; a library
   cannot specialize per program — we do).

3. **Briev-owned codegen only.** The compiler's answer is never
   "call cuBLAS" and never "run nvcc". FFI lanes (`frgn`, `#Link<>`)
   exist for users; they are not the codegen path. Every tier of GPU
   code — portable or vendor-specialized — is emitted by Briev from
   Briev's own frontend decisions. (GGML's kernels are legitimate
   *references* to validate against; never a dependency.)

4. **Specialize per device, from one frontend.** The frontend analysis
   (`GemmPlan`, tier eligibility, LoopShape, dispatch predicates)
   computes ONCE. Every backend consumes the same decisions
   (backend-contracts.md §2). Vendor backends are *projections* of one
   plan into one ISA's primitives — not independent codepaths that can
   disagree. Device probing at runtime selects the tier; the source
   program never changes.

5. **Measure before build; VERDICT the losers.** Every mechanism is an
   A/B on device (the in-process harness discipline, ledger
   2026-09-04b) before it becomes a default. A losing optimization is
   a VERDICT entry, never a silent revert, never a heuristic that
   "usually helps".

## 2. Why this doctrine exists — the ceiling physics

The f16-vs-f16 race target (ledger 2026-09-02): ggml-cuda at
**42.0 TFLOP/s** on the anchor shape (4096³), via hand-written PTX
(`mma.sync` double-pumped, `ldmatrix`, `cp.async`).

**Stage-0 verdict (2026-09-04, ledger row 2026-09-04c — MEASURED, not
hypothesized):** the coopmat ceiling microkernel
(`emit_mma_ceiling_kernels` + `mma_ceiling_bench.c`) shows the
portable SPIR-V `VK_KHR_cooperative_matrix` path reaches **the
hardware tensor peak**: the mma work fits entirely inside an
L2-load-bound window, bounding the coopmat mma rate at ≥107 TFLOP/s
≈ 100% of the F16-accumulate dense peak (102 TF on GA106 = 28 SM ×
4 TC × 512 FLOP/clk × 1.78 GHz). Two ledger corrections landed with
it: the previously quoted "50.6 TF FP16-acc peak" is the **FP32-acc**
rate (the F16-acc rate is 2×); and the production GEMM's 16.5 TFLOP/s
is **pipeline-bound** (smem fills + barriers + load ratio eat ~3× vs
the mma), NOT capped by the vendor's SPIR-V lowering.

Consequence (rev 2026-09-04): the portable tier is the race road.
The original doctrine inferred vendor-lowering caps from an 11%
f16acc-vs-f32acc delta; Stage 0's measurement refuted that — the
gap is our pipeline, not the ISA path. **Stage 1 (pipeline
extraction: deeper buffers, register-prefetch, f16x2 fills,
occupancy) is the main road to the 42 TFLOP/s anchor; the
vendor-specialized PTX tier is demoted to optional** — it exists as
the escape hatch IF a future driver era regresses the coopmat path
or a workload needs `ldmatrix`/`cp.async`-class scheduling the
lowering cannot express. The standing measurement remains the
**coopmat ceiling microkernel**: re-run per driver era; the number
gates whether the specialized tiers re-arm.

Consequence (rev 2026-09-11 night, sustained re-baseline — ledger
2026-09-08 plan, "THE WALL BROKEN"): the primary/escape framing is
DEAD. At sustained 110W with shape-matched blobs, the two tiers
**split the shape space**: the PTX f16acc tier wins 4096³ (29.3 vs
~9.5) and 8192³ (30.2 vs 21.2); the coopmat f16acc tier leads at
2048³ (27.7 vs 25.5, margin shrunk). Both are Briev-owned codegen
from one frontend plan, so per-shape tier routing is a frontend
dispatch decision — the doctrine's own "specialize per program"
clause. The tiers are **complementary projections**; selection is by
measured shape bucket (recorded in the ledger, carried by the frontend
GemmPlan), never by source annotation. The coopmat ceiling microkernel
remains the per-driver-era standing measurement.

## 3. The backend tier architecture

| Tier | Backend | Scope | Status |
|------|---------|-------|--------|
| Portable | SPIR-V (Vulkan compute; OpenCL driver present) | All coopmat/row/flat tiers, all vendors | default, fully committed; **wins small/L2-resident shapes** (2048³-class, 27.7 TF; 2026-09-11 sustained re-baseline) |
| Specialized | **PTX** (Briev-emitted, driver-JIT via `cuModuleLoadData`; cubin shipping via offline ptxas) | NVIDIA tensor-class workloads | **committed tier, complementary** — wins the mid and large shape bands (4096³: 29.3 TF, 8192³: 30.2 TF — 70% of cuBLAS anchor); tier choice per shape is a frontend dispatch decision from the ledger buckets |
| Specialized | AMD / Intel native (ROCm-shaped / Level Zero+SPIRV-direct) | future — same pattern, one vendor at a time | future |

Rules that hold across all tiers:

- **Same plans in.** A specialized tier consumes `GemmPlan` + tier
  eligibility + dispatch predicates. It never re-derives strategy from
  syntax. Kernel emission and host dispatch derive from ONE predicate
  (SPEC §9.8, dispatch geometry contract) — per backend.
- **Same contract out.** Every tier verifies against the naive
  reference tier (the general path) on every benchmark shape. f16acc
  tiers carry their own ≤1e-2 numerics gate; the f32-acc and naive
  tiers remain the correctness reference (anti-overfit doctrine).
- **Numerics tier default (decided 2026-09-11).** The DEFAULT
  accumulation contract for tensor GEMM is **f32-accumulate** (≤5e-3
  gate): it is the industry default (cuBLAS `COMPUTE_32F`, PyTorch,
  llama.cpp/vLLM all accumulate fp32; fp16-acc ships only as an
  explicitly-gated inference tier) and it preserves the benchmark
  symmetry doctrine (same output as the C reference). The
  **f16-acc tier (≤1e-2 gate) is opt-in** via `ptx_tensor_f16acc` —
  a disclosed numeric-contract choice, not a strategy keyword: it is
  the fastest correct configuration (+39% sustained at 4096³:
  29.3 vs 19.4 TFLOP/s f32; ledger 2026-09-11 full-K fix) and
  selection within a tier stays the compiler's job (select_mw_nw).
- **Capability matrix.** A specialized tier declares its surface in
  `src/backend/capabilities.rs`; out-of-surface programs still compile
  on the portable tier — probe failure falls back, never fails the
  program (SPEC §9.8 gate discipline).
- **Probe-driven, config-disclosed.** Tier selection is runtime device
  probe + `config/targets.*` profiles — disclosed machinery, never
  hidden heuristics (golden rule 3). The author sees which tier ran
  (`brievc` diagnostics, bench fingerprints); the source never encodes
  it.

## 4. What "ultimate GPU code" means operationally

- For the **author**: `.abv` reads as the algorithm. No CUDA dialect,
  no vendor intrinsics, no tuning folklore. Contracts state the
  semantic obligation; proofs carry the disjointness (no manual
  barriers — `gpu-model.md`).
- For the **compiler**: each new device class is a new *projection* of
  existing plans, not a new research project. The expensive knowledge
  (tiering, tiling, pipelining, verification) lives in the frontend
  and is paid once.
- For the **ecosystem**: auto-tuning (the `derive --stochastic` MCMC
  machinery, campaign Stage 2 S6) sweeps the strategy space per device
  profile and caches winners — derived tile tables instead of
  hand-written ones. This is the durable "smarter than CUDA": CUDA
  expert intuition, systematized, re-derived per device and per shape,
  and specialized per program.

## 5. Non-goals

- **No runtime codegen dependency**: no shipping nvcc/ptxas as a
  *build* dependency of the compiler; driver-JIT (cuModuleLoadData)
  uses the driver's own compiler at runtime, with init-time
  diagnostics when absent.
- **No vendor lock-in via libraries**: cuBLAS/cuDNN/hipBLAS calls are
  user FFI lanes, not benchmarks of the compiler and never defaults.
- **No source-level specialization**: strategy keywords stay
  correctness/intent-only (golden rule 2). A keyword never carries a
  performance win; a beaten default is a compiler bug.

## 6. Standing obligations

- The coopmat ceiling number is re-measured per driver era (it is a
  vendor variable) and recorded in the vitriol ledger.
- Every tier lands with: harness A/B (in-process, alternating-order),
  correctness sweep over the benchmark shape portfolio, ledger VERDICT
  row, capability-surface declaration, and this document updated in the
  same commit if the tier architecture itself moved.
