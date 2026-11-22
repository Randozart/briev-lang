# Saltpetre Gemm graduation (P3) — RESEARCH (2026-11-22, device session)

Supersedes: nothing. Builds on: `2026-11-22-saltpetre-op-backend-ledger.md`
(P1/P2 landed, `037a54f21`). This is P3: the first graduation. Owner
direction: op-level.

## 1. What PTL has (verified live, 2026-11-22)

- SSH `prestopoverlord@100.92.76.67` reachable (key auth).
- CPU: Intel Core Ultra X7 358H (Panther Lake). GPU: `Intel Panther Lake
  [Arc B390]` iGPU, `/dev/dri/renderD128`, Vulkan 1.4.357.
- OpenVINO python `2026.3.1` installed.
- So all three parties of the A/B live on one machine: saltpetre-CPU (anywhere),
  Briev device lane (Vulkan/iGPU), and the competitor (OpenVINO GPU+CPU).

## 2. The device GEMM gate already exists and is correctness-gated

`benchmarks/intel_ptl/g0_gemm.bv` — 512³ f32 MatMul on the `accel` lane:

- **Host-reference verify node**: 4096 sampled outputs recomputed as CPU dots;
  `G0_GEMM_BAD` counts mismatches.
- **Full-coverage input-integrity canary** (`CANARY_BAD`, 2026-11-18 R2):
  every element of `a`/`b` rechecked against its closed form — catches the
  16x over-dispatch class's out-of-tile writes that a value comparison cannot
  see.
- **No silent fallback**: `g0.sh` runs with `--accel=force
  --deny-accel-cpu-fallback` and asserts the verbose `kernel '…' compiled on
  'vulkan'` line, so a CPU fallback fails the gate.
- `g0.sh` also extracts `# gpu_time: …` from the run — device timing is
  already part of the gate output.

OpenCL on PTL is **reported, not required**: NEO rejects our IL entry point
(`CL_INVALID_KERNEL_NAME` — recorded in `g0.sh` header + results).

## 3. Recorded numbers at the relevant geometries

| item | source | number |
|---|---|---|
| GEMM 512³ f32, tiled, Briev Vulkan @ PTL | `2026-11-19-g2-gemm-tiled-bite.md` P2 A/B | **0.29 ms** (0.287–0.335 across 5 rounds), CANARY_BAD=0 |
| GEMM 512³ f32, CLI reference @ PTL | `2026-11-17-ptl-f16-tier.md` | 0.233 ms — Briev 1.46× behind, launch-bound shape |
| GEMM 4096³ f16, cooperative-matrix, Briev | `2026-11-17-ptl-f16-tier.md` | 26.2 ms |
| GEMM 4096³ f16, OpenVINO GPU p50 | `2026-11-06-intel-ptl-first-pass.md` | 9.33 ms (14.7 TFLOP/s) |
| GEMM 4096³ f16, OpenVINO CPU p50 | same | 288.6 ms (0.48 TFLOP/s) |
| saltpetre CPU scalar Gemm 512³ | **not measured anywhere** | — (this session measures it) |

The gap that makes this graduation meaningful: saltpetre's `gemm` (exec.bv)
is a scalar triple loop. 134 MFLOP at ~0.5–1 GFLOP/s single-threaded f32 ⇒
order **0.1–0.3 s**, i.e. ~300–1000× the 0.29 ms device lane. That ratio is
the number the ledger's `graduation` must carry.

## 4. The honest boundary of P3

The ledger row for Gemm stays `backend: briev` after this session. "Flipping
to the device" is NOT a one-line dispatch, because saltpetre is a host
library running inside a program — routing its Gemm to an `async accel node`
means **saltpetre itself gaining a device execution surface** (declared
kernels + the `accel` machinery from inside a `.bv` stdlib module). That is a
separate architectural piece (P4, own plan). What P3 delivers:

1. The measured baseline that a graduation number requires (Rule 12: every
   performance plan carries a baseline table BEFORE the change).
2. The ledger row's `graduation` field, filled with the concrete spec
   (512³ @ PTL Xe3, device lane = g0-gemm-class, acceptance = CANARY_BAD=0 +
   compiled-on-vulkan + the measured ratio), so "Gemm graduates" is a
   defined, measurable event, not "faster later".
3. The competitor at the same geometry (OpenVINO 512³, measured, not
   extrapolated — the 4096³ record is a different shape regime).

## 5. Proposed steps

- **M1 (repo)**: `tests/saltpetre/gemm_baseline.bv` — one-Gemm-node (moved out of `benchmarks/intel_ptl/` when landing: a `.bv` probe must sit under the recorded contract-gate exemption root `tests/saltpetre`, not the benchmarks tree)
  graph through saltpetre's session API, g0_gemm's exact input formula
  (same geometry, comparable sums), `std/os/time.bv` wall clock. Run locally
  → record.
- **M2 (PTL)**: rebuild `brievc` release (the tree's release binary is
  2026-11-20, stale); `BRIEV_PTL_SSH="ssh prestopoverlord@100.92.76.67"
  bash benchmarks/intel_ptl/g0.sh` → gemm rung: BAD=0 + canary + gpu_time.
- **M3 (PTL)**: OpenVINO MatMul 512³ via the collector lane (`OV_MODEL` 512³
  IR generated on-node), GPU + CPU p50.
- **M4 (repo)**: results doc
  `benchmarks/results/2026-11-22-saltpetre-gemm-graduation-baseline.md` with
  the baseline table + the P4 pointer; ledger `Gemm` row gains its
  graduation spec; the ledger gate learns the promotion-spec form on
  `briev` rows (invariant 5's backend check extends, new named test).
- **Land**: fetch → ancestor check → push main + lane.

## 6. Open question for the confirm gate

M3 depends on OpenVINO's python building a 512³ MatMul IR on-node. If the OV
GPU plugin does not enumerate (first-pass notes said it did), the competitor
is the **CPU plugin at 512³** (still a truthful, same-geometry vendor number)
and the GPU row is recorded as unavailable-with-why, per the PTL-lane rule
("device execution never dropped silently").

## Sources

`benchmarks/intel_ptl/{g0.sh,g0_gemm.bv,collector.bv,ceil_gemm_512_f32.abv}` ·
`benchmarks/results/{2026-11-06-intel-ptl-first-pass,
2026-11-17-ptl-f16-tier,2026-11-19-g2-gemm-tiled-bite}.md` ·
`lib/saltpetre/{exec.bv,op-backends.dbvl,op-census.txt}` ·
`lib/std/{numeric.bv,tensor_util.bv}` · PTL live probe (2026-11-22).
