# Experiment report: cuBLAS decompile, pipeline module, load-path measurement

**2026-09-16.** First experimental cycle. Three experiments, two findings,
one reusable module. Status: in progress.

## Experiment 1: cuBLAS kernel strategy decompile

**Method:** cuda-gdb `set cuda break_on_launch application` on a single-GEMM
binary at 7 sizes (64→4096³). Captured kernel name and SASS instruction mix.

**Finding:** cuBLAS selects **4 different kernels** for 7 sizes:
- 64³: CUTLASS WMMA 32×32, 1 stage
- 128³: CUTLASS WMMA 32×32, 2 stages
- 256³: cuBLAS-native 64×64, **5 stages** (32 HMMA, 28 LDGSTS.128)
- 512³: cuBLAS-native 96×128, **4 stages** (48 HMMA, 14 LDGSTS.128)
- 1024³: cuBLAS-native 128×128, 1 stage
- 2048³: cuBLAS-native 128×128, 1 stage
- 4096³: CUTLASS mma 256×128, **3 stages** (64 HMMA, 18 LDGSTS.128)

**Universal pipeline skeleton** (verified across all pipelined kernels):
1. Prologue: N_stages × (cp.async + LDGDEPBAR)
2. BAR.SYNC.DEFER_BLOCKING
3. Loop: ldmatrix → HMMA×N → fill next (LDGSTS) → LDGDEPBAR → stage wrap
4. Epilogue: barrier → final HMMA → store

**What this means for us:**
- Our tensor GEMM uses ONE 32×32 tile for all shapes — matches cuBLAS at
  ≤128³ only.
- At 4096³, cuBLAS uses 256×128 (64× more elements per CTA) = fewer CTAs,
  more data reuse, 64 HMMA per loop.
- The pipeline skeleton is universal — our pipeline.rs captures it.

**Lesson for generalization:** The tile/stage selection is a compiler
decision, not a kernel decision. The compiler should pick tile × stages
from the shape (smem budget, occupancy, K-depth).

## Experiment 2: panel-pipeline module (pipeline.rs)

**Method:** Extract the universal pipeline skeleton as parameterized PTX
emitters. 13 unit tests.

**Result:** Module compiles, tests pass, 555 lines.

**Key primitives:**
- StageConfig (power-of-2 or 3 stages)
- SmemLayout (gemm/fused variants)
- emit_stage_modulo, emit_fill_stage (the ring)
- emit_ldgdepbar, emit_bar_sync (synchronization)
- emit_prologue (initial fill + barrier)
- emit_main_loop (loop label + body callback + k-step)
- emit_ldmatrix_b_trans, emit_ldmatrix_a_x4 (fragment loads)
- emit_b_slab_base, emit_a_slab_base (stage addresses)
- FillConfig (derive fill width from per-thread work)
- f32_to_f16, f16x2_pair (utilities)

**Praetor:** 3 diagnostics (emit_main_loop had 8 params → refactored to 6).
Pre-existing baseline diagnostics in other files.

## Experiment 3: load-path measurement

**Method:** Single-warp (32 threads), M=16, two kernels:
- Direct: scalar `ld.global` per fragment (our fused kernel's path)
- Staged: `ld.global.v4 + st.shared.v4` fills, `ld.shared` reads

**Result:**
| Size | Direct | Staged | Ratio |
|------|--------|--------|-------|
| 128³ | 0.1 TF | 0.2 TF | 1.39× |
| 256³ | 0.5 TF | 0.5 TF | 1.12× |
| 512³ | 1.1 TF | 1.3 TF | 1.14× |
| 1024³ | 2.6 TF | 3.0 TF | 1.17× |
| 2048³ | 5.4 TF | 6.4 TF | 1.17× |
| 4096³ | 11.2 TF | 13.2 TF | 1.18× |

Both correct (s[0] matches at all sizes).

**What this means:**
- The staged fill-path gives a consistent **~18% speedup** at large sizes.
- This is the FILL improvement only (K-panel through smem instead of
  scattered global loads).
- The remaining ~10× gap to cuBLAS comes from:
  1. **Warp count**: 1 warp (32T) vs cuBLAS's 128-256T
  2. **HMMA density**: 16 per k-step vs cuBLAS's 32-64
  3. **Pipeline stages**: 0 stages (synchronous) vs cuBLAS's 2-5 stages
  4. **Tile size**: 16×8 (m16n8k16 per warp) vs cuBLAS's 256×128 CTA

**Honest assessment:** The staged fill helps (+18%), but the dominant gap
is occupancy + tile size, not the fill path alone. To close the 10× gap,
we need multi-warp CTAs with the staged pipeline.

## What worked
- The cuBLAS decompile: clean, reproducible, directly actionable.
- The pipeline module: correct, testable, reusable.
- The load-path experiment: isolates the fill-path effect cleanly.

## What didn't work (yet)
- The 18% staged speedup is real but modest — the fill path is NOT the
  dominant bottleneck.
- The fused kernel's 10× gap is primarily occupancy + tile, not fills.

## Next steps
1. Wire the pipeline module to the fused kernel's K-panel fills (the 18%
   path) — easy, incremental.
2. Increase warp count in the fused kernel (the BIG win) — requires
   restructuring the n-sub loop.
3. Add pipeline stages to the fused kernel — requires the smem double-buffer.
4. Cross-kernel validation on .abv files.
