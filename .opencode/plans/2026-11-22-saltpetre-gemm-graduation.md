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
   `briev` row, `graduation` is either `none` (no spec) or a line of the form
   `promote: <op> <geometry> <device> <acceptance>` with all four parts
   non-empty. The Gemm row carries the spec. Pinned by
   `briev_rows_carry_promotion_specs` (new) + the observed canary: a `Gemm`
   row with `graduation: none` while the results doc names a promotion event
   for Gemm fails.
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
- **M2**: `cargo build --release` (stale binary is 2026-11-20); run `g0.sh`
  gemm rung on PTL; capture BAD/CANARY/gpu_time.
- **M3**: on PTL, `harness.py --bench gemm512 --device GPU,CPU` (512³ variant of
  the harness geometry, generated IR + raw blobs, `collector.bv` timing);
  capture p50.
- **M4**: results doc; ledger row `Gemm` graduation = `promote: Gemm
  512^3 PTL-Xe3-vulkan BAD=0+CANARY_BAD=0+compiled-on-vulkan within N× of
  OpenVINO@512^3`; gate test + canary observation; `gate.sh quick`.
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
