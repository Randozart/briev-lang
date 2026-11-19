# G2 tiled GEMM: the 16x class is invisible on PTL (Arc B390)

**2026-11-19, PTL.** P2 of the residency-branch-and-canary plan: prove the
`g2_gemm_tiled` full-coverage input-integrity canary *bites* the 16x
over-dispatch class (commit b2d2776f9) under a vendored pre-fix `LocalSize`
parse, while the sampled outputs stay `G2_TILED_BAD=0`.

## Method (rigorous, controlled)

`brievc run` drives the accel lane **in-process**; the C driver is baked into
the brievc binary at build time (`build.rs` links `libbriev_gpu_rt.a`), so an
archive-only swap is inert. Two binaries were built, each with `build.rs`
forced to recompile the C driver:

| binary | md5 | parse | `imul`s in create_kernel |
|--------|-----|-------|--------------------------|
| `prefix.brievc`   | fd12ff53 | `local_total = lx` (pre-fix, X only)   | 0 |
| `postfix.brievc`  | 8fdc8b0c | `local_total = lx*ly*lz` (post-fix)     | 2 |

The disassembly confirms the two binaries genuinely carry different driver
code (the object-code difference is real, not a stale build).

## Result — the canary does NOT bite on this device

Interleaved A/B on the Arc B390, `g2_gemm_tiled` (512³ tiled coopmat GEMM,
`LocalSize 16 16 1`), 4 alternating rounds, same device state:

```
round 1: PRE  [gpu_time: 0.304 ms CANARY_BAD=0]   POST [gpu_time: 0.335 ms CANARY_BAD=0]
round 2: PRE  [gpu_time: 0.288 ms CANARY_BAD=0]   POST [gpu_time: 0.287 ms CANARY_BAD=0]
round 3: PRE  [gpu_time: 0.287 ms CANARY_BAD=0]   POST [gpu_time: 0.287 ms CANARY_BAD=0]
round 4: PRE  [gpu_time: 0.288 ms CANARY_BAD=0]   POST [gpu_time: 0.288 ms CANARY_BAD=0]
```

- **CANARY_BAD=0 in every round, pre and post.** The 16x surplus workgroups'
  out-of-bounds stores are **clamped** by the Arc B390 (robustBufferAccess),
  so they never corrupt the `a`/`b` input arrays the canary recomputes.
- **gpu_time is pre≈post** (~0.29 ms both). The device absorbs the 16x
  over-dispatch without a measurable kernel-time penalty here.
- `G2_TILED_BAD=0` and `GPU_SUM==CPU_SUM` in every round (the sampled outputs
  are written correctly by the in-bounds workgroups).

The one-off "0.107 ms post vs 0.30 ms pre" that first suggested a clean signal
was a **device-idle artifact** (a single lucky run); the interleaved A/B
refutes it as a reliable discriminator.

## What this settles

1. The 16x over-dispatch class is **silent on PTL/Arc B390**: no input
   corruption, no kernel-time cost. A correctness gate (the canary, the
   sampled output check) cannot catch it here.
2. The full-coverage canary in `g2_gemm_tiled` is a **real** input-integrity
   check — but its 16x-class detection is **device-dependent**. On the
   non-clamping device where b2d2776f9 was measured (the RTX 3060 class), the
   same OOB spray corrupts neighbours and the canary **does** fire.
3. "A canary never observed failing is decoration" — on PTL this canary has
   not been observed failing the 16x class, so it must not be cited as a 16x
   regression gate for PTL. It is wired into `g0.sh` as a **GEMM correctness
   rung only** (asserts `CANARY_BAD=0` + `GPU_SUM==CPU_SUM` + compiled-on),
   which is the honest strength it has on this device.

## Next step, precisely

To gate the 16x class on PTL you need a signal that actually moves here —
either (a) a device/query that reports whether the OOB stores are clamped
(disable robustBufferAccess, if the driver allows) so the canary's OOB spray
becomes real, or (b) a workload where the 16x over-dispatch has a wall-clock
cost that survives interleaving (the 4096³ ceiling fixture, not 512³). Until
one of those is measured, the 16x-class gate on PTL stays open.
