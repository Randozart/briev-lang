# Value-destination migration branch baseline

**Date:** 2026-10-30
**Branch:** `feat/generalized-value-destination-ownership`
**Baseline commit:** `e90c7843` (same code as `main` at branch creation; branch
initial commits are documentation/design-only)
**Scope:** baseline for generalized destination-ownership migration Phases
D0–D8. Timing values below are historical evidence from the same compiler
behavior and must be re-run before replacing an A/B result.

## Compiler correctness baseline

- `cargo test --lib`: 3378 passed, 0 failed (2026-10-29, commit behavior
  unchanged by branch docs).
- IR determinism quick: ALL DETERMINISTIC.
- Effect-shape quick guard: PASS.
- Recommender browser fake-peer gate: PASS at `e90c7843` lineage after
  commit `e110fd1e` and PR #48.
- I11 real browser/server gate: PASS, `maxLongTaskMs=0`.

## Runtime evidence carried from 2026-10-28/29

The following are the latest full-correctness runtime observations before this
branch. They are baseline expectations, not new branch measurements.

| Benchmark | Briev | C | Ratio | Output |
|---|---:|---:|---:|---|
| ws_frame | 0.0385s | 0.0244s | 1.58x C | MATCH |
| deep_recursion | 0.0180s | 0.0241s | 0.75x C | MATCH |
| global_lifetime | 0.0128s | 0.0247s | 0.52x C | MATCH |
| arena_bump | 0.0953s | 0.0218s | 4.37x C | MATCH |
| spsc_ring | 0.0463s | 0.0250s | 1.85x C | MATCH |

Known quality gaps are not accepted as new regressions without an A/B. The
`ws_frame` hot path remains the first performance target for generalized
object destination/scalar-replacement work.

## Migration policy

- No lane may compare against the unsafe no-copy 0.012s control as a valid
  correctness baseline; that control returned a callee stack pointer after
  return.
- Every candidate timing requires same-commit output MATCH and interleaved
  A/B against this baseline or a newly recorded corrected baseline.
- IR snapshots for `ws_frame`, FSM, SPSC, JSON/list/object fixtures must be
  captured before Phase D2 modifies lowering helpers.

## Initial branch commits

Documentation/design commits on this branch do not alter compiler behavior.
The first behavior-bearing commit will be Phase D1 (analysis schema completion)
and must preserve IR determinism.
