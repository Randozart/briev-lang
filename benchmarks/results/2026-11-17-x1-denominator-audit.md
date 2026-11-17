# 2026-11-17 — X1 denominator audit: the work count is sound, the baseline was wrong

Plan: `docs/plans/2026-11-17-device-free-work-queue.md` item 3.
Parent: `docs/plans/2026-11-16-memory-and-xpu-finish-grind.md` §X1.

## Why this needed no device

X1's first action is *"verify the work count of both arms before treating
39.7 ms as the target; a wrong denominator would make the whole comparison
meaningless."* A work count is a property of the **emitted dispatch geometry**,
not of the GPU. The geometry was previously computed inline in the tier gate
(`(m / (16·R)) * (n / 64)`); it is now one value, `GemmPlan::dispatch_geometry`,
that the gate and the audit both read — a hand-copied tile constant cannot drift
from the compiler.

Run it with:

```
bash benchmarks/gpu_workcount_audit.sh
```

## Result

```
512^3:  workgroups=64   tile=64x64 tiles=8x8   covered=262144   output=262144
4096^3: workgroups=4096 tile=64x64 tiles=64x64 covered=16777216 output=16777216
```

`covered == output` at both shapes, and `workgroups == tiles_m · tiles_n`. The
grid visits every output element exactly once: **the dispatch is neither folded
nor short.** X1's "suspect denominator" branch is refuted.

## But the comparison baseline was wrong

Turning the recorded timings into implied rates (FLOP = 2·m·n·k):

| shape | arm | recorded | implied |
|---|---|---|---|
| 512³ | coopmat | 1.05 ms | **0.256 TFLOP/s** |
| 512³ | tiled | 0.12 ms | **2.237 TFLOP/s** |
| 4096³ | coopmat | 371 ms | **0.370 TFLOP/s** |
| 4096³ | tiled | 39.7 ms | **3.462 TFLOP/s** |

X1 called 3.46 TFLOP/s suspicious because it exceeds "the measured SIMD f32
ceiling (1.46) and the f16-storage ceiling (1.63)". But look at 512³: the *tiled*
arm implies **2.24 TFLOP/s** there too — in a 0.12 ms window where a launch
artefact cannot manufacture a 9× gap. Both arms exceed both "ceilings" at both
shapes.

So those two numbers are **reference-kernel achievements, not hardware bounds**.
Comparing a timing against them produces a false "impossible denominator", which
is exactly what X1's caution did. This is a measurement-baseline error, and it
is the third of its kind in this campaign (after the dead-pid soak curve and the
shared `--out` corpus runner): the instrument, not the subject.

Two consistency checks make the point concrete:

* both arms scale ~1.4–1.6× from 512³ to 4096³ (tiled 1.55×, coopmat 1.45×).
  A folded dispatch would **collapse** at the larger shape, not scale up;
* the 8.8× / 9.3× coopmat deficit is unchanged by any of this — it is real.

## What changes for X1

1. **The exit criterion's denominator is settled**: coopmat must be compared
   against the tiled arm's own rate (**2.24 TFLOP/s at 512³, 3.46 at 4096³**),
   not against 1.46/1.63. A coopmat kernel that "beats the ceiling" while losing
   to the tiled arm has not improved anything.
2. **Still needs the device**: whether the recorded 371 ms / 39.7 ms are
   wall-clock-after-completion or launch-only. The audit bounds the *arithmetic*;
   only the device can confirm the *clock*. X4 (`clFinish`) remains the suspect,
   and it is now the only open question in that comparison rather than one of
   two.
3. **The fact-gate row (outcome (b)) can be written from these numbers** — but
   only as provisional until the clock question is answered.

## Files

* `src/backend/spirv/gemm.rs` — `DispatchGeometry`, `GemmPlan::dispatch_geometry`,
  `tensor_tier_eligible` now consumes it, plus
  `x1_dispatch_geometry_covers_the_output_and_the_denominator_is_sound`.
* `benchmarks/gpu_workcount_audit.sh` — prints the table with the reading rules.
