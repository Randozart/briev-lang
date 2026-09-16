# Multi-warp fused kernel: staged Kt/V fills (the cuBLAS lesson applied)

**2026-09-16.** The fused kernel already uses 8 warps (0.559 ms @512²,
beats the 0.577 ms composition). cuBLAS is 0.063 ms — 10× faster. The
gap is NOT occupancy; it's the load path:

- Our kernel: scalar `ld.global.b16` per Kt/V fragment (the direct path)
- cuBLAS: `cp.async` fills → smem → `ldmatrix` → HMMA (the staged path)

The staged version (`fused_attention_mma_staged_ptx`) exists but is 3.9×
slower because it stages Q (full-width, crushes occupancy). The fix:
**only stage Kt/V** (small panels), keep Q as direct loads.

## What changes

1. **Kt panel fill**: 16 threads fill a 16×16 Kt tile via `ld.global.v4.b32`
   + `st.shared.v4.b32` (the load-path experiment's +18% path).
2. **V panel fill**: same pattern for the V panel.
3. **Fragment reads**: `ld.shared.b32` from smem instead of `ld.global.b16`.
4. **Double-buffer**: Kt buffer + V buffer in smem. The V fill overlaps
   with the S computation (the pipeline).

## What stays the same

- 8 warps (32 threads × 8 = 256 threads per block)
- m16n8k16 HMMA per warp
- Two-phase structure (S = Q×Kt, then O = S'×V)
- Q loaded directly from global (no smem staging)

## Smem budget

- Kt panel: 16×16×2 = 512 bytes (one 16×16 f16 tile)
- V panel: 16×16×2 = 512 bytes (one 16×16 f16 tile)
- S tile: 16×kn×2 bytes (the scaled output, written by phase 1, read by phase 2)
- Total: 512 + 512 + 16×kn×2 = 1024 + 32×kn bytes
- At kn=512: 1024 + 16384 = 17408 bytes (well within 48KB)

## Verification

- Correctness: maxrel ≤ 1e-2 vs the direct-load kernel
- Performance: measure vs cuBLAS @512², @1024²
- Full suite: `cargo test --lib`

## This is Step 2 of the plan

Step 1 (pipeline.rs) is done. This applies the pipeline primitives to the
fused kernel's actual load path. Step 3 (tile selection from analysis)
comes after.

## Results (2026-09-16)

`fused_attention_mma_kv_staged_ptx` shipped and wired behind
`ptx_fused_staged` (default OFF). Measured on RTX 3060, 8 warps,
correctness verified against a pristine-q f64 reference at 128²
(0/16384 elements > 2e-2 rel err — the f16 rounding bound):

| Size | Direct (ms) | KV-staged (ms) | Delta |
|------|-------------|----------------|-------|
| 128² | 0.0772 | 0.0766 | +0.8% |
| 512² | 0.907  | 0.829  | +8.6% |
| 1024²| 4.164  | 4.597  | −10.4% |

**Verdict: a wash.** The coalesced fill helps mid-size but the extra
smem + per-panel barriers hurt at 1024². The single-warp load-path
experiment's +18% does not transfer to the 8-warp kernel — the fused
kernel's dominant bottleneck is structural: a 16-row block tile gives NO
Kt/V reuse across m-tiles (each block re-reads the full Kt/V matrices,
64× redundant at 1024² vs cuBLAS's 128×128 tile reusing 8×). The load
path (scalar vs cp.async+ldmatrix) is real but secondary.

**Fix direction (Step 3):** enlarge the block tile (more m-tiles per
block, or a full Kt/V panel per block shared across m-warps) so the
arithmetic intensity per Kt/V byte read rises — the same reason cuBLAS
uses 64×64/128×128 tiles. Never accept the −10% at 1024² as "noise":
it is the barrier/fill serialization dominating at scale.
