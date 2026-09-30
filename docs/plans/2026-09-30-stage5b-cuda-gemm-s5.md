# Stage 5b — CUDA GEMM S5 ladder: 4096³ 27.5 → 38+ TF (2026-09-30)

Phase B4 remainder of
`docs/plans/2026-09-28-daily-use-sweep-and-gpu-session.md`; re-rank in
`docs/plans/2026-09-30-stage5-re-rank-and-5c.md` (order 5c → 5b → 5a →
5d; 5c DONE `8c0ece90`). This plan re-derives 5b's scope — the
followup-stages table row ("S3b — cp.async GEMM pipeline, 23→38 TF")
was written stale: **cp.async landed 2026-09-10** (S4 PASS,
`docs/plans/2026-09-08-ptx-tier-execution.md:175-221`, +60-70% over
pre-cp.async). What remains of the row's prize is the S5 ladder on the
now-healthy CUDA lane.

## Scope re-derivation (what 5b actually is)

- **Lane: CUDA/PTX tier only** — kernel `tensor_gemm_ptx_smem_mw`
  (+ `_epilogue`, `src/backend/ptx/tensor.rs:976/1024`), dispatch via
  `select_mw_nw`/`strategy_to_mwnw` (`src/backend/ptx/mod.rs:1386/1428`),
  multi-stage cp.async pipeline (`src/backend/ptx/pipeline.rs`).
- **Not the Vulkan/SPIR-V lane**: its 4096³ numbers (11.23 ms / 11.9
  TF, was 4.55 ms / 30.2 TF) are **driver-bound** — driver 615.71.09
  (installed 2026-09-17) regressed the workgroup-smem + barrier fill
  path; isolation probes in
  `benchmarks/results/2026-09-30-gemm-4096-gates.md` show mma-ceiling
  and GEMV unchanged.
  **CORRECTION (Phase 1, `2026-09-30-5b-cuda-s5-ladder.md`):** an
  earlier revision of this plan claimed the CUDA lane was free of the
  vendor regression and 27.5 → 38 was chaseable with no vendor
  dependency. That was **unproven** and is retracted. The 35.5 TF "ship
  E4c" figure was a `ptx_gemm_bench`-protocol number on a
  pre-shape-strategy kernel; today's 27.1 TF is the **first working
  CUDA-lane E2E measurement** (the lane emitted a bad grid contract →
  IMA until the 2026-09-30 dual-image fix). CUDA-vs-580 attribution is
  unresolved (the same cp.async + `bar.sync` + smem-fill class is what
  the driver regressed, so it is plausible but not proven). Treat
  "→38 TF" as a **chase with no same-protocol baseline**, not a
  recovery.
- **Not front-end work**: S3a/S3b/S3b+/S4/S5-rungs already shipped
  (mma.sync exact, smem staged, K-major swizzled B, 4-stage cp.async,
  register scheduling). 5b = squeezing the remaining ~1.4× on the
  shipped kernel.

## Phase 0–1 status (2026-09-30)

Full measurements + attribution in
`benchmarks/results/2026-09-30-5b-cuda-s5-ladder.md`. Summary:

- Both-lane 4096³ correctness PASS; CUDA E2E = **27.1 TF / 5.07 ms**
  (stable ±0.2%), first working CUDA-lane E2E point.
- `ptx_tensor_stages` was **silently inert** on the strategy path
  (2016-09-16 model override) — **FIXED** (`resolve_eff_stages`, explicit
  config wins, auto byte-identical; `explicit_stage_override_beats_strategy_choice`).
- Depth is perf-neutral on 615 (auto/S2/S3 all ≈27.1). f16acc decisive
  (27.2 vs 20.6). The config-level lever is exhausted and now honest.
- H3 (depth) refuted-by-direct-measurement; H2/H6 blocked on a
  same-protocol rig (`ptx_gemm_bench` no longer drives the current
  kernel). The live lever is the documented structural smem round-trip
  (E-series: no-fill KLOOP 46.2 TF vs smem-fed ship).

## Phase 3 — structural verdict (2026-09-30)

`ptx_tensor_nofill` (diagnostic, default off; mode 1=A+B, 2=A, 3=B)
skips the K-loop cooperative fills, keeping ldmatrix + mma + barriers.
E2E 4096³: **ship 27.1 → no-fill(A+B) 42.3–45.4; no-A 38.5; no-B 38.9
TF**. Removing EITHER operand's fill alone recovers ~11.5 TF — the cost
is the per-kstep fill burst + wait + `bar.sync` serialization, not one
operand's volume. The compute side is intact on 615 (45.4 ≈ 580's 46.2);
the whole degradation is the fill path. Existing fill knobs cannot
recover it (`kps=2` 27.1, `stages=2` 27.0, `b_lookahead` 5.7, kps2+s2
27.0). Not DRAM bandwidth (4096³ is compute-bound).

**Direction confirmed: reduce/bypass the smem round-trip.** Designs
(pick + Rule-20 pre-B):
1. **Register-stage ONE operand** (A or B) — removes its smem round-trip;
   projects ~38.5 TF (E-series E1f: A-resident, 42.5 on 580).
2. **De-burst the per-kstep fills** — stagger A/B issue or split the
   wait/bar; similar projected gain, smaller emitter change (the burst
   is the serialization, not either operand).
3. **`gpu_schedule` DAG fusion** — amortize the round-trip across a
   multi-GEMM graph (architectural, separate scope).

## Baseline (Rule 12 — measured BEFORE any change)

All numbers RTX 3060 (sm_86), driver 615.71.09, batched protocol
(warmup 5 + timed N in one command buffer, Vulkan timestamps; CUDA row
= wall of the batch/N):

| Metric | Value | Source |
|--------|-------|--------|
| **CUDA 4096³ f16 (current tip, post dual-image fix)** | **4.995 ms / 27.5 TF** | `benchmarks/results/2026-09-30-gemm-4096-gates.md` |
| Vulkan 4096³ f16 (driver-bound) | 11.23 ms / 11.9–12.2 TF | same |
| cuBLAS anchor @4096³ | 42 TF | gates doc sanity anchors |
| Era record (driver 580, 2026-09-07) | 4.55 ms / 30.2 TF | `docs/plans/2026-09-04-beyond-coopmat.md:85` — NOT reproducible on 615 (era-kernel rerun = 11.46 ms Vulkan) |
| S5 gate (ptx-tier plan) | 42.0 TF | `2026-09-08-ptx-tier-execution.md:108` |
| Kernel static stats (current cubin, entry `main`) | REG=64, STACK=0, dyn SHARED=24 KB | `cuobjdump -res-usage` on the embedded `kp0` cubin |
| Ladder history | 2.11 → 17.8 → 18.3 TF (09-10 rungs) → 27.5 TF (dual-image dispatch fix 09-30) | ptx-tier plan + gates doc |
| Full runtime suite | `benchmarks/results/2026-09-30-b1-rebaseline.md` (38 rows, commit `6dca75b0`) — regression guard after |

The B1 table stands as the suite baseline: 5c proved byte-identical
output (`8c0ece90` gate results), so no suite re-run is needed BEFORE
5b starts; a full `--runtime` run is required AFTER.

## Hypotheses (each behind a Rule 20 pre-B experiment)

Ordered by expected value / risk; a refuted hypothesis blocks its fix
— never build on a refuted one.

1. **Occupancy is the ceiling.** 24 KB dynamic shared + 256 threads
   + REG=64: compute blocks/SM from the device attributes (sm_86:
   100 KB smem/SM, 48 warps/SM) — if only N blocks resident and the
   kernel is latency-bound, more CTAs (smaller smem / fewer threads)
   raise throughput. Pre-B: derive occupancy from ptxas stats + a
   block-count sweep on the EXISTING cubin (grid already `gx =
   M*N/(64*block)`; force variants via the shape/descriptor, no code
   change) and time them.
2. **B-swizzle bank conflicts.** B is K-major with XOR swizzle
   `((n>>3)^(k&7))<<4` (16 B chunks). Pre-B: audit ldmatrix access
   pattern against the swizzle analytically AND via a micro-A/B
   (two swizzle variants emitted behind a config knob, correctness
   gate first). Historical note: a prior bank-conflict audit was
   listed open at `2026-09-08-ptx-tier-execution.md:215`.
3. **Stage count / prefetch distance.** 4 stages shipped; one
   outstanding fill was not enough, 4 was the first value tried.
   Pre-B: stages ∈ {2,3,4,6,8} as an emission-level A/B (config knob,
   smem scales `(mw·1024 + nw·2048)·S` — watch the 100 KB/SM cap and
   `select_mw_nw` interplay), all-ones + f64-ref gates at each point
   before timing (kernel-index rule).
4. **Wider tiles under the register budget.** REG=64 today; ptxas
   caps matter (the -maxrregcount=108 IMA lesson, ptx-tier plan:
   capped cubins unshippable). Pre-B: derive the natural reg count
   for candidate (mw, nw) at 256 threads with `ptxas -v` BEFORE any
   device run; only candidates ≤ register file / no-spill proceed.
   `select_mw_nw` growth alternation gave (4,2) 17.4 vs (2,4) 17.3 at
   4096³ in the 09-10 sweep — re-check at today's 27.5 TF base.
5. **cp.async wait granularity / `membar.cta` cost.** The
   wait_group-0 + membar.cta + bar.sync pattern was required for
   correctness on driver 580 (stale-smem bug, BUGS.md 2026-09-10).
   Pre-B: check whether the membar is still load-bearing on 615
   (compute-sanitizer + correctness A/B) — if redundant it is pure
   stall cost. Removing a correctness guard requires a POSITIVE
   sanitizer+gate result, never a timing-only one.
6. **Epilogue cost.** `_epilogue` variant vs plain: measure the split
   at 4096³ (which path the dispatch picks, its share of runtime).

## Execution protocol (per hypothesis)

1. Correctness FIRST, both lanes, at a real shape (kernel-index rule):
   all-ones → y == K exactly (4096/4096), f64 ref ≤ 6 % maxrel,
   corner+center probes — the harness gate in
   `/tmp/opencode/gemm_h/harness2` (`… 4096 4096 4096 <iters> 64
   ones|<other>`), rebuilt from the generated runner of the candidate
   compiler. Carry `gemm_small` 64³ counter fast-forward too.
2. Timing: harness2 batched protocol, interleaved reference/candidate
   ×N, `LC_ALL=C` wall, CUDA lane (`BRIEV_ACCEL_DEVICE=cuda`);
   record ms + TF (2·M·N·K / t).
3. Any emission change goes through a config knob or a proof-licensed
   general path — no new `match arm for one benchmark` (Golden Rule
   23); hardware facts (warp, bar, cp.async widths) stay literal.
4. Record every step (pass AND refuted) in
   `benchmarks/results/2026-09-30-5b-cuda-s5-ladder.md` — timestamped,
   never retroactively edited.
5. After the ladder: full `bash benchmarks/build_and_bench.sh --runtime`
   vs the B1 table + `cargo test --lib` + Praetor diff on changed
   files. On-device gate at 4096³ BEFORE any push (kernel-index rule;
   the 3-day fill corruption lesson).

## Gates

- `cargo test --lib` green; no new warnings; Praetor: no NEW
  diagnostics in changed files (baseline-diff method).
- On-device correctness both lanes at 4096³ (all-ones exact + f64 ref)
  for every candidate BEFORE its timing row.
- B1-suite regression guard after (same-driver relative comparisons).

## Documentation (Rule 13)

- This plan (created before edits).
- Results: `benchmarks/results/2026-09-30-5b-cuda-s5-ladder.md` (new,
  per step).
- `docs/plans/2026-09-24-followup-stages.md`: stage-5 entry points
  here; correct the `1cca9df4` "vendor-blocked" wording (Vulkan-only
  caveat) in the same commit as this plan.
- `config/ir-lowering.dbvl` (+ `src/config_tuning.rs`): any new knob
  ships with rationale comments, defaults preserving current behavior.
- BUGS.md if a defect surfaces (membar question, sanitizer findings).
- `docs/plans/2026-09-08-ptx-tier-execution.md` stays timestamped —
  not edited; this plan supersedes its "S5 open" section as the live
  record.

## Commit strategy

Plan + followup correction first; then one commit per hypothesis that
survives its pre-B experiment (knob + emission + tests + results row
together). Refuted hypotheses get a results row only — no code.
