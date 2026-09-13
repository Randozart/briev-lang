# Research 2026-09-13: the parity path + the contract-leverage thesis

**Context:** ship GEMM at 35.5 TF @4096³ (84.5% of the 42-TF cuBLAS
anchor, ~69% of the 51.2 nameplate). E8a (warp-spec) built but
stage-1-incorrect. This session's research: (1) the E8a fix, (2) parity
calibration, (3) the "declarative contracts beat C" strategy with
literature anchors.

## Finding 1 — E8a fix candidate: `fence.proxy.async.shared::cta`

CUTLASS `arch/barrier.h` (fetched 2026-09-13) confirms named barriers
(`bar.arrive`/`bar.sync` with thread counts) are the sanctioned Ampere
producer-consumer mechanism — our E8a approach is correct in form.

The file also defines **`fence_view_async_shared()` = 
`fence.proxy.async.shared::cta;`** — "a shared memory fence for async
operations." CUTLASS gates it `__CUDA_ARCH__ >= 900`, but the PTX ISA
introduced `fence.proxy.async` in PTX ISA 6.0 (sm_70): it orders the
**async proxy** (cp.async's writes) against the **generic proxy**
(ldmatrix's reads).

**The E8a defect fits exactly:** consumers read producer-written smem
through `bar.sync` + a producer-side `membar.cta` — membar orders the
GENERIC proxy only. If ldmatrix's reads are not ordered against another
warp's cp.async writes, stage-1 data reads stale — the observed
all-region corruption. (The ship kernel never needed this fence because
EVERY thread waits its own cp.async group before reading — same-thread
wait_group is the completion guarantee. Only cross-thread cp.async
consumption — E8a's producer split — crosses the proxy boundary.)

**Test:** `fence.proxy.async.shared::cta` (a) producer-side after
`wait_group 0` before `bar.arrive`, and/or (b) consumer-side after
`bar.sync` before the ldmatrix stream. On-device verdict is the only
judge; ptxas accepting the instruction for sm_86 is gate zero.

## Finding 2 — parity calibration

cuBLAS 42 TF = CUTLASS-class scheduling at ~1458MHz sustained (82% of
the 51.2 nameplate @1777MHz). Our own E1f bound: a perfect fill
schedule at the ship occupancy = **42.5 = parity**. Kernel-level parity
is the top of our own measured ladder, gated only on E8a correctness.

## Finding 3 — the contract-leverage thesis (FlashAttention)

FlashAttention (Dao et al.): wins come from **IO-awareness** — fusing
so intermediates never touch HBM (10-20× memory savings at 2-4K
seqlen; "more speedup on slower GPU memory"; 2-4× faster than cuBLAS
composition). Every such win requires proving fusion legality and
intermediate liveness — knowledge C/CUDA applications hand-encode
per-model. Briev's contracts + DAG prove it automatically:

- **Fusion legality**: contract-proven dead intermediates → fuse nodes
  into one kernel (the intermediate never exists).
- **Sync elimination**: DAG independence → launch back-to-back without
  fences/streams.
- **Buffer reuse**: last-consumer chains (the `global_lifetime` shape)
  → allocator ownership without hazard.
- **Zero-copy residency**: the `.abv` doctrine already runs kernels
  directly on contract-resident state — no H2D/D2H staging, provable.

That is the structural "faster than C" tier: not a faster GEMM, but a
compiler that sees across kernel boundaries — cuBLAS composition
structurally cannot.

## Plan

1. **E8a fence fix**: toy-verify the instruction assembles for sm_86 →
   patch both placements → 2048/4096/8192 correctness gates → A/B vs
   35.5. Win bar: ≥40 TF (parity bound 42.5).
2. **GEMM campaign close**: declare the verdict with the full evidence
   ladder, whatever the number.
3. **Contract-leverage campaign**: the `gpu_schedule` pass (own plan
   doc) — DAG-driven fusion, sync elimination, buffer reuse. Anchor:
   FlashAttention-class wins become automatic.

## Undo

All fences behind the existing `ptx_tensor_warp_spec` knob (default
off); ship cubin byte-identical throughout.
