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

## E8a fence-fix attempt (2026-09-13, session close)

1. **`fence.proxy.async.shared::cta` is ptxas-rejected on sm_86**
   ("Modifier '.async' requires .target sm_90 or higher") — the CUTLASS
   gating was accurate; the fence route is dead on this part.
2. **Generic-proxy producer variant implemented** (st.shared producers:
   `ld.global.nc.v4` + `st.shared.v4`, payload regs %r29-%r32, async
   commit/wait dropped — generic-to-generic ordering through bar.sync is
   the documented-clean path). STILL incorrect: rel 0.6-1.0 across
   K=512..2048, deterministic worst elements, ZERO zero-elements in y
   (all written, systematically wrong values).
3. **Observed K-correlation**: K≤1024 → all-zero y; K=1536 → ~half;
   K=2048 → ~3/4 of ref. More ksteps = more correct. Grid sizes
   differed per shape (16-256 CTAs), confounding K with grid.
4. Ad-hoc harness prints (partial refs, DtoH-overwritten state) added
   noise — a dedicated single-CTA instrumented ws test is the required
   next step, not more driver archaeology.

**State**: ws code default-off; ship cubin byte-identical; 2199 lib
tests green. The stage-1 defect survives four fill mechanisms and both
proxies — the remaining suspects are the barrier PHASE semantics across
reuse (named-barrier state after release), the peel/loop barrier
sequence, or an emitter slip neither audit caught. Fresh session, fresh
eyes, single-CTA instrument.

## E8a single-CTA isolation (2026-09-13, session close)

**Isolated (with corrected y_off args):** 128-wide single-CTA ladder —
K=16 (consumer-only) **PASSES exact**; K=32 (one producer fill) FAIL
0.608; K=64/96/128 FAIL ~0.59-0.61. **The consumer path is exact; the
producer's fill data is wrong.** The earlier "K≤32 small-M" scare was
driver args; the producer-fill defect is real and E8a-specific.

Eliminated across two sessions: async-proxy ordering (generic-proxy
st.shared variant fails identically), producer fill addressing (audited
4× — D-mapping, stage base, stripe offset, slab↔ng mapping all exact),
barrier pairing/counts, the peel order, the %r5 rebase, the y-pass
guard. Hand-patched PTX probes proved too error-prone (the cooperative
variant introduced its own IMA) — the next instrument must be built in
the generator: a `ws_debug` mode that stores each kstep's acc chunk to a
separate y region (per-kstep dump), turning "which kstep/warp/slab is
wrong" into a single read.

**Parity path unchanged:** E1f bound 42.5 = parity, gated on this one
defect. The contract-leverage campaign (gpu_schedule) is independent of
it and carries the FlashAttention-class upside.
