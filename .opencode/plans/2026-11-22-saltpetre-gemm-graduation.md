# Saltpetre Gemm graduation baseline (P3) — PLAN (2026-11-22)

Supersedes: nothing. Builds on: `2026-11-22-saltpetre-op-backend-ledger.md`
(landed `037a54f21`) and the research note
`2026-11-22-saltpetre-gemm-graduation-research.md`.

Owner direction: op-level granularity. P3 is the first graduation: measure the
baseline a `Gemm` graduation number requires, and give the ledger row's
`graduation` field a concrete, machine-checkable spec. The row stays
`backend: briev` — device routing (P4) is a separate plan; it is not done here.

## What this session does

1. **M1 (repo)** — `tests/saltpetre/gemm_baseline.bv`: one-Gemm ONNX
   graph built in source (one `OnnxNode{op_type: "Gemm"}` and its
   `OnnxGraph`), g0_gemm's exact
   input formula (same closed form, same 512³ geometry, comparable sums),
   driven through the real session path (`sp_begin`/`sp_open_coll`/`sp_run`),
   wall-timed with `std/os/time.bv`'s `now_ns`. Runs locally on the lane
   machine; prints `SALT_GEMM_CPU_US=<n>` and a spot-check sum.

   **M1 landed deviations (2026-11-22), both forced and both recorded:**
   - The fixture lives under `tests/saltpetre/`, not `benchmarks/intel_ptl/`.
     The contract gate is `policy: deny` for call sites outside its recorded
     exemption roots, and `config/contracts.dbvl` exempts `tests/saltpetre`.
     The same file outside that root produces 27 `invalid operation`
     diagnostics from saltpetre's own call sites; widening the exemption to a
     benchmarks root would weaken the gate, so the fixture sits beside the
     corpus it exercises.
   - The graph is assembled **inline in the node body**, not in a
     `build_gemm_graph(b) -> OnnxGraph` helper. A `defn` that creates, appends
     to and returns a `List<struct>` hands back a corrupt element store
     (SIGSEGV at the call site). That is a compiler defect, not a graph
     idiom; it is recorded as an OPEN entry in `BUGS.md` (2026-11-22) with the
     measured shape boundary. Invariant 7 forbids fixing it here, so the
     fixture takes the sound shape and points at the entry from the site.
2. **M2 (PTL)** — rebuild `brievc` release (tree binary is 2026-11-20, stale),
   run `g0.sh` with `BRIEV_PTL_SSH="ssh prestopoverlord@100.92.76.67"`; the
   `g0_gemm` rung gives device BAD=0 + full-coverage canary + `gpu_time`,
   no-silent-fallback enforced (`--deny-accel-cpu-fallback`).
3. **M3 (PTL)** — OpenVINO MatMul at **512³** (the same geometry, not the
   recorded 4096³) via `collector.bv` + `harness.py`, GPU and CPU p50. If the
   GPU plugin does not enumerate, the CPU plugin carries the vendor number and
   the GPU row is recorded *unavailable-with-why* (device execution never
   dropped silently).
4. **M4 (repo)** — results doc
   `benchmarks/results/2026-11-22-saltpetre-gemm-graduation-baseline.md` with
   the BEFORE/AFTER table (the "after" is the device/vendor numbers at the
   same geometry — Rule 12 baseline table); the `Gemm` ledger row gains its
   graduation spec; the ledger gate learns the promotion-spec form on `briev`
   rows (new named test, canary-observed failing).
5. **Land** — fetch → ancestor check → `push origin HEAD:main` → push lane.

## Invariants

1. **The CPU baseline measures the saltpetre op, not a hand-copied kernel.**
   `tests/saltpetre/gemm_baseline.bv` builds the graph with saltpetre's own types
   (`OnnxNode{op_type: "Gemm"}`) and calls `sp_run` — the dispatch path that
   reaches `exec.bv`'s `gemm`. Pinned by `salt_gemm_cpu_runs_and_checks`
   (compiled+run: prints `SALT_GEMM_BAD=0` against the CPU dot reference,
   and a `SALT_GEMM_CPU_US` line).
2. **The geometry is the g0 geometry, so the three parties are comparable.**
   M=N=K=512, f32, the closed-form input of `g0_gemm.bv`
   (`a = ((i*7)%13)*0.125 - 0.5`, `b = ((i*3)%11)*0.25 - 1.0`). Pinned by
   invariant 1's reference check (it recomputes the same closed form as a CPU
   dot over 4096 sampled outputs, exactly as g0_gemm's verify node).
3. **The device lane is proven to be the device lane.** The g0 rung's
   `BAD=0` + `CANARY_BAD=0` + asserted `compiled on 'vulkan'` line (via
   `--deny-accel-cpu-fallback`) — an existing, already-canary-observed gate;
   this session re-runs it at the current commit and records the output.
4. **No number is reported unless its correctness gate passed.** Every figure
   in the results doc cites its gate (BAD/canary for the device lane,
   spot-check sum for the CPU baseline, harness max-rel for OpenVINO). This is
   the repo's timing rule, enforced by the doc template the gate test checks
   (`results_doc_has_gated_numbers`: every ms figure in the doc sits in the
   same section as its gate line — a structural check, cheap and honest).
5. **The ledger row's graduation field is a spec, not a promise.** On a
   `briev` row, `graduation` is either `none` (no spec) or
   `promote: <shape> <dtype> @ <device> vs <vendor> <dtype> — ratio <= <N>,
   BAD=0, CANARY_BAD=0, compiled-on-vulkan;` plus an `evidence:` field naming
   the results doc. Every part is required (the shape, the device, the
   competitor, the three gate tokens, the bound, the evidence), and the gate
   recomputes the ratio from the evidence doc's two p50s rather than trusting
   either the doc's own ratio line or a bound tightened later. The Gemm row
   carries the spec. Pinned by `briev_rows_carry_promotion_specs` +
   `results_doc_has_gated_numbers` (both new).
6. **The census does not move.** P3 adds no ops, retires no backends:
   `briev=30 delegated=0 total=30` unchanged. Pinned by the existing
   `census_only_improves`.
7. **No compiler change.** No `.rs` under `src/`, no `lib/std/` edits except
   none at all; the only new sources are the benchmark fixture, the results
   doc, the ledger-row text, the gate test, and the two plan docs. Verifiable
   by `git diff --stat` at commit time.
8. **The OpenVINO competitor is at the same geometry.** 512³ MatMul, not the
   recorded 4096³. Pinned by invariant 4's doc check (the doc must state the
   geometry per figure).
9. **The canary bites.** The gate change (invariant 5) is observed failing
   before being trusted: with the Gemm row's graduation field emptied,
   `briev_rows_carry_promotion_specs` fails on the real tree; restore, passes.
   Recorded in the results doc, not just the commit message.

## Named tests

| test | pins |
|---|---|
| `salt_gemm_cpu_runs_and_checks` | invariants 1, 2 (compiled fixture: BAD=0, sum line, ms line) |
| `briev_rows_carry_promotion_specs` | invariants 5, 9 (new, in `tests/saltpetre_op_ledger.rs`) |
| `results_doc_has_gated_numbers` | invariant 4 (new, in `tests/saltpetre_op_ledger.rs`) |
| `census_only_improves` (existing) | invariant 6 |
| g0 rung `g0_gemm` BAD/CANARY/compiled-on-vulkan | invariant 3 (existing gate, re-run) |
| `gate_rejects_bad_ledgers` (existing, extended) | invariant 5 (bad promotion-spec forms) |

## Steps

- **M1**: write `tests/saltpetre/gemm_baseline.bv`; `brievc run` locally
  (release build first); record `SALT_GEMM_CPU_US` (printed by the fixture).
  **DONE**: 512³ f32 → `SALT_GEMM_BAD=0`, `SALT_GEMM_SUM=131033.188`,
  `SALT_GEMM_CPU_US=319901` (319.9 ms, single-threaded scalar triple loop,
  ≈0.84 GFLOP/s), gated by `tests/saltpetre_gemm_baseline.rs`.
  A second deviation was forced later by CI: the gate memoised the build in a
  `OnceLock`, which is per *process*, and the project's gate runner is
  cargo-nextest (one process per test) — so the file's two tests raced on one
  `--out` directory and the linker lost it (`No such file or directory`, run
  45050623812). The gate now takes a cross-process claim (atomic `create_dir`,
  released on drop, 300 s deadline) and reuses the produced binary when it is
  newer than the `.bv`: 14.6 s then 0.3 s, 2/2 under nextest.
- **M2**: run the g0 rungs on PTL. **DONE**, with one environment deviation
  and one harness fix, both recorded:
  - **Deviation — the node's root filesystem is full.** PTL `/` sits at
    442 G/444 G used (64 K free) and the compiler's FFI object cache lives at
    `dirs::cache_dir()/briev-compiler/ffi` (`src/compile.rs:3291`), i.e.
    `$HOME/.cache` — `BRIEV_HOME` does **not** redirect it. The first run died
    right after `== sync` with `unable to open output file
    '/home/prestopoverlord/.cache/briev-compiler/ffi/763aaa….o': 'No space
    left on device'`. Fixed at the harness level: `g0.sh` and `run.sh` now take
    `BRIEV_PTL_ENV` (an env prefix applied to every remote shell, **after** the
    `cd` — before it, an assignment scopes only to the `cd` builtin) and the
    runs use `XDG_CACHE_HOME=/tmp/briev-cache` on the node's 31 G tmpfs. The
    disk itself is the owner's data (289 G of `~/Projects`), not build output,
    so this is a bypass, not a repair.
  - **Measured, `g0_gemm` 512³ f32, six runs**: `G0_GEMM_BAD=0`,
    `GPU_SUM=CPU_SUM=131033.188`, `CANARY_BAD=0`, `compiled on 'vulkan'` every
    time; `# gpu_time:` **0.367 / 0.235 / 0.345 / 0.346 / 0.370 / 0.371 ms**
    (median 0.356) — a range, because that is what six identical runs give.
    `GPU_SUM` equals the saltpetre fixture's `SALT_GEMM_SUM`, so the CPU
    baseline and the device lane are provably on the same inputs.
  - **Measured, `g0_elem`**: `selfadd compiled on 'vulkan'`, `G0_ELEM_BAD=0`,
    `GPU_SUM=CPU_SUM=261566496`, `gpu_time` 0.038 / 0.035 ms.
  - **Measured, `g1_lane_smoke` on levelzero**: `scale compiled on
    'levelzero'`, `staging 2120 bytes (host USM) n=256`, `dst[0] = i64 1 |
    dst[255] = i64 511`, `gpu_time` 0.008 / 0.003 / 0.006 ms; the opencl lane
    compiles the same kernel (`enqueue global=256 local=256 n=256`).
  - **Full-wrapper verdict: `G0: PASS (vulkan elementwise + 512^3 GEMM vs host
    reference)`** — vulkan and levelzero green on all six rungs, opencl green
    on all five elementwise rungs, `g2_gemm_tiled` green at 0.356–0.370 ms.
    This required a second harness fix: the reported-lane branch classified a
    run with **no** `_BAD` line as `WRONG`, so the earlier wrapper run reported
    `G0: FAIL` on `g0_fsum_approx`/opencl when in fact *nothing had been
    measured* (a hang can happen after the kernel compiled — NEO's `clFinish`
    class, BUGS.md). The branch now says `no verdict (compiled=N,
    timeout/hang) — reported` and only calls a lane wrong when a `_BAD` line
    exists and disagrees. That earlier `G0: FAIL` is historical; the PASS above
    is the current state.
  - **Recipe**: the wrapper is read with `2>&1 | grep -E "^(==|--|G0:)"` —
    every banner, every lane verdict, and the aggregate line, no truncation.
    Announced caveat carried into M4: `brievc run` does not execute the
    compiled `_runner.c` (see `2026-11-17-ptl-f16-tier.md`), so this is a
    device-lane rung value, never a shipped-path ceiling.
- **M3**: OpenVINO MatMul at **512³** on PTL (the same geometry, not the
  recorded 4096³) via `harness.py`. **DONE**, with one harness deviation:
  `harness.py` was parameterised (`--elem-n` / `--gemm-n` / `--gemm-dtype`, and
  per-bench input/reference construction) because `run.sh` passes only
  `--benches` and the geometry lived in module constants. Harness edits are not
  compiler edits.
  - **f32**: `[gemm] GPU: PASS p50=480854ns err=0.000281, CPU: PASS
    p50=602712ns err=9.02e-07` → GPU **0.481 ms** (min 0.417, p90 0.547, 20
    iters), CPU **0.603 ms**.
  - **f16** (vendor's faster lane, *not* a like-for-like): GPU
    **0.231897 ms** (min 0.226), CPU **0.644856 ms**.
  - Raw output committed verbatim beside the results doc as
    `benchmarks/results/2026-11-22-ptl-ov-matmul512-f32.json` and
    `…-f16.json` (that directory also holds raw JSON from `2026-11-06`; the
    convention is `.md` for the narrative, `.json` for the emitted evidence).
- **M4**: **DONE**. Results doc
  `benchmarks/results/2026-11-22-saltpetre-gemm-graduation-baseline.md` carries
  the same-geometry table with each figure's gate; the `Gemm` row's graduation
  is the `promote:` spec; `tests/saltpetre_op_ledger.rs` gained
  `briev_rows_carry_promotion_specs`, `results_doc_has_gated_numbers` and
  `gate_rejects_bad_promotions`. The gate was **observed failing twice on the
  live tree** before being trusted: (a) Gemm's graduation reset to `none` →
  `no \`promote:\` row in lib/saltpetre/op-backends.dbvl`; (b) the doc's
  `briev_gpu_p50_us` edited to 200.0 → `ratio_vs_openvino=0.74 does not follow
  from 200/480.9 = 0.42`. Restored, 12/12 green. Measured ratio 0.74× against
  a stated bound of 2.0×; the bound is looser than the measurement on purpose
  because the two numbers are different units (kernel time vs a whole
  compiled-model call), which the results doc states outright.
- **Land**: fetch → ancestor check → push main + lane.

## Docs updated (Rule 13)

- `benchmarks/results/2026-11-22-saltpetre-gemm-graduation-baseline.md` (new).
- `lib/saltpetre/op-backends.dbvl` — Gemm row's graduation field (one line).
- `docs/plans/INDEX.md` — P3 line if it exists there; otherwise the results
  doc is the record (timestamped, never retro-edited afterwards).

## Out of scope (named)

- P4: saltpetre's device execution surface (declared `accel` kernels inside a
  `.bv` module). The plan it needs does not exist yet.
- Any compiler change (invariant 7).
- Retiring the `gemm` arm of `run_node` (nothing to retire — the row stays
  `briev`).
- Autograd. cuBLAS/cuDNN/oneDNN bindings (P2 of the ledger plan, untouched).
