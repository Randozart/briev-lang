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

Status: **not started** (Milestone A is the clean checkpoint). The single
kernel is a NEW PTX emitter (the FlashAttention forward kernel), not an
incremental change to the GEMM emitter:

- Detect the full attention chain (qk → softmax → pv) as one `Fusion` in
  the schedule: `o = softmax_row(Q·Kᵀ·C) · V`, with the S tile provably
  dead (never HBM).
- Kernel structure (the "S stays on-chip" form, valid when a Q row-tile's
  S fits smem):
  1. Block owns a Q row-tile (e.g. 16 rows).
  2. GEMM-1: S = Q_tile·Kᵀ (all N cols) → S smem (4KB @128², 16 rows).
  3. Row-softmax over S in smem (rowmax, then exp/sum) — the
     cooperative-reduce path.
  4. GEMM-2: O = S'·V (mma), the S' tile as the B operand.
  5. Store O.
- Apply the micro-items to the softmax/scale row walks and the S-tile
  bank layout (the ldmatrix-read-of-write hazard).
- Gate: fused 1-kernel vs the 2-kernel composition (Milestone A), both
  correct (maxrel ≤ 1e-2), fused strictly faster; then the vs-cuBLAS
  composition benchmark (Milestone C).

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