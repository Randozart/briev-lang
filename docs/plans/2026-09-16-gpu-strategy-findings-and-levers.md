# GPU shape-strategy selector — complete findings & lever ledger

**Author:** Randy Smits-Schreuder Goedheijt <randozart@gmail.com>
**2026-09-16.** Consolidates the full session: the baseline that redirected
the plan, the fused-attention regression finding, the cost model, the
dispatch wiring, the honest vs-cuBLAS A/B correction, and every lever
identified (measured verdicts + open items). Plan:
`docs/plans/2026-09-16-gpu-shape-strategy-selector.md`. Architecture:
`docs/architecture/gpu-strategy-selection.md`.

## 1. The arc (what happened)

1. **Stage 0a baseline** — measured our tensor GEMM across 7 shapes vs
   cuBLAS. Found the GEMM is at cuBLAS parity for 256³+; the real gaps are
   SMALL shapes (64³/128³ underfill) and the FUSED attention kernel.
2. **Finding C — the fused attention 1-kernel is a 10× REGRESSION** vs the
   2-kernel tensor-tier composition (0.752 vs 0.073 ms @512²). The phase4b
   "fused beats comp" milestone compared against a STALE composition that
   predated the tensor-tier wiring. Chain fusion fired automatically and
   slowed every f16 attention by 10×. **Fixed: gated OFF** behind
   `ptx_fused_attention` (default 0). NOT a softmax problem (the chain is
   `GEMM → scale → GEMM`; online-softmax is only relevant for a real
   exp/max/divide chain).
3. **Stage 0b/0c** — the cost model (`src/analysis/gpu_strategy.rs`):
   `candidate_strategies` (warp-tiles × stages, pruned) +
   `estimate_time` (roofline + pipeline overlap + underfill + occupancy).
   Calibrated to reproduce the decompiled cuBLAS tile family.
4. **Stage 1** — wired `gpu_strategy::select` into `build_ptx_kernels`.
   Small-shape gap closed: +85% @128³, +87% @256³, no 4096³ regression.
   Fixed a latent runner bug (2-warp mw kernel mis-dispatched through the
   S3b grid).
5. **Stage 3** — occupancy penalty → model picks E4c stages=3. +1.7%
   @4096³, +5% @1024³.
6. **Stage 4** — validated on rectangular GEMM (4096×512×512: 20 TF) and
   a bias-add chain (non-attention middle).
7. **The honest A/B correction** — the "beats cuBLAS at every shape" table
   was WRONG (cuBLAS DEFAULT algo + optimistic clock state). Rigorous
   re-measurement with cuBLAS best-of-11 tensor algo search tells the real
   story (below).

## 2. The honest vs-cuBLAS A/B (corrected)

Method: cuBLAS best-of-11 tensor algos, both sides interleaved on device 0,
3+ rounds, batch timing, all Briev outputs max_rel ≤ 6e-3. GPU 0 (display),
unclocked (clock-lock needs sudo). cuBLAS algos: DEFAULT + ALGO0..9_TENSOR_OP.

| Shape | Briev | cuBLAS best | Verdict |
|-------|-------|-------------|---------|
| 64³   | 0.16 TF | 0.05 TF | **Briev 3.2×** |
| 128³  | 0.88 TF | 0.36 TF | **Briev 2.4×** |
| 256³  | 4.34 TF | 3.34 TF | **Briev 1.3×** |
| 512³  | 11.3 TF | 14.3 TF | cuBLAS 1.26× |
| 1024³ | 20.0 TF | 19.6 TF | **Briev 1.02×** (parity) |
| 2048³ | 25.4 TF | 24.5 TF | **Briev 1.04×** |
| 4096³ | 23.4 TF | 26.2 TF | cuBLAS 1.12× |

**Verdict: Briev wins small shapes decisively (64³–256³), holds parity at
1024³/2048³, loses the largest (512³ by 26%, 4096³ by 12%).** The 4096³ gap
is NOT strategy-selection — the selector picks our best (128×128/stages=3 =
23.4 TF vs our own 512-thread 256×128 at 21.0 TF, 2026-09-11). It is a
kernel-efficiency gap (cuBLAS's 256×128 register blocking).

Caveats on the comparison: unclocked (both sides), cuBLAS best-algo search
done but not exhaustive across cublasLt workspaces, display on GPU 0 (both
sides share it). A clock-locked run (`sudo nvidia-smi -lgc`) would firm the
absolute numbers; the RATIOS at small shapes are robust.

## 3. The lever ledger — every identified lever and its measured verdict

### Measured dead ends (do not revisit without a new architecture)

| Lever | Measured verdict | Why it lost |
|-------|-----------------|-------------|
| **E7 persistent tiles** (wave-tail "unused cores" — grid-stride loop to reclaim partial-wave idle) | **REJECTED** (2026-09-13) | The loop CFG costs ~8 registers → spills OR a lost CTA slot; both cost more than the 24%/9%/1% tail recovery. 3 attempts, all measured. |
| **E8a warp-specialization** (8 consumers + 2 fill-only producer warps — the fill-pipeline "unused cores"; E7b showed fills cost 10.7 TF = 23%, no-fill ceiling 46.2 TF) | **FIXED but 34% SLOWER** (2026-09-14) | 320T CTAs → 3 CTAs/SM occupancy tax. The prize (3.5–7 TF) was undecidable on paper; measurement decided against. `ptx_tensor_warp_spec` stays off. |
| **E5/E6 ladder** (stages, k-depth, lookahead, bank layout) | **REJECTED** — every deeper-pipeline axis loses a CTA slot and loses net. | Occupancy law: 4 CTAs/SM × 8 warps, ≤64 regs, ≤16KB smem. |
| **L6 ship micro-polish** | **CLOSED** (2026-09-14) | Five-axis config sweep exhausted. |
| **E8b** (4 producers + 4 consumers) | never built — gated on E8a winning; E8a lost. | — |

### Open levers (not yet done)

| Lever | Promise | Effort | Notes |
|-------|---------|--------|-------|
| **L4 — small-K family** (K=64-128, decode/attention-head GEMMs, M 128-4096) | **formalize the win we already measured** — small shapes are our strongest (2.4× @128³) | cheap | cuBLAS is split-K-bound here; our full-K f16 accumulation may lead. Mostly a sweep + gate + dispatch entry. The plan predicted this ("decode shapes" class neither Triton nor cuBLAS tunings own). |
| **L3 — Split-K for 2048³** (the other tail fix; 2048³ = 256 CTAs/112 slots = 2.29 waves, ~24% loss) | 2048³ 31.5 → ~38-39 TF | one session | Different from E7 (more CTAs, not a loop) so the register-cost lesson doesn't apply. f32 workspace + combine kernel; compiler-native (contracts prove associativity + workspace liveness). S = ceil(112·2/ctas) bounded to divide K/16, K ≥ 512. |
| **L5 — gpu_schedule pass** (inter-node DAG analysis: sync elimination, y-lifetime buffer reuse, epilogue fusion) | composition + launch overhead speedup | medium | Plan doc exists (`2026-09-14-gpu-schedule-pass.md`). The real attention win now that the fused 1-kernel is gated off — the composition (0.073 ms @512²) is the path. |

### The 4096³ ceiling (the honest open gap)

4096³ at 35.5 TF locked = **84.5% of cuBLAS's 42 TF anchor** — a measured
architecture ceiling. E7b decomposed it: pure compute 46.2 → +fills 35.5
(the fill machinery costs 23%). Both levers that could lift it (E8a,
persistence) measured negative at THIS kernel's register pressure. Moving
4096³ needs a new kernel architecture with body pressure ≤56 regs — not a
tuning.

## 4. The architecture as it stands

### Cost model (`src/analysis/gpu_strategy.rs`)

- `GpuHardware` — per-device table; `SM86` const holds RTX 3060 measured
  values (102 TF, 360 GB/s, 3 MB L2, 100 KB smem/SM, 48 KB cap, 65,536
  regs/SM, 28 SMs). Honest future home: config `gpu-targets.dbvl`.
- `candidate_strategies(m,n,k,hw)` — warp-tiles (mhr=4 → 64×32, mhr=2 →
  32×64) × (mw×nw) grids ≤ 256 threads × stages 1..=4; pruned by
  divisibility, smem ≤ 48KB, CTA count ≥ 1.
- `estimate_time` — `time = compute + memory/stages` (pipeline overlap) +
  underfill penalty (< SM count) + occupancy penalty (smem pinning CTAs/SM
  below the 4-CTA sweet spot). The occupancy term is what fixes the
  stage=3-vs-4 residual.
- `select` — min estimated time.

### Dispatch wiring (`src/backend/ptx/mod.rs`)

`build_ptx_kernels` calls `gpu_strategy::select`, maps the tile back to the
emitter's `(mw, nw, stages)` via `strategy_to_mwnw`; `select_mw_nw` stays as
fallback. Also fixed: `dispatch_geometry_stmt` now distinguishes a 2-warp mw
kernel (block_threads=64, shared>0) from the S3b single-warp (shared=0).

### Fused attention

`build_fused_attention_kernel` returns None unless `ptx_fused_attention`
(default 0). The composition (2 tensor GEMMs, 0.073 ms @512² = 86% cuBLAS)
is the correct default. The kv-staged kernel and the mma/staged/naive
emitters remain in the tree, gated, for future revival only behind a cost
model that proves fusion wins.

### Tests

2269 lib tests pass, including: `calibration_matches_cublas_tile_family`,
`stage_preference_calibration`, `rectangular_shapes_get_feasible_tiles`,
`e4c_tile_preserved_at_4096`, `small_shape_gets_smaller_tile_than_big`.

## 5. What's committed and what isn't

All work is committed and pushed except the final docs commit (`4137fb32`,
the A/B correction) which is 1 commit ahead of origin/main — push pending.

## 6. Next (when work resumes)

1. **L4 small-K sweep** — the cheap, high-claim win. Sweep K∈{64,128} ×
   M∈{128..4096} vs cuBLAS, gate the existing kernel, ship a dispatch entry.
   Formalizes the decode-shape lead we already measured.
2. **L3 split-K for 2048³** — the biggest single-shape fix (24% tail).
   f32 workspace + combine kernel, compiler-native.
3. **L5 gpu_schedule pass** — composition/attention speedup via DAG
   analysis (sync elimination, buffer reuse, epilogue fusion).
4. **4096³** — document as at-ceiling; revisit only with a sub-56-reg
   kernel architecture.
5. Optional rigor: clock-locked A/B (`sudo nvidia-smi -lgc`) and a cublasLt
   workspace search to firm the absolute cuBLAS comparison.

## L4 progress (2026-09-16) — the decode-shape claim, measured

Swept K∈{64,128} × M=N∈{256,512,1024,2048,4096} vs cuBLAS (best-algo):

| Shape | Briev | cuBLAS | Verdict |
|-------|-------|--------|---------|
| 256×256×64 | 2.10 TF | 0.74 | **Briev 2.8×** |
| 256×256×128 | 3.27 TF | 1.38 | **Briev 2.4×** |
| 512×512×64 | 5.01 TF | 1.86 | **Briev 2.7×** |
| 512×512×128 | 7.28 TF | 3.65 | **Briev 2.0×** |
| 1024×1024×64 | 7.23 TF | 9.46 | cuBLAS 1.3× |
| 1024×1024×128 | 11.2 TF | 13.3 | cuBLAS 1.18× |
| 2048×2048×128 | 14.1 TF | 19.3 | cuBLAS 1.37× |
| 4096×4096×128 | 15.6 TF | 21.9 | cuBLAS 1.4× |

**The claim boundary is sharp: Briev wins decode shapes up to M=N=512
(2-2.8×), loses 1024²+ (1.2-1.4×).** This matches the real decode-vs-
prefill split: attention decode runs at small batch (M≤512), prefill at
large M. Briev owns the decode regime.

**Correctness gate is M≤512 for shallow K.** The stages cap (below) fixed
512²×128, but 1024²×64/128 remained wrong — a non-deterministic race across
scattered (m_cta,n_cta) tiles at shallow K. **RESOLVED (root cause + fix):**
the `wait_group stages-2` sync left a stage in flight that the ring reuse
read early. Full drain (`wait_group 0`) at K ≤ 128 fixes it (1024²×64 AND
1024²×128 both max_rel=0, stable); deep K keeps `stages-2` for the async
overlap (8% measured cost at 2048³). The S3b gate was removed; the mw
kernel is correct at shallow K now. The full shallow-K family now works:
256²–512² at 2-2.8× vs cuBLAS, 1024² at ~7.3 TF (still ~1.3× behind cuBLAS).

**Model changes shipped with this finding:**
1. `candidate_strategies` caps stages at 3 (was 4). stages=4 at shallow K
   (8 ksteps) triggers ring-reuse corruption; large shapes already prefer
   3, so no perf regression (4096³ 23.4 TF, 1024³ 20.6 TF unchanged).
2. `tensor_gemm_ptx_smem_mw_opt` uses `wait_group 0` at K ≤ 128 (race fix)
   and `stages-2` at deep K (overlap preserved).