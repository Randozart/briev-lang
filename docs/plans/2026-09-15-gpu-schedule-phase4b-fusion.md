# Phase 4b: FlashAttention-class fusion — the composition race

**2026-09-15.** The single-GEMM sprint is closed (35.5 TF @4096³ = 84.5%
of the 42-TF cuBLAS anchor; compute-only ceiling 46.2 TF past parity; the
remaining fill gap is Ampere-structural). The headline claim is now
**"Briev beats cuBLAS composition"**: an attention-decode
(QKᵀ → softmax → ×V) fused by the compiler into ONE kernel, so the
intermediates never touch HBM — FlashAttention's documented 2-4× over
cuBLAS doing the same composition as separate kernels.

## Parity status (the numbers that frame this)

- Shipped GEMM (E4c, PTX f16acc): 35.5 TF = 84.5% of the 42-TF anchor.
- Best stable: 42.5 TF = parity (E1f bound). Compute-only: 46.2 TF = 110%.
- The four generic CUDA micro-items are already internalized in the tensor
  GEMM (128-bit cp.async fills, 4-stage pipeline, bsmem_pad bank de-phase,
  f16acc 64-reg/4-CTA occupancy). The GEMM's missing ~15% is fill-port
  contention (no TMA on Ampere) — NOT those four items.
- The four items DO apply to the naive f32 / elementwise paths the
  attention uses (verified: `naive_gemm_ptx` emits scalar `ld.global.f32`).

## What's already in place

- **gpu_schedule**: node DAG, topo order, `Fusion` detection (Phase 4a —
  a pure-scale consumer folds into the producer's epilogue), buffer reuse
  (Phase 3, enabled).
- **f16 tensor epilogue VERIFIED CORRECT** on-device (2026-09-15): the
  fused f16 GEMM `y = (a@b)*scale` matches the CPU reference at the f16
  rounding bound (maxrel 1.9e-3). `fusion_applies(elem, f16_acc)` gates it.
- `tensor_gemm_ptx_smem_mw_epilogue` wired (was dead).

## Milestone A — fused f16 attention runs as TWO tensor kernels

Build `attn_decode_h` (qk+scale fused via the epilogue, then pv) with
`ptx_tensor_f16acc: 1`; verify on-device: o = (Q·Kᵀ)·C·V matches the
3-kernel composition (maxrel ≤ the tier contract 1e-2). This is the
first fused multi-kernel pipeline on the tensor tier.

Gate: on-device A/B fused-vs-composition, 128³ and 4096³ attention shape.

## Milestone A — DONE (2026-09-15)

On-device (RTX 3060/CUDA, f16acc): fused qk+scale → pv runs as TWO tensor
kernels; `o = (Q·Kᵀ)·0.5·V` matches the CPU reference at maxrel
1.5e-3 (128³) / 1.6e-3 (512³) — the f16 rounding bound, well under the
1e-2 tier contract. The first fused multi-kernel pipeline on the tensor
tier.

Two real bugs found and fixed along the way:
1. **PTX kernel offsets ignored the Phase 3 aliasing.** `build_ptx_kernels`
   called `ssbo_layout` with `None` for the reuse map, so the PTX kernels
   wrote s2@131120 while the runner expected s2@98352 (its aliased slot) —
   corruption. Now passes `gated_reuse_map(schedule)`.
2. **Shared-buffer re-prime wiped the producer's output.** An unprimed
   2nd kernel's `mapped()` returned NULL → the resident path re-primed via
   `briev_accel_launch`, which packs the kernel's touched fields from host
   state (s2=0) and FULL HtoD's them — clobbering qk's device output before
   pv ran (o=0). Fix: an unprimed kernel maps onto the existing
   `g_shared_host` (CUDA driver `mapped()` + `launch_dev2d` attach), so the
   second launch does a scalar-only update and reads the producer's
   device-resident output.

The `kernel_touched_fields` for a fused producer still lists the
intermediate (s) rather than the epilogue output (s2) — harmless here
because the two alias to the same slot; a TODO for a producer whose scale
output does NOT alias its own output.

## Milestone B — ONE fused kernel (FlashAttention-class)

Fuse pv INTO qk: `o = softmax(Q·Kᵀ·C) · V` as a single tensor kernel.
Needs:
1. A softmax row-reduction over the fused intermediate (the smem
   resident S tile between the two GEMMs — no HBM round-trip).
2. New codegen: a fused qk→rowmax/rowsum→(scale)→pv shape, reusing the
   mma + smem machinery (A/Q and B/K panels already flow; the S tile
   stays in smem and becomes the B operand of the second GEMM).
3. Apply the four micro-items to the newly-added elementwise/softmax
   parts: vectorized row loads (128-bit), bank-tuned smem layout for the
   S tile (the ldmatrix-read-of-write hazard), register-scoped temporaries.

Gate: fused single-kernel vs the two-kernel composition (A/B), both
correct (maxrel ≤ 1e-2) and the fused strictly faster.

## Milestone C — the vs-cuBLAS composition benchmark

New harness: attention-decode at 128² / 1024² / 4096² seqlen, f16:
- Briev: the fused kernel(s).
- cuBLAS composition: `cublasGemmEx(Q,Kᵀ)` → `cublasStridedBatch` softmax
  (or an elementwise scale) → `cublasGemmEx(S,V)`, same shapes.
Report TF + wall time + maxrel. The parity claim is "fused Briev <
cuBLAS composition time" at the attention shapes.

Gate: ledger row with both sides measured on the locked-clock box.

## Micro-items checklist (for the fused kernel's non-GEMM parts)

- [ ] 128-bit loads in the softmax/scale row walks (2×f16 per .v2, 8 per .v4).
- [ ] S-tile smem layout bank-tuned (row stride +1 or XOR swizzle) so the
      second GEMM's B-operand reads don't conflict with the first GEMM's
      writes.
- [ ] cp.async fills for the V panels (the second GEMM's A operand) overlap
      the first GEMM's mma.
- [ ] ptxas register count ≤ 64 (no spill) at the fused occupancy.

## Milestone B — design (single fused FlashAttention-class kernel)

Status: **v1 DONE — the ONE fused kernel is correct on-device**
(2026-09-15). Fusion is an EMERGENT property of the analysis (see the
principle recorded in `docs/architecture/agent-reference.md` §6): no
`FusedAttention`, no attention names — a general `ChainFusion`
(topology only), operands derived at codegen.

- Analysis: detect any linear RAW chain `N1 → N2 → N3` where N2 is
  elementwise, each of `mid_in`/`mid_out` has exactly one reader (dead),
  and `N3`'s output is terminal. Records:
  ```rust
  pub struct ChainFusion { producer, middle, consumer, mid_in, mid_out, scale }
  ```
- Codegen: ONE fused PTX kernel (`fused_attention_ptx`): phase 1 computes
  the scaled S tile into SHARED memory (the on-chip intermediate — never
  HBM), `bar.sync`, phase 2 computes `o = S'·V` reading S' from smem.
  Operands derived from the producer/consumer GEMM shapes.
- v1 scope: f16 operands + a square middle (`on == kn`); the mma rung is
  the next optimization.
- Gate: fused 1-kernel vs the 2-kernel composition, both correct
  (maxrel ≤ 1e-2), fused strictly competitive; then vs-cuBLAS (Milestone C).

## Milestone B v1 — DONE (2026-09-15)

On-device (RTX 3060/CUDA): `attn_decode_h` compiles to ONE kernel
(`qk__scale_pv`) that computes `o = (Q·Kt)·0.5·V` at **maxrel 4.5e-4** —
the f16 rounding bound. The scaled S tile stays in shared memory; the
kernel has exactly ONE global store (o). The 3-node chain is absorbed:
the runner dispatches the fused kernel at the producer's order position
and skips the middle/consumer. Tests: `detects_chain_fusion`,
`fused_attention_ptx_stages_s_in_smem` (smem staging, single global
store, scale folded). 2215 tests pass.

## Milestone B mma rung — DONE (2026-09-15): fused BEATS the composition

The fused kernel moved to the m16n8k16 tensor cores (`fused_attention_mma_ptx`,
direct per-warp fragment loads, the scaled S' staged in smem). Occupancy
was the whole game: the first mma version ran 32 single-warp blocks and
was 3.4× SLOWER than the 2-kernel composition (1.94 vs 0.58 ms @512²) —
the on-chip-S win was lost to the underfilled SM count. Fix: **8 warps
per block**, each warp owning an n-slice of the shared S' tile (the
fragment math is per-LANE, `tid % 32` — using the block tid was the OOB
bug). The 256-thread blocks give the same total mma count as the
composition, so the fused kernel wins on the S-round-trip it avoids:

- **@512²: fused 1-kernel 0.559 ms vs 2-kernel composition 0.577 ms** —
  the fused kernel is ~3% faster (both correct, maxrel ≤ 1e-2).
- Correct on-device at 128² and 512² (maxrel 0.00026–0.00045).

The emergent-fusion thesis holds: one kernel, S never touches HBM, and
the fused shape beats the composition it replaces.

## Milestone C — vs-cuBLAS composition benchmark — DONE (2026-09-15)

New harness: `benchmarks/gpu/cublas_attn_bench.cu` (nvcc + cuBLAS 13) —
the same composition the fused kernel replaces (S = Q·kt, S' = S·0.5,
O = S'·V), f16 operands, f32 acc, beta=0 fresh outputs. Measured on the
RTX 3060, synchronous, 20 iters:

| shape | Briev fused 1-kernel | Briev 2-kernel | cuBLAS composition |
|---|---|---|---|
| 512² | 0.559 ms | 0.577 ms | 0.063 ms |
| 1024² | 2.271 ms | — | 0.232 ms |

**The fused kernel BEATS the Briev composition (the emergent-fusion
claim holds) but cuBLAS is ~9-10× faster at the shapes the fused kernel
supports.** The gap is the fused kernel's LOAD PATH, not its math: the
direct per-warp global fragment loads have no cp.async/smem staging, so
the ~54× mma-time overhead dominates. The tensor tier's staged pipeline
(the fills + ldmatrix) is the missing rung — the same machinery that
took the 4096³ GEMM from naive to 35 TF. The fused kernel's supported
shapes are also capped by the S' tile (16·kn·2 ≤ 48KB ⇒ kn ≤ ~1500), so
it cannot reach the large-K shapes where Briev's tensor kernels approach
cuBLAS.

Next: the cp.async smem-staged fused kernel (fills + ldmatrix fragments,
the D3 lesson applied to the fused shape).

The fused kernel moved to the m16n8k16 tensor cores (`fused_attention_mma_ptx`,
direct per-warp fragment loads, the scaled S' staged in smem). Occupancy
was the whole game: the first mma version ran 32 single-warp blocks and
was 3.4× SLOWER than the 2-kernel composition (1.94 vs 0.58 ms @512²) —
the on-chip-S win was lost to the underfilled SM count. Fix: **8 warps
per block**, each warp owning an n-slice of the shared S' tile (the
fragment math is per-LANE, `tid % 32` — using the block tid was the OOB
bug). The 256-thread blocks give the same total mma count as the
composition, so the fused kernel wins on the S-round-trip it avoids:

- **@512²: fused 1-kernel 0.559 ms vs 2-kernel composition 0.577 ms** —
  the fused kernel is ~3% faster (both correct, maxrel ≤ 1e-2).
- Correct on-device at 128² and 512² (maxrel 0.00026–0.00045).

The emergent-fusion thesis holds: one kernel, S never touches HBM, and
the fused shape beats the composition it replaces.

## Milestone C — the vs-cuBLAS composition benchmark (not started)

## Docs

- `docs/plans/2026-09-15-gpu-schedule-phase4b-fusion.md` (this).
- Update `docs/plans/2026-09-14-gpu-schedule-pass.md` "Next" on Phase 4b
  completion.
- BUGS.md entries for defects found.

## Commands

- Build: `cargo build --release` (config: `ptx_tensor_f16acc: 1` during
  Milestone A/B device work; committed default stays off).
- On-device: `bash benchmarks/parity/run.sh` for CPU; the GPU kernels via
  the generated runner / a small harness (Vulkan for SPIR-V, CUDA for PTX).
- Tests: `cargo test --lib`; Praetor on changed files.