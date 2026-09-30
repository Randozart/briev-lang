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

## Phase 2' — CTA rasterization — partial: column-major REJECTED

`ptx_gemm_grid_order` (0 = row-major ship, 1 = column-major), all-ones
PASS everywhere, steady-state:

| shape | row-major (0) | column-major (1) |
|---|---|---|
| 4096³ | **27.1** | 11.4 |
| 8192³ | **25.7** | 9.5 |
| 2048³ | ~26 (noisy) | 11.4 |

Column-major is **2.4× worse**: it shares the B slab across concurrent
CTAs but destroys A locality, and A locality dominates (A re-read ×32
with row-major already L2-served). So the ship order is already the best
*single-axis* order; sharing B alone is a regression.

Remaining L2 candidate: a **2D (m_group × n_group) swizzle** that keeps a
few A and B slabs co-resident. Constraint: a slab is ~1 MB (A tile
128×K=4096×2), so L2 (3 MB) holds only ~3 slabs — 2D grouping can only
use tiny groups. Expected gain is therefore bounded; measure before
building.

## Open question (recorded)

The 2048³ steady numbers are noisy on this rig (27.7 → 13.5 → 12.2 in one
window) while 4096³/8192³ are stable — likely the unclocked DVFS caveat
plus possible shared-GPU contention. Any future A/B must interleave
reference/candidate and discard the first run.

## Phase 2'' — forced CTA tile — WIN for deep-K large shapes

`ptx_tensor_force_mw`/`ptx_tensor_force_nw` (0 = auto) bypass the strategy
model. 4096³, all-ones PASS, steady:

| tile (mw,nw) | threads | smem | TF |
|---|---|---|---|
| auto (2,4) 128×128 | 256 | 24576 | 26.8 |
| (4,4) 256×128 | 512 | 36864 | 30.7 |
| **(2,8) 128×256** | 512 | 36864 | **31.5** |
| (4,2) 256×64 | 256 | 30720 | 23.2 |
| (8,2) 512×64 | 512 | 55296 | 21.4 |
| (2,2) 64×64 | 128 | 18432 | 19.0 |

Interleaved auto vs (2,8) (reliable): 4096³ **26.6–27.0 vs 30.7–31.4
(+16%)**; 8192³ 25.7 vs 31.8 (**+24%**); 4096×4096×1024 +12%. The wider
tile cuts A re-reads (n_tiles 32→16). This **validates the memory-bound
reframe**: the E-series rejected these tiles under a compute-bound
assumption; under DRAM-boundedness they win.

**Gate (case-specific):** thin-K `4096×4096×512` (K=512, near-L2, not
DRAM-bound) regresses hard — (2,8)/(4,4) 15–20 TF vs auto 25.7. So the
wider tile is for **deep-K shapes whose operands exceed L2**; keep the
auto tile for thin-K / L2-resident shapes. 2048³ is inconclusive (the
sustained runs throttled — see the hazard note).

**Model gap (Phase 3).** `estimate_time` divides `memory_s` by `stages`
(treating memory as latency that the pipeline hides) and applies the
4-CTA/SM occupancy penalty unconditionally — so it under-rates the wider
tile's traffic saving and over-rates its occupancy cost, picking (2,4).
The fix must model the DRAM-bandwidth *floor* (`max(compute, bytes/BW)`)
and apply the occupancy penalty only when compute-bound. That is a
calibration change requiring the model tests — do it as its own step.

## Phase 3 — model gating (implemented 2026-09-30)

`src/analysis/gpu_strategy.rs`:
- **Roofline floor**: `seconds = max(compute_s, memory_s)/ilp_bonus`
  (was `compute + memory/stages`, which hid the DRAM-bound cost).
- **Occupancy penalty only when compute-bound** — when DRAM-bound the
  SMs saturate memory regardless of CTAs/SM.
- **Candidate cap 256 → 512 threads**, so the wide tiles are candidates.
- **Guards** (both measured): wide tiles (`area > 128×128`) require
  `K ≥ 1024` AND `≥ 256 CTAs`. Thin-K (K=512) and underfilled 2048³ stay
  on 128×128.

On-device (auto, default config), all-ones PASS:
| shape | tile | TF | vs pre-change |
|---|---|---|---|
| 4096³ | wide | 30.7–31.4 | 26.9 → **+16%** |
| 8192³ | wide | 31.7–31.9 | 25.7 → **+24%** |
| 2048³ | narrow | 27.9 | ~26 (no regression) |
| 4096×4096×512 | narrow | (unstable) | guarded to narrow |

Model tests updated: `e4c_tile_preserved_at_4096` → `dram_bound_shapes_
get_wide_tile` (4096/8192 wide); added `thin_k_keeps_narrow_tile`,
`underfilled_2048_keeps_narrow_tile`. Full lib suite 2779 green.

## MEASUREMENT HAZARD (must fix before trusting fine A/B)

This rig's GPU numbers became unreliable over long sessions: the SAME
narrow thin-K kernel measured **25.8 TF early and 12.5 TF later in one
session** (2048³/4096³/8192³ measured fine in the same loop), and
multi-round loops degrade monotonically (e.g. 2048³ 17.9→8.6). Temps are
low (~40–47 °C) and the GPU idles at 210 MHz between runs, so this is not
simple thermal throttle — likely DVFS/clock-state or a second user of
GPU 1. **Conclusions in this file are drawn only from values reproduced
across ≥3 runs in a clean window**; the wide-tile win (+16/24%) and the
2048³/narrow figures satisfy that. Fine thin-K and K=1024 numbers do NOT
yet. A controlled re-measurement (cool-downs, per-round process restart,
ideally GPU 0 with the display) is required before locking the exact
calibration constants.

## Controlled re-measurement (2026-09-30, post-hazard)

Protocol: fresh process per rep (fresh CUDA context), 6 reps per variant,
all TF values recorded + median, forced `(mw,nw)` configs (auto / narrow
(2,4) / wide (2,8)), CUDA on GPU 1, clock-sampled with `nvidia-smi`.

| shape | narrow (2,4) | wide (2,8) | verdict |
|---|---|---|---|
| 4096³ | 27.0 [26.7–27.0] | **31.0 [30.9–31.2]** | wide **+15%**, tight ✓ |
| 8192³ | 25.7 [25.6–26.0] | **31.7 [31.7]** | wide **+23%**, tight ✓ |
| K=1024 (4096²) | 27.2 [26.8–27.5] | **29.7 [29.5–30.0]** | wide **+9%**, tight ✓ |
| 2048³ | 11.9 [11.5–12.0] | 10.3 [8.7–16.0] | both low — rig-limited |
| thin-K K=512 | 19.2 [15.8–20.8] | 19.8 [17.1–21.1] | both low — rig-limited |

**Root cause of the small-shape instability (isolated):** during a 500-iter
2048³ run the GPU sat at **225 MHz / 13.8 W at 100 % utilisation**, while
the 4096³ kernel ramps to ~1800 MHz / 100 W. So *short/small-grid kernels
run clock-capped on this rig* — their absolute TF is a lower bound and
must not drive decisions. The large-shape numbers (4096³/8192³/K=1024)
ramp normally and are the trustworthy basis for the wide-tile win.

**Decision:** the wide-tile default stands (the win is large, tight, and
clock-confirmed at the ramping shapes). The `K ≥ 1024 ∧ ≥ 256 CTAs` guard
is kept — it is principled (underfill) and the 2048³/thin-K evidence,
while rig-limited, never shows the wide tile *better*. The 2048³ "wide
regression" seen earlier (10–15 vs 26) is **retracted as a rig artifact**:
in the controlled run the *narrow* tile also measured ~12.

## GPU-suite correctness under the new default (2026-09-30)

The default GEMM tile changed, so the composite paths were gated:

- Attention composite (`bash benchmarks/m3_attention_harness.sh`), both
  device lanes, max rel err gate < 1e-3:
  - NKV=256 (D=128): CUDA **PASS** (s 1.68e-5, o1 2.38e-7, a 1.23e-5),
    Vulkan **PASS**.
  - NKV=4096 (deep-K PV → exercises the tile selection): CUDA **PASS**
    (s 4.29e-5, o1 3.11e-7, a 1.63e-5), Vulkan **PASS**.
- GEMM matrix all-ones at real shapes: 2048³, 4096³, 8192³, K=1024,
  thin-K K=512 — all PASS (tables above).

Remaining (noted, not done): a full `gemm_chain`/elementwise-fusion
correctness sweep and a `cargo`-level end-to-end GPU run — low risk since
the changed surface is the GEMM tile geometry (covered above) and the
attention composite already emits the fused chain (2–3 kernels).





