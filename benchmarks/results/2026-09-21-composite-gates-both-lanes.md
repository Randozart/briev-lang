# 2026-09-21 — composite gates: both-lane validation + the 2x root cause

Gate matrix after the runtime double-dispatch fix (all at the fixture
geometries, RTX 3060, double-precision references):

| Fixture | Arm | CUDA | Vulkan |
|---|---|---|---|
| softmax_composite (H=8 NKV=256 D=128) | deferred | 7.03e-06 PASS | 2.06e-05 PASS |
| softmax_composite_small (H=8 NKV=64 D=16) | online | 6.45e-06 PASS | 6.45e-06 PASS |
| m3 attention, 3-node template (4096, dmaj) | chain | 9.65e-06 PASS | 1.02e-05 PASS |
| m3 attention, composite template (4096) | 1-launch | 2.93e-06 PASS | 8.30e-06 PASS |

The Vulkan deferred-arm number crossed 0.99 -> 2.06e-05 with the runtime
fix alone: the SPIR-V backend was never wrong. Root cause + mechanism:
BUGS.md 2026-09-20 entry (re-narrowed 2026-09-21) — the lazy-buffer
"prime" in `briev_accel_rt.c` dispatched the kernel once as an allocation
hook, then the resident path re-seeded from post-prime (clobbered) host
state. The online arm self-healed (first-iteration rescale multiplies
stale acc by Exp(-inf)=0), which is why small-span fixtures passed while
deferred fixtures 2x'd.

Timing at gate geometry (established 2026-09-20, unchanged by the runtime
fix — the fix alters one-time priming, not steady-state dispatch):
composite 1-launch ~198-200 us vs 3-kernel chain 202 us vs flash-decode
91 us (f16 KV). Fresh micro-timing loop rides the next perf session.
