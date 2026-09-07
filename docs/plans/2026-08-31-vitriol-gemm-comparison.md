# VITRIOL comparison — GEMM as the .abv GPU benchmark

**Date:** 2026-08-31
**Status:** spec — milestone work starts here
**Comparison target:** VITRIOL (`~/Desktop/Projects/VITRIOL`) — LLM inference
engine on THIS box: i7-3770 (AVX, no AVX2), RTX 3060 12GB, GTX 1070 Ti 8GB,
DDR3. Chimera CUDA+Vulkan backends, certified-numbers evidence culture.

## Why VITRIOL is the right target

1. **Same machine** — its certified numbers (BENCHMARKS.md) were measured on
   the exact GPUs the .abv stack runs on.
2. **The evidence culture** — certified full-workload runs, VERDICTS.md for
   negative results, fingerprint trails. This ledger adopts those rules.
3. **LLM inference IS GEMM.** `C = A×B` at llama.cpp shapes has a directly
   comparable number on this box (llama.cpp CUDA and Vulkan paths).
4. **Portability is a real .abv differentiator.** VITRIOL's build mandates
   CUDA archs `61;86` (two compile targets). One .abv/SPIR-V binary runs on
   both GPUs unchanged.
5. **The residency convergence.** VITRIOL's VERDICTS.md: "streaming a
   fitting model is a pessimization — resident execution beats every
   streamed configuration." Our device-residency work hit the same verdict
   independently. Same box, same lesson — documented as a shared result.

## Workload ladder (milestones)

| M | workload | kernel surface needed | reference |
|---|----------|----------------------|-----------|
| M0 | elementwise + RoPE-style passes | none (works today) | llama.cpp elementwise CUDA |
| M1 | **GEMV** `y = A·x` (M×K, one dot product per item) | in-body loop (`foreach` over range) + FMA | llama.cpp GEMV |
| M2 | **GEMM** `C = A×B` (N=M=K=4096) | M1 + tiling/shared memory (later) | llama.cpp GEMM CUDA + Vulkan |
| M3 | quantized row (Q4/Q8 dequant + dot) | M2 + integer/dequant ops | llama.cpp quant kernels |
| M4 | attention-probe scoring slice | M3 | VITRIOL LULL numbers |

## The language gap M1 needs (this is the driver)

Briev has `foreach` over ranges (SPEC §11.4 — the sole iteration keyword,
lowers to a counted `0..Count` loop). The kernel surface does not accept it
yet. GEMV in .abv would read:

```abv
async node gemv [i < M][i == M] {
    let acc: Float = 0;
    foreach k in 0..K {
        acc = acc + a[i * K + k] * x[k];
    }
    y[i] = acc;
}
```

## M1 status: LANDED (2026-08-31, same day)

- **Proof**: `stmt_is_kernel` accepts bounded `foreach k in start..end` —
  loop var is work-item-private (added to locals), bounds pure, body obeys
  the existing rules (affine writes, pure reads). Non-range collections
  stay host-side.
- **Lowering**: structured loop in the SPIR-V kernel (OpLoopMerge +
  OpPhi in the header + preheader/continue condition computation).
  Subtleties that cost real debugging time, recorded for posterity:
  the guard body must branch into the preheader (unterminated-block panic);
  OpPhi must OPEN the header (before OpLoopMerge); the condition must be
  recomputed in the continue block AFTER the increment (a stale test ran
  the body once past the end and faulted the GPU).
- **Capability gate**: foreach + slices_ranges enabled for the SPIR-V table.
- **Let-with-float-initializer fix**: `let acc: Float = 0;` materializes a
  float zero (storing an i64 zero into a float variable was an OpStore
  mismatch).
- **Verified on device**: identity-matrix GEMV (64×64) returns y[i] = x[i]
  exactly through the real dispatch path — dot-product accumulation,
  indexing, and the loop all correct on the RTX.
- Example: `examples/gpu/gemv.abv` (M=K=4096) compiles, spirv-val
  vulkan1.3 PASS.

## Next (M1 → M2)

- GEMV performance run (M=K=4096): GPU vs single-thread CPU vs llama.cpp
  GEMV — the first ledger number for M1.
- GEMM M2: multiple gemv-like launches vs a tiled kernel; shared-memory
  tiling is the M2 performance item.

Honest note: llama.cpp's GEMV/GEMM are years-tuned. M1/M2 acceptance is
**correctness first, then measured trajectory** — every milestone logs its
number in this ledger (VERDICTS.md rules: losses are recorded, not hidden).

## Evidence rules (adopted from VITRIOL)

- Every number is a full-workload run (whole kernel, warm), not a
  first-launch JIT figure; JIT warm-up runs are labeled
- GPU/CPU/losses all recorded; negative results get a VERDICTS entry
- Config fingerprint: GPU name, driver, kernel blob size, dispatch shape
- Same-box comparisons only

## Ledger

| date | milestone | GPU (3060) | CPU ref | verdict |
|------|-----------|-----------|---------|---------|
| 2026-08-31 | pairs elementwise 16M | 70ms (PCIe-bound single launch) | 36ms (in-cache) | workload memory-bound; residency required — see first-gpu-dispatch results |
| 2026-08-31 | **M1 GEMV M=K=4096 baseline** (`gemv_bench`, pre-O2) | avg 17.9ms, min 15.2ms, **1.88 GFLOP/s** (resident launch, 5 warmup + 20 iters, max_rel_err 0.0) | 30.0ms single-thread, 1.12 GFLOP/s | **GPU 1.67× single-thread CPU.** Both far from roofline — the kernel is scalar, no FMA, no vectorized loads (that is the point: this is the pre-optimization baseline). Harness: `benchmarks/gpu/gemv_bench.c` (reads a .spv, self-contained; correctness gate vs double-accumulate CPU ref before timing). llama.cpp same-shape number: NOT yet on the ledger — build pending. |
| 2026-08-31 | **O2 FMA fusion** (explicit GLSL.std.450 Fma at lowering; FMul+FAdd no longer emitted for `a*b+c`) | avg 15.5ms, min 15.1ms, **2.16 GFLOP/s**; max_rel_err 0.0 | (same run CPU ref 15.9ms — CPU timing varies between runs; the GPU A/B is the comparison) | **KEEP.** avg −13% vs baseline (17.9→15.5), min ~parity (15.2→15.1) — the kernel is memory-bound, so the math-throughput win shows mostly in the average. Numerics: exact match vs the double-accumulate reference (single rounding per step). spirv-val vulkan1.3 PASS. Kernel blob 2548→2612B (ExtInstImport + Fma). |
| 2026-08-31 | **llama.cpp anchor** (`benchmarks/gpu/ggml_gemv_bench.c` — bare ggml_mul_mat F32 M=K=4096, VITRIOL's build-rebis, same box) | **ggml-cuda: avg 0.213ms, min 0.204ms, 157.8 GFLOP/s** (~300GB/s ≈ 83% of VRAM roofline) | ggml-cpu 1T 4.4ms (7.7 GFLOP/s), 8T 3.8ms (8.9) | **The race target.** We are 4.2× behind ggml-cuda (0.93 vs 0.213ms) but already 4.7× ahead of ggml-cpu. Gap analysis: their kernel runs ~32× more threads (warp-per-row) saturating bandwidth; ours runs one thread per row with a serial K-chain (128 warps total). Conclusion: the next rung is subgroup-cooperative row kernels (K-split within a workgroup + OpGroupNonUniformFAdd), NOT more load width — O3's verdict already showed loads are coalesced. |
| 2026-08-31 | **O3 float4** (vec4-typed SSBO members + shifted scalar fallback + aligned group loads in unrolled prefix AND a step-4 vector remainder loop) | avg 0.93ms (was 0.98ms scalar), ~36 GFLOP/s; exact-correctness gate OK (7e-4 f32-level) | — | **VERDICT: ~5% on GEMV — below the 20% threshold.** Root cause understood: GEMV is DRAM-bandwidth-bound and its scalar loads were ALREADY fully coalesced (consecutive work items → one transaction per warp); float4 cuts instruction count, not DRAM traffic. KEPT as infrastructure: the retype+group-load machinery is general and will pay on compute-bound GEMM (M2) and poorly-coalesced patterns. The genuine GEMV lever is split-K (parallelism), recorded as the successor rung. |
| 2026-08-31 | **O1 unroll factor 4→16** (config default) | avg ~0.99ms across 3 runs, max 1.06ms; ~34 GFLOP/s | — | **KEEP** — best-case parity with factor 4 (~0.9ms) but variance collapses (worst run 1.06ms vs 3.15ms). The per-launch outliers at factor 4 were the dominant noise source. |
| 2026-08-31 | **Device-local working set** (driver allocates VRAM SSBO; host staging only seeds/syncs scalars/downloads) + LocalSize 64→256 A/B (256 wins 3.4×: 3.0ms vs 0.9ms avg) | **best-run avg 0.9ms, min 0.876ms, ~38 GFLOP/s**; avg varies 0.9–3.0ms across runs (clock/launch wobble — recorded honestly, min is stable); max_rel_err 0.0 | 30.1ms single-thread | **KEEP — the real lever.** 17.5× over the O2 best (15.5→0.886ms). The previous "resident" mode was PCIe-bound: the SSBO lived in HOST_VISIBLE|HOST_COHERENT sysmem (~4GB/s effective; reads scaled with bytes, not threads — a 8192×2048 shape ran SLOWER than 4096² despite identical work). With VRAM residency the same kernel hits ~75GB/s effective read bandwidth (still ~5× off the 3060's roofline → O3 float4/split-K remain live levers). Pairs 4096² 2D verified end-to-end; 2011 lib tests green. Trap for the record: VkBufferMemoryBarrier.buffer must be set — a null buffer invalidates the submission and the GPU silently drops the whole command buffer.
| 2026-08-31 | **2D dispatch infrastructure** (§2b of gpu-next plan; pairs relaunched as a 4096×4096 grid) | 16,777,216 items via `briev_accel_launch_resident_2d`, fast-forward i = 16777216 correct; gemv 1D rerun within noise (avg 17.7ms, min 14.8ms) | — | **INFRASTRUCTURE, geometry-only** — no perf claim yet: the kernel still reconstructs scalar `i` and the body's shift/mask ops remain. Gains arrive with the substitution pass (row/col direct from gid.y/gid.x, gated on a guaranteed-2D launcher). Soundness bonus: the bounds guard now also covers literal counts not divisible by the workgroup size (a latent 1D hole found while wiring 2D). |
| 2026-09-02 | **Float16 GEMM end-to-end on device** (`gemm_h_bench`, 4096³, naive tier — the f16 pipeline's FIRST correct device run; the "~25% y-fill fault" is GONE) | avg 502ms, min 468ms, **273.78 GFLOP/s**; max_rel_err 2.442e-04 = the expected single f16 store-rounding (f32 compute, f16 storage) | — | **CORRECTNESS MILESTONE, not a perf row** — the naive tier is scalar-by-design; the f16 win lives in the tensor tier (spirv_coopmat knob, next). What landed to make this possible: f16-as-storage-format lowering (f32 Function storage, OpFConvert at the SSBO boundary — commit d56d910d), runtime feature PROBING (features2 + apiVersion 1.2 instance + tensor-device preference — commit 3314b834), the Float16 capability scan, and the precision-bounded literal admission. Harness: `benchmarks/gpu/gemm_h_bench.c` (f16-exact seeds, RNE encoder mirroring the backend's). Regression gates rerun with the new runtime: GEMV coop 0.199ms / rel 0.0 (ledger parity); GEMM f32 4096³ rel 0.0. Note: the earlier "256³ quirk" is RESOLVED (2026-09-02): it was a harness/blob configuration mismatch — gemm_bench derives offsets from its M·N·K args while the blob's state arrays were literal-sized (16777216); with the arrays sized to the shape, 256³ passes at rel 0.0. Harness rule: derive offsets from the generated runner's layout (the authority). |
| 2026-09-02 | **TENSOR TIER CORRECT ON DEVICE** (`gemm_h_bench`, 4096³, `spirv_coopmat=1`) — the M2.2 blocker is RESOLVED | avg 55.6ms / 2.47 TFLOP/s, **min 41.2ms = 6.85 TFLOP/s** (max 316ms — launch wobble, min is the stable signal per ledger convention) | ggml anchor 10.9ms / 12.6 TFLOP/s | **CORRECTNESS MILESTONE + first perf row.** max_rel_err 2.442e-04 — IDENTICAL to the naive tier's bound; two independent implementations agree on every sampled element. Root cause of the "fault": the coopmat GRID DECODE had tile_m/tile_n SWAPPED (commit a181951b) — only 25% of tile-rows were ever computed: exactly the "~25% y-fill" the M2.2 session misread as driver/NVVM. Second bug: the runner's v1 16×16-tile work-count formula 4× over-dispatched the 16×64-tile v2 kernel (out-of-range tiles smeared garbage over correct outputs). Doctrine in action: the naive tier (general path) served as the correctness reference that made the bisect possible. Next perf rungs (recorded, not yet run): A-panel shared reuse across the 4 warp fragments, occupancy (16k workgroups of 32 lanes underfills the 3060). Known quirk: the runtime's features2 probe reads 0 for 16bit/coop on this driver while create + kernels work — NVIDIA under-enforcement, diagnostic-only. |
| 2026-09-02 | **B-reuse rung: R=2 tile-rows per workgroup** (`spirv_coopmat_tile_rows`; B fragments load once per workgroup and feed R mma chains — B DRAM traffic ÷ R) | interleaved 3-round A/B, mins: R=1 ~41ms → **R=2 13.2ms = 10.4 TFLOP/s (stable)** → R=4 bimodal 9.9/70ms → R=8 28ms; every config rel 2.442e-04 | ggml anchor 10.9ms / 12.6 TFLOP/s | **KEEP, R=2 default.** 3.1× over R=1, 0.83× the anchor's time. R=4's 9.9ms round (13.9 TFLOP/s — past the anchor) is bimodal under co-tenant load: re-A/B on an idle box before promoting. R=16 VERDICT: rejected (slower AND miscomputed on device — suspected coopmat fragment spill past 256 acc regs/lane; capped in the shared clamp). En-route fix: `field_int(key, idx)` — idx is a FIELD INDEX, not a default; the reader passed 8 and silently pinned every build to the fallback, invalidating the first A/B sweep. |
| 2026-09-02 | **Clean-window re-A/B + sustained measurement** (GPU idle window, 3 interleaved rounds; batched mode added to gemm_h_bench) | sync mins: R=1 42.5ms stable, R=2 **13.3ms stable**, R=4 9.6ms best-mode but bimodal 66-83ms in 2/3 rounds. Batched (×20, one submit): R=2 33ms/call, R=4 18ms/call | ggml anchor 10.9ms — measured SYNC-PER-ITER (ggml_gemm_bench: 20× `ggml_graph_compute_with_ctx`), so our sync mins are the comparable numbers: R=2 = 0.82× anchor, R=4 good-mode 1.11× | **DVFS finding**: clocks pulse 1837MHz↔139MHz with GPU_IDLE throttle flags even DURING batched submission (power 8-44W of 170W cap, 44°C — not thermal) — the sustained-vs-burst gap (2.5×) is an open investigation (persistence mode / clock pinning unexplored, needs root or NVML). VERDICT unchanged: R=2 default (stable 10.4 TFLOP/s sync), R=4 = the high-variance fast mode (14 TFLOP/s when boosted). |
| 2026-09-02 | **Clock lock + R=4 promotion** (user: `nvidia-smi -pm 1 -lgc 1800,1837`) | unfed R=4 mins stable 9.56-9.78ms = **14.3 TFLOP/s, 1.14× past the anchor**; the 70ms bimodal mode eliminated (it was idle-downclocking between fence-waited launches, GPU_IDLE flags, 8-44W/170W — never thermal); R=2 unchanged 13.4ms (DRAM-bound) | anchor 10.9ms / 12.6 TFLOP/s (sync-per-iter, fed/warm — now apples-to-apples) | **DEFAULT → R=4.** The anchor race is WON on the same box: 9.6ms vs 10.9ms. Deployment note recorded: without locked clocks, bursty callers prefer R=2. Unlock reminder recorded in the plan (`nvidia-smi -rgc -pm 0`). |

## Optimization doctrine (locked, 2026-08-31 — user)

> Regular Briev is optimised for native code through LLVM. Whatever we must
> do to optimise the SPIR-V backend for GPU, we will.

The SPIR-V backend is a first-class optimization target, not a
correctness-only emitter. The ladder mirrors what LLVM does for native
code, translated to GPU reality — and follows the house rules:
frontend-driven (analysis computes, backend consumes), config-tuned
(`config/targets.dbvl` per-GPU entries like `[target.spirv64]`), measured
before built.

| # | optimization | GPU effect | prerequisite |
|---|--------------|-----------|--------------|
| O1 | constant-trip-count loop unrolling (config factor) | fewer branches, ILP | foreach lowering ✓ |
| O2 | FMA fusion (mul+add chains) | math throughput | verify driver fusion |
| O3 | vectorized loads/stores (float4 via OpTypeVector) | memory throughput | alignment proof |
| O4 | shared-memory staging for tiled reuse (GEMM tiles) | DRAM traffic cut | tiling analysis |
| O5 | occupancy shaping (LocalSize search) | latency hiding | certified shapes |
| O6 | tree reductions in shared memory | GEMV/GEMM epilogue | O4 |

Every rung: before/after numbers on the same box in this ledger. A rung
that loses is a VERDICT entry, not a silent revert.

### O1 spec (first rung, implemented now)

Unroll a foreach whose trip count is a compile-time constant (`0..K`, K
literal const) by a config factor, emitting the unrolled groups plus a
remainder loop. Implementation site: the foreach lowering in
src/backend/spirv/lower.rs (the body is re-emitted per group with the loop
variable's loads pointing at per-group constants). Budget: a code-size cap
clamps the factor. Config: `spirv_unroll` in the tuning table, default 4.

| 2026-09-02 | **llama.cpp CUDA F16 anchor** (`ggml_gemm_h16_bench` — F16×F16→F32 mul_mat, CUDA build-rebis, same box, locked clocks, sync-per-iter 20 iters) | **ggml-cuda: avg 3.271ms, min 3.205ms = 42.0 TFLOP/s** — 83% of the 3060's FP16-acc tensor peak (50.6 dense): their CUDA f16 path IS double-pumped mma, NOT cuBLAS-class-fp32 | fp32 CUDA anchor for the record: 10.9ms / 12.6 TFLOP/s (= the Vulkan anchor; cuBLAS-class SGEMM hits the FP32 shader peak). ggml-cpu(8T) f16: 1025ms / 134 GFLOP/s | **THE TRUE RACE TARGET — 2.9× ahead of us, not ~1.4× as estimated.** Our tensor tier (14.3, FP32-acc coopmat) is at the FP32-acc ceiling's 57% and CANNOT pass 42.0 without the FP16-acc tier (user-approved as a separate numerics contract, gate ≤1e-2). The campaign's critical path is now unambiguous: f16-acc coopmat tier + occupancy reshape + A-panel reuse. Also recorded: our GEMM anchor comparison was against their FP32 number; the f16-vs-f16 race starts NOW at 14.3 vs 42.0. |
| 2026-09-04 | **Smem double-buffer staging CORRECT + A/B vs direct loads** (`gemm_h_bench`, 4096³, batched ×20, 6 interleaved rounds, UNLOCKED clocks — root lost since 09-02, `-lgc` denied) | tensor tier now defaults to the smem pipeline (scalar f16 fills → Workgroup staging → coopmat loads from Workgroup ptrs, 2-stage double-buffer, 0x508 barriers). smem: 18.2/21.3/16.6/23.3/19.7/21.8ms (best-ever window 8.15ms = 16.9 TFLOP/s); direct (commit 3d6ffdb2 compiler): 19.7/20.2/**8.62**/23.1/21.2/25.4ms — paired rounds 3-3, medians 20.7 vs 21.2ms | direct best 8.62ms (15.9 TFLOP/s, matches 515160c6's locked number) | **VERDICT: PARITY.** The 3060's 3MB L2 serves the direct B-panel reuse nearly as well as explicit staging; fill+barrier overhead eats what staging saves at this footprint. Correctness: smem 4.476e-03, direct 4.436e-03 — both OK (tiny delta = K-panel accumulation grouping, same gate). Root cause of the month-long "all zeros": `emit_smem_fill` baked `(u*32)` into the constant AND multiplied by c32 → flat = lane + u·1024, each lane covered 1/32 of the arrays (commit 51b791cd). smem stays default (at parity, enables A-panel reuse + occupancy rungs). Open: clock pinning still needs root; sustained-vs-burst gap unresolved. |
| 2026-09-04b | **Perf blocks: harness hardened, R sweep, shape sweep** (plan 2026-09-04-gemm-perf-blocks; in-process A/B — two SPVs, alternating per-round batched submissions, self-A/B ratio 0.985-1.001) | **R sweep @4096³** (correct dispatch, hot rounds): R4 8.3-8.5ms = 16.5 TFLOP/s reference; R2 ratio 0.637 (17.8ms avg); R8 ratio 0.628 (16.4ms avg, 32 acc fragments/lane kills occupancy) → **R=4 CONFIRMED optimal, no config change**. **Shape sweep** (smem-R4): 2048³ 0.905ms = **19.0 TFLOP/s** (24MB, L2-friendly); 4096²×512 1.504ms = 11.4 TFLOP/s (rel 7.2e-03 = f16acc rounding at tier's own ≤1e-2 gate — the bench's 5e-3 bound is the f32-acc contract); 8192³ ~84.8ms = **13.0 TFLOP/s** (rel 8.2e-03, same tier gate). **8192³ smem-vs-direct A/B: ratio 0.767 — smem +23%** (69.5ms stable vs 96.7-99.4 oscillating; footprint 384MB ≫ L2 — the staging hypothesis CONFIRMED where it matters) | ggml-cuda 42.0 TFLOP/s @4096³ locked | **VERDICT: smem default VALIDATED** (never loses at any footprint, wins big at DRAM-bound); R=4 stays. 4096³ smem-vs-direct remains regime-parity (two full A/B runs swung 0.835/1.141 — both kernels' boost windows are 8.2-8.6ms; treat as equal). Bench harness lessons (committed): per-kernel dispatch counts are MANDATORY for differing tile geometry (shared count → half-zero output + fake speedup: the "R2 = 21 TFLOP/s" was an under-dispatch artifact); array literals in shape variants must match M·K/K·N/M·N individually or the baked SSBO offsets disagree with the arg-derived layout (OOB y writes → fence-timeout hang; rel 0.8756 = exactly 7/8 = K-mismatch signature). Harness artifacts NOT compiler bugs — no BUGS.md entries. |
| 2026-09-04c | **STAGE 0: coopmat mma ceiling = HARDWARE PEAK — the portable path can win** (plan 2026-09-04-beyond-coopmat Stage 0; instrument committed: `emit_mma_ceiling_kernels` + `mma_ceiling_bench.c`) | microkernel: 4×4 distinct (A_j,B_j) fragments/iteration, 16 mma, runtime bound, NoContraction-decorated, loaded fragments, varying row bases (fold-proof: three driver optimizations silently eliminated four early drafts — see BUGS.md 2026-09-04). MEASURED (RTX 3060, bound=10000, 4096 wgs, batched ×4): both f16acc and f32acc = **50.1 ms/launch** — the window is **L2-load-bound** (16 coopmat loads/iter ≈ 6.7 TB/s) and the mma work (5.4 TFLOP/launch) fits entirely inside it ⇒ **coopmat mma rate ≥ 107 TFLOP/s** | ggml-cuda anchor 42.0 TFLOP/s | **VERDICT: THE PORTABLE PATH REACHES HARDWARE TENSOR PEAK — Stage 2 (PTX tier) DEMOTED to optional.** Two ledger corrections: (1) the "50.6 TF FP16-acc peak" label is the **FP32-acc** rate — the true F16-acc dense peak on GA106 = **102 TF** (28 SM × 4 TC × 512 FLOP/clk × 1.78 GHz); our measured ≥107 TF/s ≈ 100% of it. (2) The production GEMM's 16.5 TF is **pipeline-bound** (smem loads + fills + barriers eat 3× vs the mma), NOT vendor-lowering-capped. The race is now a PIPELINE optimization problem: fills/barriers/load-ratio — exactly Stage 1 (D1-D4). Stage 0 artifacts: three driver quirks recorded in BUGS.md (constant-fragment mmas = zeros; SSBO stride-16 coopmat loads = zeros; NoContraction ignored for coopmat de-fusion). |
| 2026-09-04d | **Stage 1 D3: paired smem fill — 16.7 → 20.7 TFLOP/s (+24%)** (`spirv_coopmat_fill_pairs`, default 1; tier-scoped to the smem tensor lane) | the fill's per-element half loads/stores pair into v2f16 ops: the a/b/y SSBO members + the smem arrays carry an array-of-v2f16 view (byte-identical, the vec4 machinery's trick, rspirv-dedup-safe), the fill = one aligned v2f16 load+store per pair (no bitcasts, no byte math), the coopmat smem loads + the y stores walk [pair_idx, 0] to half pointers. In-process A/B (4096³, 5 rounds, both 4.476e-03 OK): **6.64 vs 8.25 ms/call = 20.7 vs 16.7 TFLOP/s** | mma-bound floor 2.7 ms (137.4 GFLOP / 51.2 TF FP32-acc... the f32acc rate) | **The fill was ~8× the mma instruction volume per panel — halving it delivered the predicted rung.** The plan decision preceded the state-buffer setup so the pair view applies ONLY to the smem tensor tier's fields (the fallback lanes keep scalar half views — the default is safe). Next: D1 (2 panels/stage, halve barriers), D2 (register-prefetch), D4 (occupancy). |
| 2026-09-04e | **Stage 1 D1: 2 panels/stage — VERDICT REJECTED** (`spirv_coopmat_panels_per_stage`, default 1) | the pipeline is pps-parameterized (the prologue fills, the loads, the mma threading, the runtime refill); pps=2 = 32 mma per barrier pair but 16 live operand fragments + 16 accumulators. MEASURED (in-process A/B vs the D3 baseline): **12.9 vs 6.6 ms/call — 2× SLOWER** (the occupancy cost swamps the barrier savings) with a residual value defect (rel 8.674e-01) | the D3 baseline 6.6 ms / 20.7 TFLOP/s | **The 4×4 register blocking IS the occupancy sweet spot; wider live sets lose.** Debug traps recorded: the D1 refill's panel index must be RUNTIME ((kt_pair+2)·pps+pi — stage s re-reads at pair-iteration kt+2; a compile-time base filled one panel forever); `field_int(key, N)`'s N = the FIELD INDEX not a default (index 1 → None → the config default 2 — 'reverted' binaries kept emitting pps=2); NaN rel-err compares false → the correctness gate printed '0.000e+00 OK' on a NaN-output kernel (the bench now dumps y[0] vs ref vs the f16 grid). The pps infrastructure stays (the pps=1 path verified refactor-identical). |
| 2026-09-04f | **Stage 1 D3b: f16×4 fill — VERDICT REJECTED (both forms)** (`spirv_coopmat_fill_quad`, default 0) | Form 1 (2×-unrolled pairs loop, same v2f16 units): **18% SLOWER** (ratio 1.183) — the same load/store count per pair, only loop overhead halved; the extra live ranges cost more than the branch savings. Form 2 (true v4f16 member view — one 8-byte load/store per 4 halves, count/2 instructions): **13% SLOWER** (ratio 1.133, stable both submission orders). The pairs mode's 4-byte SSBO/smem transactions are the sweet spot; the 8-byte v4f16 access path loses more per byte than the halved instruction count gains — after D3 the fill is no longer instruction-bound. Machinery KEPT: the pair view is width-parameterized (`view_width`, `pair_member_type(width)`, `coopmat_fill_quad_active` = knob ∧ pairs ∧ M/K/N ÷4); the quad unit emits only behind the knob. Default path verified refactor-identical (self-A/B 0.996, 4.476e-03 OK both). | pairs baseline 9.05 ms (A/B window) | **The fill wall is transaction width × latency, not instruction count — pairs (4B) saturates it.** |
| 2026-09-04g | **Stage 1 D5: K-panel stagger — VERDICT REJECTED** (`spirv_coopmat_stagger`, default 0) | the stagger forces co-resident workgroups to fill different K panels → distinct DRAM streams. MEASURED (in-process A/B): **1.148× SLOWER (15%)**. Root cause: the "phase-lock" IS the L2-broadcast efficiency — co-resident WGs at the same K panel share each A/B DRAM fetch via L2. The stagger broke 8-way panel fetch sharing. **The D5 insight (L2 sharing is a feature, not a defect) redirected D4 from desynchronization to within-WG panel sharing.** | D3 baseline ~10 ms (session-conditional) | **L2 broadcast sharing is the dominant hidden optimization. Breaking it for theoretical occupancy is strictly wrong.** |
| 2026-09-05 | **Stage 1 D4: subgroup occupancy (S>1) — VERDICT REJECTED** (`spirv_coopmat_subgroups`, default 1) | S subgroups/sharegroup = LocalSize 32×S, each handles a different tile_n; A panel shared, B per-subgroup. MEASURED (in-process A/B, S=1 vs S=2): **S=2 is 7% SLOWER** (11.2 ms vs 10.4 ms). Root cause: S=2 → smem 16KB → 3 WGs/SM (down from 4) → fewer co-resident WGs → fewer L2 broadcast hits. The 50% thread increase (128→192) does NOT compensate. Infrastructure kept: the S>1 fill partition (cooperative A, per-subgroup B) is correct and ready for future hardware with larger smem budgets. Correctness: both S=1 and S=2 pass 4.436e-03 (OK). | S=1 baseline ~10.4 ms / 13.2 TFLOP/s | **Occupancy is not the bottleneck; L2 sharing dominates. On RTX 3060 (48KB smem/SM), 4 WGs/SM is the optimum.** |
| 2026-09-07 | **D4 RETEST at the 5.3ms era — VERDICT REVERSED: S=2 is 7-8% FASTER** (`spirv_coopmat_subgroups`, default → 2) | the D4 rejection was measured at ~10ms (pre-D3/smem), when smem was 16KB/WG (3 WGs/SM). At the current 5.3ms baseline the smem is 12KB/WG for S=2 (A 4KB + B 8KB) → 4 WGs/SM × 64 lanes = 8 warps/SM vs S=1's 6; the extra warps now hide the fill DRAM latency better than the WGs/SM drop hurts L2 sharing. In-process A/B (4096³, 5 rounds, both 4.436e-03 OK): clean rounds ratios **0.92/0.93/0.94/0.877** (S=2/S=1) → S=2 ≈ 5.35-5.44ms vs S=1 5.76-5.84ms. S=4 retest: **REJECTED** (5-10% slower — B×4 = 20KB/WG → 2 WGs/SM, L2 sharing destroyed). | S=1 5.76-5.84ms (same A/B window) | **Occupancy now pays: the fill's DRAM latency is the binding constraint at 23 TFLOP/s, and 8 warps/SM hide it better than 6. The D4 verdict was era-conditional; the knob lands at S=2.** |
