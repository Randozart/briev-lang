# GPU strategy selection — the cost model behind the efficient default

**2026-09-16.** The compiler picks the GPU codegen strategy (tile, stages,
load path) per shape automatically, from shape evidence + measured hardware
parameters — the "maximum efficient default" (AGENTS.md Golden Rule 2) made
real. No hand-tuned config, no per-shape match arms. Plan:
`docs/plans/2026-09-16-gpu-shape-strategy-selector.md`.

## The principle

Every shape is efficient in SOME use case:
- **Small shapes** (64³–256³) win with small tiles — parallelism. A big
  128×128 tile gives 1 CTA at 128³, underfilling the SMs.
- **Large shapes** (2048³+) win with big tiles — arithmetic intensity and
  operand reuse.
- **Thin-K** wins with no pipeline (can't amortize the prologue).
- **Deep-K** wins with a staged pipeline (latency hiding).

The cost model derives all of these from one function — a uniform model,
not a lookup table.

## The model (`src/analysis/gpu_strategy.rs`)

### GpuHardware — the measured device table

Per-device constants: peak tensor TFLOPS, DRAM bandwidth, L2 size, smem
per SM, smem CTA cap, register file, SM count. `GpuHardware::SM86` holds
the RTX 3060 measured values (from the GPU ledger:
`docs/plans/2026-08-31-vitriol-gemm-comparison.md`). These belong in config
per device — the honest future home is a `gpu-targets.dbvl` table, not a
hardcoded const.

### candidate_strategies(m, n, k, hw)

Enumerates the feasible (tile, stages) set:
- Warp-tiles from the tensor tier (mhr=4 → 64×32 warp, mhr=2 → 32×64).
- CTA tiles = warp tile × (mw × nw) warp-grid, mw·nw·32 ≤ 256 threads.
- Stages 1..=4.
- Pruned by: shape divisibility, smem ≤ 48KB CTA cap, CTA count ≥ 1.
- Bounded enumeration (constant, not data-dependent).

### estimate_time(m, n, k, strategy, hw)

`time = compute_s + memory_s / stages` (the pipeline model: cp.async hides
memory behind compute; only ~memory/stages is exposed) plus two penalties:
- **Underfill**: fewer CTAs than the SM count → scale by unused-SM fraction.
- **Occupancy**: smem pinning CTAs/SM below 4 → scale by 4/ctas_per_sm. The
  E4c measured sweet spot is 4 CTAs/SM (16KB stages=2, 24KB stages=3); 32KB
  stages=4 drops to 3 CTAs/SM and loses ~1%.

### select(m, n, k, hw)

Returns the min-estimated-time strategy.

## The dispatch wiring (Stage 1)

`build_ptx_kernels` (`src/backend/ptx/mod.rs`) calls `gpu_strategy::select`
for the tile + stages, mapped back to the emitter's `(mw, nw, stages)` via
`strategy_to_mwnw`; the legacy `select_mw_nw` walker stays as the fallback
when the strategy doesn't map. The stage count flows to the emitter's
`stages` and the runner's `shared_bytes` consistently.

### Latent runner bug fixed

A 2-warp mw kernel (block_threads=64, shared>0) was mis-dispatched through
the S3b single-warp grid (`dispatch_geometry_stmt` keyed on
`block_threads > 64`, but S3b also uses 64-thread blocks). Now distinguished
by `shared_bytes > 0` (mw has dynamic smem; S3b doesn't).

## Calibration against the competitor

The decompiled cuBLAS kernel map (7 shapes → 4 kernels, tile/stage family)
is the ground truth the model is validated against:
`calibration_matches_cublas_tile_family` asserts the selected tile area is
within 0.25×–4× of cuBLAS's at every shape. Exact stage counts are
tolerated mismatch — cuBLAS autotunes at runtime; its exact counts are
empirical search artifacts, not formula outputs.

## Measured results (RTX 3060, sm_86, batch timing, all max_rel ≤ 6e-3)

### Corrected A/B (2026-09-16, interleaved rounds, cuBLAS BEST-algo search)

An earlier table (below, marked DEPRECATED) claimed "Briev beats cuBLAS at
every shape". That used cuBLAS's DEFAULT algo, which is suboptimal, and a
different clock state. The rigorous re-measurement — cuBLAS with a
best-of-11-tensor-algo search, both sides interleaved on device 0, 3+
rounds each — tells the honest story:

| Shape | Briev | cuBLAS best | Verdict |
|-------|-------|-------------|---------|
| 64³   | 0.16 TF | 0.05 TF | **Briev 3.2×** |
| 128³  | 0.88 TF | 0.36 TF | **Briev 2.4×** |
| 256³  | 4.34 TF | 3.34 TF | **Briev 1.3×** |
| 512³  | 11.3 TF | 14.3 TF | cuBLAS 1.26× |
| 1024³ | 20.0 TF | 19.6 TF | **Briev 1.02×** (parity) |
| 2048³ | 25.4 TF | 24.5 TF | **Briev 1.04×** |
| 4096³ | 23.4 TF | 26.2 TF | cuBLAS 1.12× |

**Verdict: Briev decisively wins small shapes (64³/128³/256³), holds
parity at mid sizes (1024³/2048³), and loses the largest shapes (512³ by
26%, 4096³ by 12%).** The 4096³ gap is NOT a strategy-selection failure:
the selector correctly picks the best of OUR strategies (128×128/stages=3,
23.4 TF vs the 512-thread 256×128 at 21.0 TF measured 2026-09-11), and
cuBLAS's 256×128 win is a different kernel's register blocking — a
kernel-efficiency gap, not a tile-selection gap.

### DEPRECATED table (retained for the record — see correction above)

| Shape | Briev GEMM | cuBLAS (default algo) | Ratio |
|-------|-----------|--------|-------|
| 64³   | 0.16 TF   | 0.04   | 4.0× |
| 128³  | 0.88 TF   | 0.33   | 2.7× |
| 256³  | 4.34 TF   | 3.10   | 1.4× |
| 512³  | 11.35 TF  | 9.52   | 1.19× |
| 1024³ | 20.62 TF  | 18.61  | 1.11× |
| 2048³ | 26.31 TF  | 23.63  | 1.11× |
| 4096³ | 27.05 TF  | 25.42  | 1.06× |

The wins that ARE solid: small shapes (the one-config-for-all 128×128 tile
underfilled small grids — the selector's smaller tiles close it), and
mid-size parity. The large-shape gap (512³/4096³) is the honest next-step
target: cuBLAS's kernel has more per-thread work (register blocking /
warp-specialization), which the tensor tier's E8a warp-spec path was built
to explore but has not yet shipped at parity.

## The fused-attention lesson (Finding C)

The phase4b "fused beats composition" milestone compared against a stale
composition predating the tensor-tier wiring. On the current compiler the
fused 1-kernel is a 10× regression (0.752 vs 0.073 ms @512²): the 16-row
m-tile gives zero Kt/V reuse. The composition (two tensor GEMMs, 0.073 ms
= 86% of cuBLAS) is the correct default. The fused path is gated OFF
(`ptx_fused_attention`, default 0). This was NOT a softmax problem — the
chain is `GEMM → scale → GEMM`, no softmax; online-softmax is the right
answer only for a real exp/max/divide chain.

## What's deferred

- **Per-device hardware table** in config (cross-GPU calibration). The
  sm_86 constants are hardcoded; a runtime probe or per-device table is the
  honest extension when a second GPU family matters.
- **Runtime strategy probe**: deferred as unnecessary on sm_86 — the static
  model matches the measured optimum. Its residual (clock variance) is a
  measurement concern, not a kernel-selection one.
- **k-chunked fused rewrite**: only worth it if it beats 0.073 ms @512² —
  a much higher bar than the plan assumed; the composition already wins.