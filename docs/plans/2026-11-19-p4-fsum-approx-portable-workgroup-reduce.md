# P4: `g0_fsum_approx` OpenCL — portable workgroup reduce for the `approx` fold lane

**2026-11-19, scoped. Implementation is the next step (this is the scoping doc, per
"write the plan before plan-driven work").**

## The defect

`g0_fsum_approx` prints `WRONG` on the NEO/OpenCL lane while `g0_fsum_exact` and
both levelzero lanes are correct. The `approx` keyword licenses the reassociated
warp-slice fold lane, which the SPIR-V backend lowers to a hardware
`OpGroupNonUniformFAdd` (the `SubgroupFAdd#` intrinsic) at **Subgroup** scope.
The row kernel launches `LocalSize 32`. Subgroup scope is only equivalent to
"all 32 invocations" when the device's subgroup width == 32 — true on
levelzero (measured 32) but **not** on NEO's OpenCL (subgroup width 16 on Xe),
where each of the two 16-wide subgroups reduces its own half and both write the
row → `G0_SUM_BAD` high / `GPU_SUM` wrong.

## Options considered

| option | status | why |
|--------|--------|-----|
| hardware `Subgroup` scope (current) | broken on NEO | device subgroup width 16 ≠ LocalSize 32 |
| hardware `Workgroup` scope | **REFUTED 2026-11-18** | `G0_SUM_BAD=220`, `GPU_SUM=0 CPU_SUM=0` — the hardware workgroup-scope reduce is unsupported/no-op on this driver (BUGS.md 2026-11-18) |
| device-width `LocalSize` (query subgroup width at dispatch) | not chosen | needs a dispatch-side device query + a per-width local size; the reduce is still hardware-subgroup, so the kernel shape has to match the device |
| **portable workgroup shared-memory reduce** | **chosen (P4)** | device-independent; correct at any subgroup width |

## The fix (P4): shared-memory + ControlBarrier + lane-0 store

Replace the hardware `SubgroupFAdd#` reduce, **in the 32-lane row fold kernel
only**, with a portable workgroup reduction:

1. Declare a workgroup `OpVariable` (shared) float scratch array, sized to the
   kernel's local size (32 for the row fold).
2. Each lane: `shared[LocalInvocationId] = partial;` then `OpControlBarrier
   (Workgroup, Workgroup, Workgroup)`.
3. Tree fold: for `offset` in `[16, 8, 4, 2, 1]`:
   `if (lane < offset) shared[lane] += shared[lane + offset];`
   `OpControlBarrier (Workgroup)` between each level.
4. Lane 0 (or every lane, uniformly after the final barrier) reads
   `shared[0]` — the full 32-lane sum — and that value is what the store uses
   (`lane-0 store`).

Result: a workgroup-wide sum that is correct regardless of the device's
subgroup width. No hardware `OpGroupNonUniform*`.

## The danger (why this is not a one-line change)

`SubgroupFAdd#` is shared machinery:
- `src/backend/spirv/lower.rs` `emit_subgroup_reduce` (~line 1018) is the single
  emitter for the FAdd/FMax/FMin (and the Int `emit_subgroup_int_reduce`)
  hardware reduces.
- The 32-lane **row fold / cooperative-softmax** kernels
  (`src/backend/spirv/kernel.rs` ~1188, ~1362, `synthesize_softmax_stmts`) use
  it at 32 lanes — the ones broken on NEO.
- The **coopmat GEMM** tile-accumulate path also references `SubgroupFAdd#`
  but at 256 lanes (16×16 workgroup). A blanket change to a 32-lane shared
  array would **break GEMM** (it needs 256 lanes).

So the portable reduce must be **sized to the emitting kernel's local size**
and must not disturb the GEMM/coopmat path. The shared scratch array must be
declared per-kernel at the right width, and the emitter needs to know its
kernel's local size.

## Verification (the gates)

On PTL, OpenCL lane (`BRIEV_ACCEL_DEVICE=opencl`), via `brievc run` (in-process;
the C driver is baked into the binary — archive-only swaps are inert, P1
lesson):
- `g0_fsum_approx`: `G0_SUM_BAD=0` and `GPU_SUM==CPU_SUM` (was WRONG).
- `g0_fsum_exact`: stays `BAD=0` (regression check on the non-`approx` lane).
- `g0_gemm` / `g2_gemm_tiled`: stay `BAD=0` / `CANARY_BAD=0` (GEMM not broken).
- Licensed lane (`approx`) `gpu_time ≤ 0.106 ms` (the P4 budget).
- levelzero + Vulkan lanes: unchanged `BAD=0` (the portable reduce must not
  regress the lanes that already worked).

## Rollback

The hardware reduce is one `builder.emit(Op::GroupNonUniformFAdd, ...)` in
`emit_subgroup_reduce`; a new `// 2026-11-19 (P4)` block marks the portable
shared-memory path. Reverting that block restores the hardware form. The GEMM
path is untouched by design.
