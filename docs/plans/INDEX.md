# Plan Index — current status

**2026-09-08.** 426 files in `docs/plans/`. Historical plans are reference-only
(never retroactively edited — see AGENTS.md Rule 13). This index lists the
LIVE plans and the recently-closed ones a new session needs. All others:
read for context, treat as historical record.

## OPEN — real remaining work

| Plan | Status | What's left |
|------|--------|-------------|
| `2026-09-02-graphics-ray-and-images.md` | Milestone A DONE; **B OPEN** | Storage image through the compute stack + live X11 window (swapchain blit). Milestone B sketch at §Milestone B; A outcomes at §Milestone A outcomes |
| `2026-09-06-cpp-expressiveness.md` | **Active** | Design doc for C++-level expressiveness. ISR/vector-table (Phase 9) split out to `2026-09-06-isr-handlers-and-sections.md` |
| `2026-09-12-dynamics-causal-dag.md` | **ACTIVE** | The causal DAG — compile-time wiring (proven/weak edges), cycle classification, liveness refusal, `--explain-causality` report; LLVM experiment gates any future fusion. Findings: BUGS.md 2026-09-12 |
| `2026-09-04-beyond-coopmat.md` | Stage 0 DONE, Stage 1 EXHAUSTED (2026-09-08 note), Stage 1.5 ACTIVE | Portable tier confirmed at structural limit; PTX tier (Stage 2) DEMOTED to optional. Read before any new GPU perf work — the campfire note at line ~127 is the current truth |

## RECENTLY CLOSED (reference when touching related code)

| Plan | Closure |
|------|---------|
| `2026-09-08-master-workstream.md` | Phases 1-6 DONE; Phase 7 optional, Phase 8 items verified done (see `2026-09-08-continuation.md`) |
| `2026-09-08-gemm-occupancy-campaign.md` | CLOSED — 4.58ms wall was a stale-build artifact; 0.708ms = 24.3 TFLOP/s (95% of HW peak) |
| `2026-09-08-hashmap-rehash-and-foreach-fix.md` | COMPLETE |
| `2026-09-07-init-block-phi-predecessor.md` | COMPLETE (`c4ee2e19`) |
| `2026-09-07-noalias-benchmarking-and-test-alignment.md` | DONE (`12c0b3b8`, `d0220812`, dead-test deletion) |
| `2026-09-06-isr-handlers-and-sections.md` | COMPLETE (signed off, committed) |
| `2026-09-06-p0-p1-implementation.md` | DONE — P0 prefetch (`335d028d`), P1 strength-reduce (`dd5f5e26`) |
| `2026-09-05-gpu-profiling.md`, `2026-09-05-kernel-profiling-analysis.md` | COMPLETE |
| `2026-09-04-gemm-perf-blocks.md` | Superseded by the occupancy campaign closure |
| `2026-09-01-smallm-splitk.md`, `-warp-mlp-ilp.md`, `-vec4-projection-layout.md`, `-cooperative-row-kernels.md` | Rungs landed/refuted; results in the vitriol ledger |
| `2026-08-31-gpu-next.md`, `2026-08-31-o3-float4-loads.md`, `2026-08-31-abv-gpu-by-default.md` | DONE |

## GPU ledger

`2026-08-31-vitriol-gemm-comparison.md` is the single-source GPU benchmark
ledger (M1 GEMV → O2/O3 → GEMM f32/f16 → mma ceiling → CUDA race). Add new
rows there, never rewrite old ones.

## Full landscape reference

`docs/architecture/gpu-backend-strategy.md` is the complete, evaluated
optimization-space document (Roofline reality, emitter routes, async
pipelines, Briev beat-CUDA levers, multi-vendor matrix, roadmap). Use it
for any GPU-direction question; the stage/execution plans are the concrete
campaigns.

## How to classify a plan you're about to touch

1. Grep for `Status:`/`**Status:**` in the first 25 lines — most recent plans carry one.
2. If none: check the ledger or git log for the committing session's outcome.
3. If still unknown: read it; the date + title usually tells whether it's pre- or post-rewrite.