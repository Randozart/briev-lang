# PTX Tier Execution — S1 driver + S2 emitter

**Date:** 2026-09-08
**Design:** `docs/plans/2026-09-04-beyond-coopmat.md` §Stage 2 (RE-ARMED 2026-09-08, condition #2 met)
**Doctrine:** `docs/architecture/abv-gpu-doctrine.md` — Briev-owned codegen only, no nvcc/cuBLAS in the compiler path.

## Baseline (Rule 12 — recorded BEFORE changes)

Source: `docs/plans/2026-08-31-vitriol-gemm-comparison.md` ledger + this session's GEMM campaign closure.

| cell | number |
|------|--------|
| GEMM f16 4096³ SPIR-V tensor tier (R=4, smem, fused fill) | **0.708 ms avg / 24.3 TFLOP/s** (95% of 25.6 HW FP16-acc peak) |
| Portable tier structural limit (Stage 1 exhaustion, 2026-09-08) | 4.55 ms / 30.2 TFLOP/s |
| mma m16n16 coopmat ceiling (L2-load-bound) | 50.1 ms/launch, ≥107 TFLOP/s mma rate |
| ggml-cuda 4096³ f16 (same box, device 0 = RTX 3060) | 3.2 ms / 42.0 TFLOP/s |
| **S5 gate** | match 42.0 TFLOP/s at 4096³, then beat |

Target GPUs: RTX 3060 (GA106, sm_86), GTX 1070 Ti (GP104, sm_61) — both present
(Device 0 / Device 1). Driver 580.178.04, CUDA 13.0, libcuda + ptxas present.

## S1 — CUDA driver module `lib/runtime/briev_dev_cuda.c`

Mirror `briev_dev_vulkan.c`'s shape (dlopen, dlsym table, no CUDA headers —
raw numeric enums verified against `cuda.h`):

1. **dlopen** `libcuda.so.1`; resolve driver API: `cuInit`, `cuDeviceGetCount`,
   `cuDeviceGet`, `cuDeviceGetName`, `cuDeviceGetAttribute`, `cuCtxCreate`,
   `cuModuleLoadData` (PTX text → module; driver-internal ptxas = the JIT),
   `cuModuleGetFunction`, `cuMemAlloc`, `cuMemFree`, `cuMemcpyHtoD`,
   `cuMemcpyDtoH`, `cuLaunchKernel`, `cuStreamCreate`, `cuStreamSynchronize`,
   `cuFuncSetAttribute` (CU_FUNC_ATTRIBUTE_MAX_DYNAMIC_SHARED_SIZE_BYTES).
2. **BrievDeviceDriver** registration: `available()` (dlopen + cuInit + a
   compute-capable device), `init()`, `create_kernel(ptx_bytes, size, ...)`
   (module + function), `launch`, `launch_dev`, `launch_dev2d`,
   `launch_dev2d_batch`, `download_dev`, `device_name`, shutdown.
3. **Projection semantics** identical to Vulkan: kernel gets ONE flat buffer
   (the packed `%State` projection). CUDA kernel signature: `f(const float* proj)`
   with a `proj_bytes`-sized `__constant__`-like read? No — pass the pointer as
   kernel param (single param). GEPs match the SPIR-V field offsets (arrays
   then scalars, name-sorted, `proj_offset` per field).
4. **Residency**: `cuMemAlloc` VRAM working set + `cuMemcpyHtoD` staging sync
   (dirty ranges) + `cuMemcpyDtoH` download — same contract as `launch_dev2d`.
5. **LocalSize**: PTX uses block dims at launch; default 64×1×1 (keep in step
   with `VK_LOCAL_SIZE_X`), overridable later via config.
6. **Init-time diagnostic** if `ptxas`/JIT absent: the driver's module load
   fails → chain falls through (verbose prints the CUDA error string via
   `cuGetErrorString`).

**Chain order:** `{ &briev_dev_cuda, &briev_dev_vulkan, &briev_dev_opencl }` —
CUDA first (the perf tier), `BRIEV_ACCEL_DEVICE` env override unchanged. The
SPIR-V path stays byte-identical when the probe selects Vulkan.

**Gate:** a hand-written PTX saxpy blob runs through `create_kernel` + `launch`
in a scratch C harness and matches the host reference. Then the REAL test:
existing `.abv` runner compiled with the PTX path? NO — the runner emits SPIR-V
blobs; the PTX path needs the S2 emitter to produce PTX. So S1's correctness
gate is the hand-written PTX harness (mirrors how the coopmat tier was proven:
general path first).

## S2 — PTX emitter `src/backend/ptx/`

Consumes `GemmPlan` + tier eligibility + dispatch predicates (frontend-driven,
backend-contracts §2). Surface declared in `capabilities.rs`. `--backend ptx`.

- **S2a** minimal: non-tensor GEMM/naive + saxpy-shaped kernels in PTX, enough
  to route an existing `.abv` benchmark through the CUDA driver end-to-end and
  match SPIR-V output (the correctness bridge before tensor ops).
- **S2b** tensor family (S3): `mma.sync.aligned.m16n8k16`, `ldmatrix`,
  `cp.async` — per-op microtests first.

## Gates

| Step | Gate |
|------|------|
| S1 | PTX saxpy harness: exact vs host reference on BOTH devices (3060 + 1070 Ti) — **PASS (RTX 3060, 1D + 2D + batch)** |
| S2a | `--backend ptx` routes an existing GEMM `.abv`; y == SPIR-V path (rel 0.0) — **PASS (gemm_bench on PTX blob: max_rel_err 0.000e+00 @4096³)** |
| S3 | single-mma / single-ldmatrix microtests with known fragments — **IN PROGRESS** |
| S4 | shape portfolio correctness (2048³/4096³/8192³/skinny-K/small), tier gates 5e-3/1e-2 |
| S5 | ≥42.0 TFLOP/s at 4096³ (ledger row) |
| S6 | `derive --stochastic` tile sweep → config cache |

## Docs to update (with S2, same commit)

`docs/architecture/backend-contracts.md` (PTX charter row), `docs/HANDOFF-2026-08-31-gpu.md` (status block), `spec/SPEC.md` §9.8 (already written — verify), `AGENTS.md` reference index, `docs/plans/INDEX.md`.

## Undo

New module + new driver file only; SPIR-V path byte-identical under `--backend spirv` and under probe selection. Config changes reverted via git; losers get VERDICT rows in the ledger.