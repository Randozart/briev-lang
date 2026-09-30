# Stage 5b — CUDA GEMM S5 ladder: Phase 0–1 findings (2026-09-30)

Plan: `docs/plans/2026-09-30-stage5b-cuda-gemm-s5.md`. Machine: RTX 3060
(sm_86), driver **615.71.09**, CUDA lane pinned to the idle GPU via
`CUDA_VISIBLE_DEVICES=1`. Harness: generated `.abv` runner + the spliced
gate `main` (all-ones correctness + batched timing), scratch rig
`/tmp/opencode/5b/`. Full suite baseline:
`benchmarks/results/2026-09-30-b1-rebaseline.md`.

## Phase 0 — pre-flight

`brievc freshness` up to date; `gemm_h.abv` built from tip. Both-lane
4096³ all-ones **PASS** (CUDA + Vulkan, maxrel=0, 4096/4096).

## Phase 1 — config-only baseline sweep (no rebuild; `--config-dir`)

4096³ f16, 20 iters, CUDA/GPU1:

| variant | `ptx_tensor_f16acc` | `ptx_tensor_stages` | desc smem | TF |
|---|---|---|---|---|
| A | 1 | 0 (auto) | 24576 | **27.2** |
| B | 1 | 3 | 24576 | 27.2 |
| C | 1 | 2 | 24576 | 27.2 |
| D | 0 | 0 | 24576 | 20.6 |
| E | 0 | 2 | 24576 | 20.7 |
| F | 0 | 3 | 24576 | 20.6 |

**Finding 1 — the `ptx_tensor_stages` knob was silently inert.** All
three depth variants produced byte-identical desc geometry (smem 24576,
block 256) and identical timing. Root cause: the 2026-09-16 strategy
model (`strategy_to_mwnw`) returned its own `stages`, which overrode the
explicit config — violating the documented override contract
(`set_ir_lowering_from_dir`: "a silently ignored override would poison
every measurement"). **FIXED** (`resolve_eff_stages`: explicit nonzero
config wins; auto=0 still defers to the model → ship path byte-identical).
Regression test: `explicit_stage_override_beats_strategy_choice`.

**Finding 2 — f16acc is decisive**: 27.2 (f16) vs 20.6 (f32) — 1.32×.
Matches the E4c rationale (f16 acc halves the accumulator registers).

## Post-fix depth A/B (knob now live), 4096³, 30 iters ×3

| stages | desc smem | TF (×3) |
|---|---|---|
| auto (→3) | 24576 | 27.1, 27.1, 27.2 |
| **2** (E4c's 16 KB config) | 16384 | 27.0, 27.1, 27.1 |
| 3 | 24576 | 27.0, 27.2, 27.2 |

**Depth is perf-neutral on 615** (all within ±0.5%). The E4c-era claim
(stages 2 = 35.51, stages 3 = 35.86 on driver 580, `ptx-mma-issue-ceiling`
/ `2026-09-14-parity-stages3`) does **not** reproduce: the same 16 KB
geometry now measures 27.0.

## Attribution — is 27 a driver regression? (unresolved for CUDA)

| measurement | 580 era | 615 today | ratio |
|---|---|---|---|
| Vulkan coopmat 4096³ | 4.55 ms | 11.46 ms | **2.5× (driver-bound, proven)** |
| mma ceiling (reg-resident, no smem) | 50.1 ms | 50.3 ms | 1.00× |
| CUDA mw 4096³ via `ptx_gemm_bench` | 35.5 TF (E4c, pre-shape-strategy kernel) | 22.9 TF (`ptx_gemm_bench` FAILs correctness on the current kernel) | not comparable |
| CUDA mw 4096³ E2E (`.abv` runner) | — (dual-image CUDA lane was **broken/IMA until 2026-09-30**) | **27.1 TF** | no era baseline |

**No CUDA E2E regression is established.** The 35.5 figure was a
`ptx_gemm_bench`-protocol number on a kernel that predates the
2026-09-16 shape-strategy model; today's 27.1 is the **first working
CUDA-lane E2E measurement** (the lane emitted a bad grid contract → IMA
until the 2026-09-30 dual-image fix, `benchmarks/results/2026-09-30-gemm-4096-gates.md`).
`ptx_gemm_bench` cannot drive the current kernel: MW_SMEM below 24576
IMAs (the kernel emits for a fixed smem), and at 24576 its reference
FAILs (protocol drift since 09-14). A like-for-like driver attribution on
the CUDA lane requires rebuilding an era commit's E2E path — not yet done.

Consequence for the plan: **"27.5 → 38 TF" has no same-protocol historical
baseline** — it is a chase, not a recovery. The Vulkan lane's vendor
regression is proven; the CUDA lane's is plausible (same cp.async +
`bar.sync` + smem-fill class that the driver regressed) but unproven, and
its first measured E2E point is 27.1 TF.

## Next (per the 5b plan hypotheses)

H2/H3/H6 are either refuted-by-proxy (H3 depth, above) or blocked on a
correct same-protocol rig. The live lever remains the documented
structural gap — the smem round-trip (E-series: no-fill KLOOP 46.2 TF vs
smem-fed ship). Work continues there; the config-level lever is now
honestly exhausted and correctly wired.

## Phase 3 — structural probe (smem round-trip is the wall on 615)

Instrument: `ptx_tensor_nofill` (default off) — the mw emitter skips the
K-loop A/B `cp.async` strip fills (the prologue still fills stage 0 once),
keeping ldmatrix + mma + barriers. Timing-only (output garbage). Measured
through the same E2E rig, 4096³, 30 iters:

| variant | TF (×3) |
|---|---|
| ship (fills on) | 27.1 |
| **no-fill (A+B)** | **42.3, 45.4, 45.4** |
| **no-A only** | **38.4, 38.6** |
| **no-B only** | **38.5, 39.3** |

**Removing EITHER operand's fill alone recovers ~11.5 TF** (27.1 →
38.5), and both recover ~18 TF (→45). The cost is therefore not the
volume of one operand's traffic but the **per-kstep fill burst + wait +
`bar.sync` serialization**: halving the burst unblocks the same amount as
removing it entirely. (E-series on 580 concluded "A fill is the wall";
on 615 it is symmetric.)

**The fill path costs 18.3 TF (27.1 → 45.4).** And the compute/ldmatrix
side is intact on 615: no-fill 45.4 ≈ the 580-era 46.2. So the driver
degradation (27.1 vs 580's 35.5 smem-fed) is entirely on the
fill/smem-round-trip path, not the mma schedule — consistent with the
Vulkan lane's proven smem+barrier regression.

Existing fill-side knobs cannot recover it (same rig, 615):

| knob | TF |
|---|---|
| `ptx_tensor_ksteps_per_stage: 2` | 27.1 (neutral) |
| `ptx_tensor_stages: 2` (16 KB) | 27.0 (neutral) |
| `ptx_tensor_b_lookahead: 1` | **5.7** (4× slower on 615) |
| kps=2 + stages=2 | 27.0 |

Note the fill cost is NOT DRAM bandwidth: 4096³ is compute-bound
(2·4096³ ≈ 137 GFLOP vs ≤64 MB of A/B reads). It is the cp.async issue +
smem-write + barrier cadence mechanism.

**Verdict: the structural direction is the correct one** — reduce or
bypass the smem round-trip. The A/B isolation sharpens the target:
- **Register-staging ONE operand** (A or B) removes that operand's smem
  round-trip → projects to ~38.5 TF (E-series E1f measured 42.5 on 580
  with A-resident).
- **De-bursting the per-kstep fills** (stagger A and B issue, or split
  the wait/bar) projects to a similar gain with a smaller emitter change
  — the burst+barrier is the serialization, not either operand.
- `gpu_schedule` DAG fusion amortizes the round-trip across a graph
  (architectural, separate scope).

The config/pipeline lever is exhausted. Design + Rule-20 pre-B one of the
above before implementing.
