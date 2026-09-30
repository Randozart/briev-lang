# Stage 5b — structural fill-path campaign results (2026-09-30)

Plan: `docs/plans/2026-09-30-stage5b-structural-fill-campaign.md`.
Rig: `/tmp/opencode/5b` (scratch) — generated `.abv` runner + spliced
gate `main`; CPU reference checks; batched timing. GPU: RTX 3060 sm_86,
driver **615.71.09**, CUDA lane pinned to GPU 1 (`CUDA_VISIBLE_DEVICES=1`).
Baseline prior results: `benchmarks/results/2026-09-30-5b-cuda-s5-ladder.md`.

All numbers 4096³ f16, 30 iters batched, TF = 2MNK/t. Correctness is
checked by all-ones (exact) for every non-diagnostic variant; diagnostic
modes (garbage output) are timing-only.

## Phase 1 — schedule / de-burst — REJECTED

Hypothesis: the per-kstep fill **burst + wait + `bar.sync`** serializes
the pipeline, so re-scheduling it (split commit groups, progressive
waits, interleaved issue) recovers throughput.

Evidence against (before building H-D1/H-D2):

| observation | result | reading |
|---|---|---|
| `ptx_tensor_ksteps_per_stage: 2` (halves fill + bar cadence) | 27.1 TF (neutral) | cadence is not the cost |
| no-A fill alone | 38.5 TF | per-object *bytes* matter |
| no-B fill alone | 38.9 TF | symmetric — the burst, not one operand |
| **mode 4: fills ON, commit/wait/membar/bar SUPPRESSED** | **23.6–23.8 TF** | removing synchronization does not help; it hurts |

Mode 4 (`ptx_tensor_nofill: 4`) keeps every fill instruction but drops
the `cp.async.commit_group` / `wait_group` / `membar.cta` / `bar.sync`.
If the wait+barrier were the serialization, this should approach the
no-fill ceiling (45.9); instead it is *below* ship. So the cost is the
**fill work itself** — the `cp.async` issue, the global read, and the
smem write — net of any benefit the barrier provides.

**Verdict: REJECTED.** De-burst / wait-splitting / interleaved issue
cannot recover the gap. The only remaining lever is to **remove fill
work for an operand** → register-staging (Phase 2). (Rule 20: this
negative blocks all schedule-side follow-ons.)

## Reference points (same window, same rig)

| variant | TF |
|---|---|
| ship | 27.1 |
| no-fill A+B (ceiling) | 45.9 |
| no-A | 38.5 |
| no-B | 38.9 |
| mode 4 (fills, no sync) | 23.6 |
| `kps=2` | 27.1 |

## Phase 2 — register-staging one operand — NEXT

Target: feed the mma A- or B-fragments directly from global registers,
removing that operand's cp.async + smem write + ldmatrix (and its share
of the wait). Projected ~38.5 TF (E-series E1f ≈ 42.5 on 580; here the
no-A / no-B points bound it). Gate on `ptxas -v`: reject any candidate
that drops below the 4-CTA/SM occupancy sweet spot. Correctness gate
(all-ones exact + f64 ref ≤6%) before any timing. Record per-shape
verdicts here; REJECTED paths stand.
