# Effect-shape merge A/B — noisy rows

**Date:** 2026-10-21
**Purpose:** confirm that the effect-shape selection merge did not regress the
rows whose full-suite ratios varied by a few percent.

## Provenance

- Baseline: detached worktree `/tmp/opencode/premerge-a7fe` at `a7fe6c3d`
  (main before the effect-shape merge).
- Current: `feat/effect-shape-followups` from `938667f2` (merged selection).
- Both compilers built fresh in release mode.
- Both binaries built from the same `.bv` source with bound `BOUND=50000000`.
- 7 runs per side, interleaved; medians reported.
- Host and harness: same host as the full-suite runs.

## Results

| Benchmark | Baseline median | Current median | Current / baseline |
|---|---:|---:|---:|
| telemetry_stream | 0.158244s | 0.158804s | **1.004** |
| sweep_mid | 0.232553s | 0.232832s | **1.001** |
| ring_buffer | 0.045289s | 0.043958s | **0.971** |
| queue_drain_sym | 0.030170s | 0.029229s | **0.969** |

Raw runs are in the session log; the medians above are the decision basis.

## Verdict

- No confirmed regression: `telemetry_stream` +0.4%, `sweep_mid` +0.1%,
  both inside the noise band.
- `ring_buffer` and `queue_drain_sym` improved slightly; neither is
  selector-triggered.
- The merge's earlier full-suite deltas were run variance, not a codegen
  regression on these rows.

## Follow-up

No code change. The remaining plan items proceed to per-txn `BatchShape`.
