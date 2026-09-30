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

## Phase 2 — register-staging — REFUTED by scaling + bandwidth analysis

Before implementing, a shape-scaling A/B (ship vs no-fill A+B) was run to
locate the cost. Steady-state (first cold run discarded — unclocked DVFS
adds a slow first dispatch), 4096³-class rig, f16/tensor tier:

| shape | operands | ship | no-fill | gap |
|---|---|---|---|---|
| 2048³ | 8 MB | 26.0 TF | 36.4 | 10.4 |
| 4096³ | 32 MB | 27.1 | 45.7 | 18.6 |
| 8192³ | 128 MB | 25.7 | 47.3 | 21.6 |
| 4096×4096×512 | 4 MB | 25.6 | ~27 | **~0** |

The gap appears **only when the operands exceed L2 (3 MB)**; at thin-K
(4 MB, largely L2-resident) there is no fill cost. Effective bandwidth at
4096³: A re-read N/128 = 32× plus B re-read M/128 = 32× ⇒ ≈2.1 GB of
operand traffic in the 5 ms kernel → **≈420 GB/s, at the 360 GB/s DRAM
limit** (L2 supplies the rest). 8192³: ≈17 GB / 42.8 ms ≈ 397 GB/s.

**Conclusion: the "fill cost" is DRAM traffic from operand re-reads; the
kernel is memory-bound at large square shapes.** The no-fill ceiling
(45.9) removes those reads and is therefore **unreachable** — it is a
compute-only bound, not an achievable target.

**Register-staging is REFUTED.** Staging an operand per-warp changes the
smem round-trip for *extra global reads*: with the (2,4) tile, A is
shared by the 4 nw warps (4× redundant reads) and B by the 2 mw warps
(2×). At 4096³ that would push A traffic from ~1 GB to ~4 GB — strictly
worse, bandwidth-doomed. (The E-series E1f 42.5 must have been an
L2-resident / microbench regime, not this one.)

**Correct lever: increase reuse / L2 locality, not remove the fill.**
- Larger CTA tile (fewer operand re-reads) — the E-series rejected big
  tiles on *occupancy* grounds under a compute-bound assumption; under a
  memory-bound reality the trade flips and must be re-measured.
- **L2-friendly CTA rasterization** (swizzle the 1D `ctaid.x` → tile
  decode so concurrent CTAs share B slabs in L2) — cheap, no kernel
  body change, directly targets the re-read traffic.
- Split-K for underfilled shapes (unchanged from the findings ledger).

Phase 2 (register-staging) is therefore closed REJECTED, and the campaign
pivots to the reuse/L2 axis (a new phase; see the plan).

