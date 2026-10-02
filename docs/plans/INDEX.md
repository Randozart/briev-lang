# Plan Index — START HERE (current status)

**2026-10-02.** `main` tip `ab667667`. 430+ files in `docs/plans/`; historical
plans are reference-only (never retroactively edited — AGENTS.md Rule 13).

**Suite state at tip:** `cargo test --lib` 2843 green; 19 pre-existing
warnings; `gemm_h` byte-identical; Praetor improved vs baseline.

**GPU standing state:** fused online softmax (`ptx_deferred_online: 1`) is
the shipped default — composite + fused online decode = **72.5 µs p50
variant-verified** (two-pass ≡ online at decode geometry,
max_rel 5.894e-06; s8 gate hardened 8.48e-06/6.57e-06/2.06e-05). Shipped-best
decode config: fused + split=4. `ptx_fused_attention`/`ptx_fused_staged`
knobs deleted with the family; `ptx_deferred_skip_pass` diagnostic-only.

**Trusted GPU instruments** (never a hand-spliced probe — a probe
contradicting these means the gates are broken):
`benchmarks/deferred_ab_gate.sh` (variant diff, the A/B instrument),
`benchmarks/m3_attention_harness.sh` (live reference, honors
`BRIEFC_FLAGS`), `benchmarks/softmax_gate.sh` (NaN-hardened, honors
`BRIEFC_FLAGS`), atomic/workid/bad_ptx gates. Late-session kernel work
needs gates re-run at the TARGET geometry — s8 passing says nothing
about decode.

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

`docs/plans/2026-09-24-followup-stages.md` — the post-metacomputing queue.
Declared order: **1 Front D → 2 Wave 1 (done) → 3 Wave 2 → 4 Wave 3 → 5 GPU
re-rank.** Interop Wave 2 ran ahead of stage 1 (independent lanes).
Stage verdicts: **Stage 1 Front D A/B-REJECTED 2026-09-25** (deferred
emitter stays; plain path 26× slower on composite @4096;
`benchmarks/results/2026-09-25-front-d-ab.md` — settled, no re-run).
**Stage 5 GPU re-rank executed 2026-09-30/10-01**: 5c ✅ (`8c0ece90`),
5a ✅ (72.5 µs, target 125 beaten; family retired `a7871a27`),
5d headline ✅ (GemmPlan remainder behind its own A/B), 5b remainder =
the GEMM fill-pipeline campaign (see Workstream 3). Stage table:
`2026-09-30-stage5-re-rank-and-5c.md`.

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

**SETTLED — A/B REJECTED 2026-09-25** (`benchmarks/results/
2026-09-25-front-d-ab.md`): the deferred emitter stays; the plain
general path is 26× slower on composite @4096. Retirement effort moved
to the matcher ledger instead: fused-attention family retired
(`a7871a27`, ~1300 lines); remaining loans: `has_warp_slice` (retired
to config, 5c `8c0ece90`), `detect_row_softmax`, `GemmPlan` (each
behind a perf A/B — 5d remainder).

---

## Workstream 3 — GPU performance (umbrella stage 5, re-rank EXECUTED)

Stage table of record: `2026-09-30-stage5-re-rank-and-5c.md`
(5c ✅ 5b-active 5a ✅ 5d-headline ✅). Session record:
`benchmarks/results/2026-09-30-5a-attention-decode.md`.

**Current queue (in order):**

1. **Attention float4 k/v loads** (72.5 → ~60 µs est.) — design +
   constraints banked in the 5a results file (last two sections):
   transposed d-mapping flip (`lane + 32·i` → `lane·4 + i`) + v4 fusion
   are INSEPARABLE; the pipelined map is the vehicle but the k-expr
   Debug key is strip-invariant (fix: per-strip unique binder
   substitution); fixture offsets 16-aligned; scalar fallback on the
   `Add(row, d_binder)` shape miss. Whole-function treatment — no
   splices into `emit_deferred_region` (postmortem:
   `2026-10-01-5a-fused-j-loop.md`). Gates at decode geometry
   (m3 + softmax + deferred_ab_gate, both lanes) BEFORE timing.
2. **div slowpath** — one sizing probe first.
3. **GEMM fill-pipeline campaign** (32 → 42 TF @4096³) —
   `2026-09-30-stage5b-structural-fill-campaign.md`; the
   contract-licensed pipelining note (fill reorders loads the
   shape proves safe); no-fill evidence bounds the prize (compute
   intact at 45.4 TF); Rule 12 protocol.
4. **Re-rank table fold-in** — fold fused-attention numbers into the
   stage-5 table (row 3 already marked DONE).
5. **GemmPlan retirement** (5d remainder) — behind its own A/B, not
   yet gated.

Also open (post-5 ladder): **M3** producer-consumer chain fusion;
**M4** `numeric.bv` declarations + vocabulary retirement
(`2026-09-20-gpu-dialect-beyond-cuda.md`).

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

1. **Attention float4 k/v loads** (Workstream 3 queue item 1) — design
   fully banked, fresh-session whole-function treatment, est. 10-20%.
2. **GEMM fill-pipeline campaign** (Workstream 3 queue item 3) — the
   biggest absolute prize (32 → 42 TF), plan + correctness license
   written (`2026-09-30-stage5b-structural-fill-campaign.md`).
3. **Wave 2b runtime pairs / alias binding** — approved design, biggest interop value.
4. **Quick wins** — `hardware_validator` hookup (stale-binary guard now
   shipped: `brievc freshness`).
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
| `2026-09-28-daily-use-sweep-and-gpu-session.md` | Phase B GPU (B1–B4) DONE; Phase A core DONE 2026-10-01 (A1 name-capture fix, A2 Stack peek, D5 test_collections repair); A3 executable gate + A5 audit + A6 probe REMAIN |
| `2026-09-30-stage5-re-rank-and-5c.md` | 5c DONE (`8c0ece90`, byte-identical IR gate + determinism fixes); 5a DONE (72.5 µs); session record `2026-09-30-5a-attention-decode.md` |
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
