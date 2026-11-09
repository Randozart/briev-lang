# Intel lane ceilings + compiler efficiency — results (2026-11-09)

Plan: `docs/plans/2026-11-09-intel-lane-ceilings-and-probe-cost.md`.

## W1 — probe-gate emission (DONE)

The output-equality gate compared one basic block per element. Replaced with a
runtime loop per write buffer (same full element-for-element comparison).

| metric | before | after |
|---|---|---|
| `g0_elem.bv` IR | 641 MB / 13.6M lines | 204 KB / 5810 lines |
| `g0_elem.bv` build+link (local) | >45 min (2700 s timeout) | 78 s |
| `g0_gemm.bv` build+link (local) | — (same class) | 27 s |
| gate semantics | every element, tol/exact | unchanged |
| g0_elem output | BAD=0 GPU_SUM=261566496 CPU_SUM=261566496 | identical |

`g0_elem` requires `ulimit -s 65536` (pre-existing, BUGS.md 2026-11-06).

## Gate matrix on PTL (2026-11-09)

Vulkan (required): elem, gemm 512³, sum, minmax, fsum_approx, fsum_exact — all
PASS (`BAD=0`, GPU == CPU, compiled on device).

Level Zero (required, promoted): elem, sum, minmax, fsum_approx, fsum_exact —
all PASS with `compiled on 'levelzero'` and host-identical results. GEMM: CPU
fallback, self-check ok (documented: Vulkan-only until G3 — L0/NEO cannot
consume the Vulkan image, IGC requires OpenCL.std).

OpenCL/NEO (reported): elem, sum, minmax, fsum_approx, fsum_exact — correct
when they run; the exact-fold kernel intermittently hangs in `clFinish`
(BUGS.md 2026-11-09, ~1/3 under load). Never silent: a hang is printed, a
wrong result fails.

Stage F fast smoke (`g1_lane_smoke.abv`): both lanes correct, repeated.

## Fixes along the way

- `--accel=force` accepted (both spellings) and supplied by `g0.sh`; the
  retired per-fixture `!> accel: force;` policy now lives in the gate flags
  (migration audit hole from the attr commit).
- `g0.sh`: `grep -c`/`grep -o` no longer abort the script under `set -e`;
  a wrong lane result fails, a lane hang is reported.
- L0/NEO receive ONLY the OpenCL projection; kernels without one mark the
  chain dead (clean CPU fallback) instead of feeding IGC the Vulkan module.
- OpenCL flavor uses `OpenCL.std` extended instructions (fmin/fmax/s_min/
  s_max/fma/exp/sqrt/fabs); GLSL.std.450 is now impossible on that flavor
  (IGC rejected the min/max fold lane: "Expects OpenCL.std").
- `BRIEV_ACCEL_EXPLAIN=1` prints per-body eligibility/decision and the reason
  a node has no OpenCL projection.

## W2 — ceilings (in progress)

Four-number matrix pending: Vulkan ceiling (hand GLSL), OpenCL ceiling
(hand OpenCL C), Briev-Vulkan, Briev-OpenCL; roofline bounds first. No lane
verdict is drawn from generated-kernel numbers (emitter quality ≠ lane
ceiling).
