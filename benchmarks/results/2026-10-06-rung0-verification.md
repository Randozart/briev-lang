# Rung-0 re-verification at tip + 5b PTX timing-rig status (2026-10-06)

**Context:** Phase 2 of `docs/plans/2026-10-04-three-surfaces-functional.md`.
Rung 0 (the SPIR-V small-N f16 defect) was root-caused + fixed 2026-10-04
(`7f285b6f`, BUGS.md). This note re-verifies it at the current tip and records
the state of the 5b timing rig before the next fill-campaign lever.

Rig: RTX 3060 (2×, driver 615.71.09), Vulkan + CUDA present.

## 1. Rung 0 holds at tip — PASS

Instrument: `bash benchmarks/declared_matmul_gate.sh <fixture> M N K f16`
(the three-mode declared-matmul gate; all-ones exact + patterned f64-ref).

`examples/gpu/gemm_h.abv` is hardcoded 4096³, so a small-N run needs a matching
fixture. Temp fixtures (M=N=K=64/128, shape-general `a[M*K] b[K*N] y[M*N]`)
were built, gated, then removed. Results:

| shape | lane | all-ones | patterned |
|---|---|---|---|
| 64³ f16 | cuda | EXACT PASS (4096/4096) | max_rel 0.000e+00 PASS |
| 64³ f16 | vulkan | EXACT PASS (4096/4096) | max_rel 0.000e+00 PASS |
| 128³ f16 | cuda | EXACT PASS (16384/16384) | max_rel 0.000e+00 PASS |
| 128³ f16 | vulkan | EXACT PASS (16384/16384) | max_rel 0.000e+00 PASS |

No regression since 2026-10-04. Rung 0 is closed at tip.

**Gate gotcha (for the next user):** `declared_matmul_gate.sh` finds its runner
with `ls "$OUT"/*_runner.c`, which skips dotfiles — a fixture named
`.tmp_*.abv` yields "no runner generated". Use a non-dot name.

## 2. The 5b PTX/CUDA timing rig is stale — needs reconstruction

The fill campaign's numbers (27.1 TF CUDA ship, 45.9 no-fill ceiling) came from
an **ephemeral** scratch harness (`/tmp/opencode/gemm_h/harness2`, per
`2026-09-30-stage5b-cuda-gemm-s5.md`), not a committed script. Attempts to
reproduce a PTX-lane timing path today:

- `benchmarks/gpu/gemm_h_bench <gemm_h.spv>` — **dispatch failed** at 4096³ on
  both `BRIEV_ACCEL_DEVICE=cuda` and `=vulkan`. Its `briev_kernel_desc`
  (`mw_bt`, `mw_smem`) is stale for the current kernel — consistent with the
  recorded note "`ptx_gemm_bench` cannot drive the current kernel: MW_SMEM
  below 24576".
- `brievc build gemm_h.abv --backend ptx` emits `gemm_h.ptx` + a runner, but the
  runner's CUDA dispatch **fails** ("briev: dispatch failed") — the PTX lane is
  not runnable through the current `.abv` runner path.
- The generated `.abv` (SPIR-V) runner *does* run under
  `BRIEV_ACCEL_DEVICE=cuda` and `=vulkan`, but a spliced repeat-loop harness
  (reset the node index, time N iterations) measured ~9.1 / ~7.6 TF at 4096³
  f16 — ~2.6× below the recorded 19.9 TF Vulkan figure. The reset-per-iteration
  protocol is **not** the campaign's batched protocol, so those numbers are not
  comparable and the harness was discarded rather than committed.

**Conclusion:** the next fill-campaign lever (L2/B-traffic, per Phase 2') cannot
be measured until a committed, validated GEMM timing harness exists. The
blocking work is rig reconstruction, not kernel changes.

**Recommended next step:** rebuild the campaign's batched protocol as a
committed script — seed once, launch the resident kernel N times *without*
resetting the node index between launches (the resident state persists), report
median ms + TF — and validate it against a known record point (4096³ f16 Vulkan
≈ 19.9 TF quad-fill) before any new lever is measured. Then proceed with the
L2-friendly CTA rasterization experiment (grouped swizzle; the earlier
column-major attempt was REJECTED 2026-09-30).

## Correctness gates run

- `declared_matmul_gate.sh` 64³/128³ f16, both lanes: PASS (above).
- No code changed in this note; suite unaffected (2901 green at tip).
