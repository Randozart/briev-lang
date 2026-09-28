# Plan Index — START HERE (current status)

**2026-09-28.** `main` tip `5f666996`. 430+ files in `docs/plans/`; historical
plans are reference-only (never retroactively edited — AGENTS.md Rule 13).

This index is the fresh-session orientation: the live foreign lanes, the
active umbrella, every workstream's remaining work with pointers, the open
bugs, and recommended starting points. Everything below is a map, not the
territory — read the linked plan before starting a workstream.

---

## Live foreign lanes — do NOT touch from main-side work

| Lane | Worktree | Domain |
|------|----------|--------|
| `feat/e14a-intent-synthesis` | `../briev-e14a` | Electronics `.ebv` (component laws, tolerance model, ERC) — **C2's parked remainder belongs here** |
| `feat/bad-dialect` | `../briv-compiler-bad-dialect` | Embedded / bad-dialect bootstrapper |
| (baseline) | `../briev-compiler-baseline` | Rule 12b A/B worktree — measure `main` only |

Standing exclusion (`2026-09-24-followup-stages.md` §Exclusion): never merge,
never touch; all baselines/gates measure **main only**.

---

## The active umbrella

`docs/plans/2026-09-24-followup-stages.md` — the post-metaprogramming queue.
Declared order: **1 Front D → 2 Wave 1 (done) → 3 Wave 2 → 4 Wave 3 → 5 GPU
re-rank.** Interop Wave 2 has run ahead of stage 1 (independent lanes), so
**Stage 1 Front D is still untouched** despite the ordering.

---

## Workstream 1 — Cross-dialect interop

Plan of record: `2026-09-21-cross-dialect-interop.md`;
design: `2026-09-25-sbv-ebv-bridge.md` (§"The graft pattern", §"Syntax
decision"); execution: `2026-09-27-wave2-sbv-ebv-exec.md`.

| Piece | Status | Next |
|-------|--------|------|
| Wave 1 — provenance, per-module semantics, collision gate, `.rbv`→`.bv` edge | **DONE** (C0–C6, `2026-09-25-interop-wave1.md`) | — |
| Wave 2 static `.sbv`→`.ebv` graft | **C0/C1 DONE** (`5d82be7a`, `47247c89`); **C2 parked** (`5f666996`) | C2's electronics remainder → e14a lane; interop-side: nothing blocking |
| `.sbv` corpus | `examples/silicon/sensor_die.sbv` (die), `examples/silicon/die_board.ebv` (checks clean; `build` needs the tolerance site) | — |
| Wave 2 declarations — `.bv`↔`.abv`, `.bv`↔`.sbv` | not started | declare the edges |
| **Wave 2b runtime pairs** — alias = interface-instance binding (`.bv` drives die pins → MMIO under the alias, synthesized-obj disclosure, unaliased-instance gate; `.abv` buffer surfaces) | not started | **biggest interop deliverable; approved design already written** |
| Wave 3 derivation — transitive bridges, synthesized bridge node, skip-a-runtime refusal | not started | after 2b |

C2 findings + tolerance-site options A–D: tail of `2026-09-27-wave2-sbv-ebv-exec.md`.

---

## Workstream 2 — Front D: retire the deferred emitter (umbrella stage 1)

NOT started. A/B whether the plain general path holds the composite number
(~198 µs) without the structural matcher; retire if pass, rebuild on current
analysis if fail. Full procedure + gates: `2026-09-24-followup-stages.md` §1.
Ties to the **matcher retirement ledger** (`proof-vs-shape.md`: every
structural matcher is a loan with a repayment gate) — other loans: fused
attention family (~1400 lines), `has_warp_slice`, `detect_row_softmax`.

---

## Workstream 3 — GPU performance (umbrella stage 5, re-rank)

Re-rank AFTER stages 1–4 with a fresh baseline. Current ordered candidates
(`2026-09-24-followup-stages.md` §5): **5a** attention 198→125 µs (retire
fused-attention behind composite parity) · **5b** S3b `cp.async` GEMM 4096³
23→38 TF · **5c** warp-slice threshold → config/composite params (mechanical)
· **5d** M4 ladder (`detect_row_softmax`→`detect_reduction`→`GemmPlan`).

Also not started: **M3** producer-consumer chain fusion; **M4** `numeric.bv`
declarations + vocabulary retirement (`2026-09-20-gpu-dialect-beyond-cuda.md`).

**OPEN correctness**: shallow-K emitter race (K≤128, M·N≥1024²) — dispatch
gates those shapes to the slow race-free kernel; the emitter race itself is
unfixed (needs the `ws_debug` position-encoded fill). BUGS.md ≈ line 6270.

Pointers: `2026-09-16-gpu-strategy-findings-and-levers.md` (lever ledger +
the 64³–4096³ vs-cuBLAS map), `docs/architecture/gpu-backend-strategy.md`
(full landscape), vitriol ledger (single-source benchmark ledger).

---

## Workstream 4 — Runtime elimination (`briev_rt.c`)

`lib/runtime/briev_rt.c` is still ~20 KB on main. Families A+B+C (cast lanes,
print family, string ops — 14/14 parity corpora) merged; **remaining families**
(collections/vector, allocator, …) + embedded fold + Electronics/Silicon parts
+ logo swap. Parity harness: `bash benchmarks/parity/run.sh`.
Pointer: `2026-09-09-briev-native-runtime-and-family-realignment.md` (Parts
A–E, parity gates, allocator-ownership + `Asm#` amendments).

---

## Workstream 5 — Language features / expressiveness

- **Collections/watchdogs/memory Phase E** — `seq`/`vol`/`async`/`sync<g>`
  modifiers + the Rule 22 concurrency-classification gate. User-deferred;
  full track (`2026-07-31-collections-watchdogs-memory.md`,
  `planned-features-tracker.md` item 8).
- **`Asm#` two-mode intrinsic** — unlocks prefetch, hand SIMD, `rdtsc`,
  cpuid, TLS, fibers ("one primitive away" tier).
- **Allocator ownership** — `lib/std/alloc.bv` + `alloc-strategies.dbvl`;
  compiler keeps only the `--no-std` bootstrap heap.
- **`inline_frgn!` plugin**; **stable addresses** (address-of + contract
  class — genuinely future).
- **Self-hosting native emission tier** (x86-64/aarch64 or `.s` text) — the
  last rung. Embryo: `lib/compiler/*.bv`, the tamer VM, `compiler-in-Briv`
  dogfood passes. Bare-metal (rv64) tier already proven.
- Pointer: `docs/architecture/briev-capability-frontier.md` (expressiveness
  closure, tier table, the residual gap is ecosystem not mechanism).

---

## Workstream 6 — Silicon / CIRCT

Retire `.cbv` (Part D); `.sbv` semantics beyond the graft; **hook up the dead
`hardware_validator`** (zero call sites — the `.sbv` synthesizability gate
never runs; BUGS.md 2026-09-28, compiler-owned, unclaimed); CIRCT
`ExportVerilog` rejects `hw.module.generated` (OPEN toolchain, BUGS.md ≈5393).

---

## Workstream 7 — Other active tracks

From this index's 2026-09-08 pass — **verify freshness before starting**:

| Plan | Status | What's left |
|------|--------|-------------|
| `2026-09-02-graphics-ray-and-images.md` | Milestone A DONE; **B OPEN** | Storage image through the compute stack + live X11 window |
| `2026-09-06-cpp-expressiveness.md` | **Active** | C++-level expressiveness remainder (ISR/sections split out, DONE) |
| `2026-09-12-dynamics-causal-dag.md` | **Active** | Causal DAG wiring (proven/weak edges), cycle classification, liveness refusal, `--explain-causality`; fusion gated on an LLVM experiment |
| `2026-09-04-beyond-coopmat.md` | Stage 0 DONE; Stage 1 EXHAUSTED; **1.5 ACTIVE** | Portable tier at structural limit; PTX tier (Stage 2) demoted to optional |

---

## Open bugs / known gaps (`BUGS.md`)

- CIRCT `ExportVerilog` `hw.module.generated` — **OPEN** (toolchain).
- GPU shallow-K emitter race — **OPEN, correctness-gated**.
- Baseline-harness defects — **PARTIAL**; protocol round-trip proofs — **PARTIAL**.
- `json.bv` migration blocked on generic type inference + three language gaps.
- `hardware_validator` dead code — OPEN (found in C2; unclaimed).
- `2026-09-11-phase2b2-instance-state.md` — extracted housekeeping item, pending.

**Closed 2026-09-28** (this session, umbrella
`2026-09-28-native-daily-use-gpu-parity-umbrella.md`):
- Tuple-returning defn with a String field — **FIXED-VERIFIED** at tip
  `1d0dc01f` (repro compiles + runs; BUGS.md entry updated).
- BEAST TypeDef members — **FIXED** (`1300ce92`): serialize/deserialize
  round-trip `body.members` + `parse_toplevel` dispatch + round-trip test.
- **Stale-binary guard — SHIPPED** (`478940bb`): `brievc freshness`
  command, mtime compare vs `src/`+`config/`, exit 1 + offending file
  when stale.
- `get_env_int_or` migration — **STALE INDEX ENTRY**: zero references
  remain (env.bv replaced the intrinsic 2026-07-19); no action needed.
- Front D (umbrella stage 1) — **A/B REJECTED 2026-09-25** (deferred
  emitter stays; plain path 26× slower on composite @4096;
  `benchmarks/results/2026-09-25-front-d-ab.md`). Stage 1 verdict
  settled — no re-run needed at the 2026-09-28 tip.

---

## Recommended starting points (no foreign-lane overlap)

1. **GPU re-baseline + re-rank** (umbrella stage 5) — stage 1 (Front D) is
   settled REJECTED; the stage-5 candidate list needs a fresh baseline at
   the current tip before picking 5a/5b/5c/5d.
2. **Wave 2b runtime pairs / alias binding** — approved design, biggest interop value.
3. **Quick wins** — `hardware_validator` hookup (stale-binary guard now
   shipped: `brievc freshness`).
4. **C4 pinout records** — self-contained `.dbv` grammar + validator + `--fab`.
5. **Runtime families H/I/J** — finish `briev_rt.c` (511 lines remain:
   async/event machine, spawn/setenv, Tamer HCALL, string-bitop helpers).

---

## GPU ledger

`2026-08-31-vitriol-gemm-comparison.md` is the single-source GPU benchmark
ledger (M1 GEMV → O2/O3 → GEMM f32/f16 → mma ceiling → CUDA race). Add new
rows there, never rewrite old ones.

## Full landscape reference

`docs/architecture/gpu-backend-strategy.md` — the complete, evaluated
optimization space (Roofline, emitter routes, async pipelines, beat-CUDA
levers, multi-vendor matrix). Use for any GPU-direction question; the
stage/execution plans are the concrete campaigns.

## How to classify a plan you're about to touch

1. Grep `Status:`/`**Status:**` in the first 25 lines — most recent plans carry one.
2. If none: check the ledger or `git log` for the committing session's outcome.
3. If still unknown: read it; date + title usually says pre/post-rewrite.

## Recently closed (reference when touching related code)

| Plan | Closure |
|------|---------|
| `2026-09-08-master-workstream.md` | Phases 1-6 DONE; Phase 7 optional, Phase 8 verified done |
| `2026-09-08-gemm-occupancy-campaign.md` | CLOSED — 0.708ms = 24.3 TFLOP/s (95% HW peak) |
| `2026-09-08-hashmap-rehash-and-foreach-fix.md` | COMPLETE |
| `2026-09-07-init-block-phi-predecessor.md` | COMPLETE (`c4ee2e19`) |
| `2026-09-07-noalias-benchmarking-and-test-alignment.md` | DONE |
| `2026-09-06-isr-handlers-and-sections.md` | COMPLETE |
| `2026-09-06-p0-p1-implementation.md` | DONE — P0 prefetch, P1 strength-reduce |
| `2026-09-05-gpu-profiling.md`, `-kernel-profiling-analysis.md` | COMPLETE |
| `2026-09-04-gemm-perf-blocks.md` | Superseded by the occupancy campaign |
| `2026-09-01-smallm-splitk.md`, `-warp-mlp-ilp.md`, `-vec4-projection-layout.md`, `-cooperative-row-kernels.md` | Rungs landed/refuted; results in the vitriol ledger |
| `2026-08-31-gpu-next.md`, `-o3-float4-loads.md`, `-abv-gpu-by-default.md` | DONE |
| Interop Wave 1 (`2026-09-25-interop-wave1.md`) | DONE (C0–C6) |
| `.sbv` die graft C0/C1 (`2026-09-27-wave2-sbv-ebv-exec.md`) | DONE; C2 parked |
