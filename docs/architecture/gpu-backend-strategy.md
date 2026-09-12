# GPU Backend Strategy — Full Optimization Landscape

**2026-09-08.** Consolidates the complete design space for the GPU backend —
the engineering analyses, the current committed work (Stage 2 PTX tier), the
Briev-specific levers that could *beat* optimized CUDA, the multi-vendor
target matrix, and the forward roadmap. Read together with:
`gpu-model.md` (borrowing-not-barriers thesis), `abv-gpu-doctrine.md` (one
program, peak everywhere, Briev-owned codegen), `gpu-offloading.md`,
`backend-contracts.md` (analysis-once + capability matrix), and the campaign
`docs/plans/2026-09-04-beyond-coopmat.md` + execution
`docs/plans/2026-09-08-ptx-tier-execution.md`.

This document is deliberately complete — every possibility discussed, with
an evaluation. Nothing here is a commitment by itself; the roadmap section
marks what is being built now vs. what is a future option.

---

## 1. The Roofline Reality

Optimizing for a GPU is fundamentally different from a CPU. A CPU compiler
wins by scalar/algorithmic transforms (bounds-check elimination,
vectorization, devirtualization, register promotion). A GPU "optimized CUDA"
kernel (CUTLASS, hand-tuned Triton, ggml-cuda) operates **at the physical
hardware ceiling** — 90–95%+ of the Roofline model's DRAM bandwidth or
compute FLOP/s.

> You cannot beat the speed of light of the silicon by compiling the same
> math better. You can only beat CUDA by changing the math, eliminating
> memory roundtrips, or scheduling instructions better than nvcc/ptxas.

This frames every strategy below: the wins come from **what Briev knows that
CUDA's C++ frontend does not** (invariants, reactive topology, contract
bounds), and from **emitting at the right hardware layer** (not the SPIR-V
driver path on NVIDIA).

The current ledger ground-truth (from `2026-08-31-vitriol-gemm-comparison.md`
+ 2026-09-11 sustained re-baseline):

| cell | number |
|------|--------|
| ggml-cuda F16 4096³ (RTX 3060, locked) | **3.271 ms = 42.0 TFLOP/s** — 83% of the 50.6 TF FP32-acc dense peak; double-pumped mma |
| True F16-acc dense peak, GA106 | ~102 TF (28 SM × 4 TC × 512 FLOP/clk × 1.78 GHz) |
| **Briev PTX f16acc @4096³** (full-K store-only) | **29.3 TF** (4.44e-3) — 70% of the cuBLAS anchor; beats coopmat |
| Briev PTX f16acc @8192³ | **30.2 TF** (8.22e-3) — beats coopmat 21.2 |
| Briev PTX f16acc @2048³ | 25.5 TF (1.30e-3) — coopmat leads at 27.7 |
| Briev SPIR-V coopmat mma ceiling (Stage 0) | ≥107 TFLOP/s mma rate — L2-load-bound, pipeline-bound NOT vendor-capped |
| Portable tier structural limit (Stage 1) | 4.55 ms / 30.2 TFLOP/s — DRAM-fill + pipeline bound |

The lesson: the portable SPIR-V path **reaches hardware tensor peak**; the
production GEMM is *pipeline-bound* (fills/barriers/load-ratio eat ~3× the
mma rate). The PTX tier (Stage 2) exists to express `cp.async`/`ldmatrix`-class
scheduling the SPIR-V lowering cannot — and it now reaches **70% of the
cuBLAS anchor** at 4096³, with the remaining gap traceable to the register
allocation / occupancy wall (64 regs = 2 CTAs/SM vs cuBLAS's 3+).

---

## 2. Current State (what is already built and validated)

The engineering analysis's headline recommendation — *"emit PTX directly,
bypassing the SPIR-V driver layer on NVIDIA, for tensor-core + async-copy
access"* — is the Stage 2 tier already under construction:

| Step | Status | What it is (in the analysis's terms) |
|------|--------|--------------------------------------|
| **S1** | DONE | `lib/runtime/briev_dev_cuda.c` — CUDA driver via `dlopen("libcuda.so.1")`, `cuModuleLoadData` runtime JIT (**the analysis's "Option B"**: PTX text → driver ptxas → SASS, no nvcc needed). Probe-first in the driver chain (cuda → vulkan → opencl). |
| **S2a** | DONE | `src/backend/ptx/mod.rs` — `--backend ptx` emits naive GEMM PTX text; correct (max_rel_err 0.000e+00 @4096³ via `gemm_bench`). |
| **S3a** | DONE | `mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32` fragment layout locked — **exact, rel 0** on RTX 3060 (both JIT and cubin paths). |

So the analysis's core architectural moves (direct PTX, `mma.sync`, driver JIT)
are validated and largely landed. The sections below cover what is NOT yet
built: the async memory pipeline, the Briev-specific fusion lever, the
emitter-route tradeoff, occupancy tuning, and the multi-vendor matrix.

---

## 3. The Optimization Landscape — "all the best methods" evaluated

### 3.1 Asynchronous memory pipelines (`cp.async`, multi-stage)

The single biggest named lever in the beyond-coopmat campfire. The portable
tier's floor is DRAM B re-read: B = 32 MB re-read by 64 M-tiles = 2 GB at
360 GB/s ≈ 5.5 ms. **SPIR-V has no async global→workgroup copy**; the D2
register prefetch hides latency but cannot raise bandwidth utilization
beyond what the fused fill already achieves.

`cp.async.ca.shared.global` (Ampere+) copies global→shared directly, bypassing
registers, and a **multi-stage (double/triple-buffer) pipeline** overlaps
global→smem transfer of tile k+1 with the mma compute of tile k.

- **Evaluation: essential for the 42.0 TFLOP/s anchor.** This is the
  mechanism that closes the portable tier's 30.2→42 TF gap. Requires the
  S3b tensor GEMM kernel and `.shared` smem staging.
- **Cost:** manual smem lifecycle + `bar.sync`/`cp.async.wait_group`
  bookkeeping. Briev's reactive stage model can drive this (see §5.4).

### 3.2 `ldmatrix` fragment loads

`ldmatrix.sync.aligned.m8n8.x4.shared.b16` loads 4 8×8 f16 matrices from
shared memory into 4 `.b32` registers per lane in one instruction — the
efficient path to build mma A/B fragments from smem tiles.

- **Evaluation: essential for tensor GEMM throughput** — replaces the
  hand-rolled per-lane `ld.shared` + pack sequence (which the S3a microtest
  proved correct but is 4-8× more instructions). The `ldmatrix` ×4 form maps
  directly onto the `m16n8k16` A fragment (4 regs) and B fragment (2 regs,
  via the ×2 form or two of the 4).
- **Cost:** addressing is per-16-byte-rows (each thread supplies a 16-byte
  aligned row address); needs the correct smem swizzle to avoid conflicts.

### 3.3 Warp primitives: `shfl.sync`, `mbarrier`, `bar.sync`

- `shfl.sync` (register-to-register exchange within a warp, no smem) — used
  in epilogues (row/col reductions) and some fragment shuffles.
- `mbarrier` + `cp.async.mbarrier.arrive` — fine-grained producer/consumer
  barriers for the async pipeline (TMA on Hopper uses these).
- `bar.sync 0` — block-wide barrier for smem staging.

- **Evaluation:** `shfl.sync` optional (mma epilogues here are element-wise
  stores, no reduction). `mbarrier` relevant only when the pipeline needs
  per-stage arrival tokens beyond `bar.sync`. Start with `bar.sync` + the
  `cp.async` wait group; add `mbarrier` only if the single-block pipeline
  becomes the bottleneck.

### 3.4 Vectorized 128-bit loads (`float4`, `ld.global.v4.f32`)

Coalescing: a warp of 32 threads reading `A + i*4` is one 128-byte
transaction. Vectorizing to `ld.global.v4` (16 bytes/thread) cuts instruction
count and raises bandwidth utilization.

- **Evaluation:** already implied by the SPIR-V tier's vec4 handling
  (`vec4_fields`, 16B-aligned arrays). The PTX tier should emit `.v4.f32`
  loads for the smem fills when the contract guarantees 16B alignment +
  count%4==0 (see §5.2 — the invariant gives this for free). Low risk, high
  value.

### 3.5 Bank-conflict-free shared-memory layouts

32 shared-memory banks; a warp hitting different addresses on the same bank
serializes (up to 32×). Hand-tuned CUDA uses padding (`[TILE_M][TILE_K+1]`)
or XOR-swizzle of the lower index bits.

- **Evaluation: important once the tensor kernel is correct.** The mma A/B
  smem tiles (f16) are read by `ldmatrix` in a fixed lane pattern; the smem
  layout must be swizzled so that pattern is conflict-free. Briev's affine
  access analysis can synthesize the padding/swizzle (see §5.3) rather than
  hardcode one magic layout.

### 3.6 Occupancy: register caps and launch bounds

On GPUs the register file is shared per SM. If a thread uses 33 registers
instead of 32, max warp occupancy can halve, and the scheduler cannot hide
memory latency → a performance cliff. The analysis's concrete point: tune a
register ceiling (`.maxnreg` / `__launch_bounds__`) to protect occupancy.

- **Evaluation: add a config knob now (decided).** Emit `.maxnreg` into the
  PTX driven by `config/targets.toml`; measure occupancy against the CTA
  tile (128 threads) in S3b. Foundational for S5.

### 3.7 Tensor-core instruction breadth (future hardware)

- `mma.sync.m16n8k16` — Ampere (current, S3a locked).
- `mma.sync.m16n8k8` / fp8 / bf16 variants — data-type breadth.
- `wgmma` — Hopper (sm_90): warpgroup-level mma with big-tile + mbarrier
  pipeline, the highest-throughput form.
- **TMA** (`cp.async.bulk.tensor`) — Hopper/Blackwell: tensor-map-driven
  global↔smem bulk copies, offloads addressing.

- **Evaluation:** future. The current target (RTX 3060, sm_86) has neither
  `wgmma` nor TMA. Design the S3b kernel so the async-pipeline abstraction
  (stage → wait → compute) maps onto `cp.async` today and `mbarrier`+TMA on
  sm_90+ tomorrow (see §6 roadmap).

### 3.8 The SPIR-V driver tax (targeting NVIDIA)

NVIDIA's SPIR-V pipeline (Vulkan/OpenCL) does not receive the decades of
microarchitectural tuning nvcc gets, and SPIR-V abstracts away the warp/tensor/
async primitives. The analysis: leaving 30–50% on the table via SPIR-V.

- **Evaluation: confirmed by the ledger.** The portable tier *does* reach
  tensor peak (Stage 0 ceiling ≥107 TF), so the tax shows up as the missing
  async-copy/finer-acc capability, not a raw-capability ceiling. The PTX
  tier exists precisely to unlock those. **But**: do not abandon SPIR-V —
  it remains the canonical path for mobile/embedded and non-NVIDIA via Vulkan
  (see §6 multi-vendor). The doctrine's "one program, peak everywhere"
  argues for keeping SPIR-V as the portable fallback and PTX as the NVIDIA
  peak path.

---

## 4. Emitter Route Evaluation: hand-PTX vs LLVM NVPTX vs SPIR-V

The analysis raised the question of emitting via **LLVM NVPTX** (reusing the
existing LLVM backend) rather than hand-written PTX. Full tradeoff:

| Dimension | Hand-PTX (current S2) | LLVM NVPTX | SPIR-V (portable) |
|-----------|----------------------|-----------|-------------------|
| **Fragment control** | Direct — mma fragments already exact (S3a) | Via `@llvm.nvvm.wmma/mma` — LLVM owns packing, can fight exact layout | Via `VK_KHR_cooperative_matrix`, coarse acc (m16n16) |
| **cp.async / ldmatrix / TMA** | Direct text emission, full control | NVVM intrinsics (`@llvm.nvvm.cp.async.*`) exist but less mature | Not expressible (the campfire's reason to leave) |
| **Scheduling (ptxas)** | ptxas JITs our text — we write the schedule shape, ptxas does SASS | ptxas gets LLVM's IR, may re-schedule differently | Vulkan driver's own ptxas-equivalent, lower priority |
| **Briev-owned codegen (doctrine)** | YES — every byte ours | NO — LLVM IR + NVPTX is a foreign pipeline in the kernel path | YES |
| **smem layout control** | Direct `.shared` + swizzle | Via allocas + addrspace(3), indirect | Via SSBO/binding layout |
| **Toolchain dependency** | None (driver JIT only) | LLVM NVPTX target init in-process | None (Vulkan driver) |
| **Portability** | NVIDIA only | NVIDIA only (nvptx64) | Cross-vendor + mobile/embedded |

**Evaluation / recommendation:**
- **Stay with hand-PTX for the tensor GEMM family (S3b–S5).** The mma
  fragment layout is already exact and Briev-owned (doctrine §4). The
  analysis's *reason* to use NVPTX — ptxas scheduling — applies equally to
  hand-PTX text (the driver JITs our text with the same ptxas). NVPTX's
  advantage is *not* needing to write fragment/smem assembly by hand, but we
  have already written and verified it.
- **Revisit NVPTX only for non-tensor kernels** where LLVM's scalar/vector
  optimization (mem2reg, unroll, vectorization) would save Briev from
  hand-writing a general SIMT emitter. If the PTX tier grows beyond GEMM to
  general `.abv` compute (a future S7), NVPTX becomes the pragmatic choice
  for the general emitter while the GEMM family stays hand-PTX.
- **Keep SPIR-V** as the portable/mobile path (doctrine: one program, peak
  everywhere).

---

## 5. Briev-Specific "Beat CUDA" Levers

These are what the analysis's Part 2 argues Briev can do that CUDA's C++
frontend cannot. They are the *edge* — the only way to genuinely beat
hand-optimized CUDA rather than match it.

### 5.1 Cross-node kernel fusion (the biggest lever)

CUDA round-trips intermediate results through HBM/VRAM between kernels
(GEMM → bias → activation → …), throttled by the memory bus. Because Briev
represents program execution as a **state-transition topology** (`accel`
nodes over counters), the compiler can trace data dependencies **across
transactions** and fuse producer/consumer nodes into ONE kernel, keeping
intermediate state in registers/shared memory/L1, bypassing global DRAM.

- **Why it beats CUDA:** individual optimized kernels can't remove the
  inter-kernel DRAM roundtrip; a fused Briev kernel can. The analysis claims
  "the only way to achieve a 2–5× speedup over individually optimized CUDA
  kernels."
- **Status: NOT yet addressed.** `GemmPlan::match_stmts` matches a single
  GEMM shape; the accel analysis is per-node. Fusing GEMM→epilogue chains
  needs (a) a language/shape to express a fused pipeline in one `.abv`, and
  (b) an analysis pass that detects the producer/consumer chain and hands the
  PTX emitter a single fused kernel plan. This is a **frontend + analysis
  build**, tracked as the top future item (§6).
- **Caveat:** the GEMM-family-only surface gate (S2a) must be extended with
  a fused-GEMM-epilogue arm, and the correctness gate must cover the fused
  output vs. the separate-kernel reference. Do NOT attempt before S5's
  single-node anchor is won — it builds on the same emitter machinery.

### 5.2 Zero-divergence vectorization via contracts

Hand-written CUDA guards boundaries (`if (idx < N)`) → warp divergence +
scalar loads. Briev contracts can assert `[N % 128 == 0]` / `[len %
warp_size == 0]` / alignment → the compiler **erases bounds checks** and
emits 128-bit vector loads (`float4`/`v4.f32`) with unrolled static
induction steps, guaranteeing zero divergence.

- **Status: the vectorization primitive exists** in the SPIR-V tier
  (`vec4_fields`, 16B-aligned projections). The PTX tier should emit
  `.v4.f32` for smem fills from the same alignment/divisibility contract.
  Low-risk, high-value.

### 5.3 Bank-conflict-free smem synthesis

Briev's array shapes/access strides are verifiable contract invariants. A
lowering pass can analyze the affine map `tid → address`, detect a
stride-32-on-32-bank pattern, and **synthesize** padding (`[TILE_M][TILE_K+1]`)
or an XOR-swizzle of the low index bits — guaranteeing zero conflicts without
hand-hardcoding one magic layout.

- **Status: NOT yet addressed.** The smem layout in the tensor tier is
  hand-chosen. A first-class pass (per Rule 10 — no prototyping) that derives
  the swizzle from the access affine map is a candidate S3b/S5 item.

### 5.4 Async pipeline from the reactive stage model

The analysis's multi-stage async pipeline (prefetch k+1, compute k-1) maps
naturally onto Briev's reactive staging. Instead of hand-writing the
double-buffer unroll, the emitter can model the K-loop as a software pipeline
whose stage shape comes from the frontend (`GemmPlan` stages/panels — the
SPIR-V tier already computes these). The PTX emitter **reuses that analysis**
rather than re-deriving it (backend-contracts §2: analysis-once).

- **Status: the analysis (stage/panel counts) already exists** in
  `GemmPlan`/`config_tuning` (`spirv_coopmat_panels_per_stage`,
  `coopmat_stages`). The PTX emitter consumes them to shape the `cp.async`
  pipeline. This is the natural S3b structure.

---

## 6. Multi-Vendor Target Matrix

The doctrine ("one program, peak everywhere") plus the analysis's landscape
for non-NVIDIA hardware. The matrix is the *future* target surface; today
the NVIDIA path (SPIR-V portable + PTX peak) is the committed work.

| Vendor | Peak path | Mechanism | Runtime load | Notes |
|--------|-----------|-----------|--------------|-------|
| **NVIDIA** | PTX text → driver JIT (S1/S2) | `mma.sync`, `cp.async`, `ldmatrix`, `wgmma`/TMA (sm_90+) | `cuModuleLoadData` | The current peak path; portable fallback via SPIR-V |
| **AMD (RDNA/CDNA)** | LLVM AMDGPU → ELF `.hsaco` | `@llvm.amdgcn.mfma.*` matrix cores, `ds_bpermute`/`readlane`, LDS (addrspace 3) | ROCm/HSA `hsa_code_object_reader_load_from_memory` / `hipModuleLoadData` | No lossy SPIR-V middleman (ROCm converts SPIR-V back to LLVM IR). 64-thread wavefronts (RDNA 32). |
| **Intel (Arc/DataCenter/Xe)** | Native GEN ISA via `ocloc` AOT, or Level Zero | XMX `SPV_INTEL_joint_matrix`, `SPV_INTEL_subgroups` | `zeModuleCreate` | `ocloc` gives AOT `.bin` (zero JIT, deterministic regalloc). Intel embraces SPIR-V first-class; use SPIR-V + Intel extensions. |
| **Apple Silicon** | MSL (Metal Shading Language) text | `simdgroup` (32-thread warp), threadgroup memory, SIMD-shuffle | `newLibraryWithSource:` | MSL is the industry route (WebGPU/Mojo/MLX). AIR is undocumented/version-churn — avoid. SPIR-V via MoltenVK is second-class. |
| **Mobile/embedded (Adreno, Mali)** | SPIR-V via Vulkan | `VK_KHR_shader_subgroup_*` | Vulkan | SPIR-V IS canonical here — proprietary ISAs, no stable LLVM backends. Enable subgroup extensions. |

**Unified pipeline vision:** `[Briev AST/Contracts] → [Core LLVM IR or
direct emission] → vendor-native` — NVIDIA → PTX text → CUDA driver; AMD →
AMDGPU `.hsaco` → ROCm/HSA; Intel → native GEN or SPIR-V+XMX → Level Zero;
Apple → MSL → Metal; mobile → SPIR-V → Vulkan. Each vendor's doctrine row:
Briev-owned codegen, no foreign compiler in the kernel path, peak on the
probed device from ONE frontend plan (the abv-gpu-doctrine's tier
architecture: portable SPIR-V + per-vendor projections).

**Ordering note:** the multi-vendor surface is *future*. The campaign is
NVIDIA-first (S3b–S5) because that is where the 42 TFLOP/s anchor lives and
where the pipeline machinery (cp.async/ldmatrix/mma) is being built. AMD
(`mfma`/`.hsaco`) is the closest second — the same emitter concepts
(analysis-once plan, smem pipeline, fragment layout) transfer.

---

## 7. Roadmap

### Committed (this campaign — NVIDIA peak path)

- **S3b** — tensor GEMM kernel into `--backend ptx`: warp-tile geometry from
  `GemmPlan` (64×64 CTA / 128 threads default), smem staging, `ldmatrix`,
  `cp.async` multi-stage pipeline (stage counts from the existing analysis),
  `mma.sync` loop, epilogue stores. Emit `.maxnreg`/launch-bounds from a
  config knob (occupancy, decided). Reuse the S3a-locked fragment layout.
- **S4** — correctness gate: whole shape portfolio (2048³/4096³/8192³/
  skinny-K/small) vs. the naive reference tier, gates 5e-3 (f32-acc) / 1e-2
  (f16-acc).
- **S5** — performance gate: close the 29.3→42.0 TFLOP/s gap @4096³
  (currently at 70% of cuBLAS anchor). The remaining gap is the
  register-allocation / occupancy wall (64 regs = 2 CTAs/SM vs
  cuBLAS's 3+). Occupancy tuning, smem bank-conflict layout,
  `.v4.f32` fills.
- **S6** — auto-tune loop: `derive --stochastic` sweeps
  (tile × stages × warps) per device profile, winners cached in
  `config/targets.*`.

### Future options (documented, not committed)

- **Fusion pass (biggest lever)** — cross-node GEMM→epilogue fusion via the
  reactive topology (design doc first, then analysis pass + fused-kernel
  emitter arm). §5.1.
- **General non-GEMM PTX emitter** — via hand-PTX or revisit LLVM NVPTX
  (§4) once the GEMM family wins.
- **Bank-conflict smem synthesis pass** — affine-access-driven swizzle (§5.3).
- **wgmma/TMA tier** — Hopper/Blackwell, mapping the S3b pipeline onto
  `mbarrier` + TMA (§3.7).
- **AMD tier** — `mfma`/`.hsaco` via ROCm (§6).
- **Intel / Apple / mobile tiers** — per §6.

### Docs to keep current as work lands
`backend-contracts.md` (PTX charter row), `HANDOFF-2026-08-31-gpu.md`
(status block), `spec/SPEC.md` §9.8, `AGENTS.md` reference index, plans
INDEX.

---

## 8. Open Questions (for future evaluation)

1. **Fusion expressiveness** — what `.abv` shape (or frontend synthesis)
   expresses a fused GEMM→epilogue chain so `GemmPlan`-style analysis can
   match it? This is the gating question for §5.1.
2. **NVPTX general-emitter threshold** — at what kernel-surface size does
   LLVM NVPTX become cheaper than a hand-written general SIMT PTX emitter?
3. **F16-acc numerics contract** — RESOLVED (2026-09-11): the K-budget
   boundary is ≈K=12288, verified on device for both PTX and coopmat
   tiers (5.2e-3 @K=4096, 8.2e-3 @K=8192, approaching 1e-2 gate).
   The tier router must enforce K≤12288 for f16acc; larger K belongs
   on the f32-acc tier.
4. **smem swizzle generality** — does a single affine-analysis swizzle cover
   all tile shapes, or is a per-shape swizzle table (from `derive --stochastic`)
   the durable answer?
5. **Multi-vendor priority** — after NVIDIA, which vendor (AMD most likely)
   receives the next peak-tier projection?
