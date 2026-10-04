# Shallow-K tip re-verification + GPU bug-ledger truth pass (2026-10-03)

Adopted scope: "Adopt plan as written". Phases 0–3. Execution status
inlined below — Phases 0/1 partially DONE (read-only evidence), edits
pending permission lift.

## Finding that reshaped candidate A

The shallow-K emitter race (BUGS.md ≈6568, INDEX "OPEN correctness") was
**fixed 2026-09-16, same day the [OPEN] header was written and never
flipped**:

- Fix: `src/backend/ptx/tensor.rs:1762` —
  `wait_depth = if k <= 128 { 0 } else { stages.saturating_sub(2) }`
  (full `cp.async.wait_group 0` drain at shallow K; root cause was
  `stages-2` leaving a stage in flight for ring reuse). Provenance
  comment inline; measurements recorded (7.13 TF @1024²×64, deep-K
  overlap preserved).
- Gate removed: zero refs to `shallow_k_race`/`mw_kernel_ok` in src/
  and benchmarks/.
- Stale docs: BUGS.md:6568 header `[OPEN]`; INDEX lines ~141 + ~213
  describe the removed gate as live; INDEX div-slowpath items
  (lines ~104, ~239) stale (closed 2026-10-02, misattribution <0.5%).

## Phase 0 — dispatch ground truth — DONE (read-only)

- `tensor = a_elem == 2 && M%32==0 && N%16==0 && K%16==0`
  (`src/backend/ptx/mod.rs:1120`): the PTX mw tensor tier is **f16
  only**; f32 GEMMs get `naive_gemm_ptx` (flat, race-free). The race
  lived in the **f16** mw kernel — the L4 repro's "f32 ref" was the
  reference computation, not the operand. Retargeted Phase 1 to f16.
- Built `gemm_h` 1024²×64 at tip: emitted PTX = 562 lines, 16 mma.sync,
  8 ldmatrix, `cp.async.wait_group 0` ×2 — **the fix is present at tip**.

## Phase 1 — tip re-verification — IN PROGRESS

Fixture set: `examples/gpu/.tmp_shallow/gemm_h_<MxNxK>.abv` (gemm_h.abv
with consts M/N/K sed'd; MN = M * N) at the claim matrix:
512²×64, 512²×128, 1024²×64, 1024²×128, 2048²×128, 4096²×64, 4096²×128.

- **All-ones mode: 21/21 PASS** — every shape × 3 runs × both lanes,
  EXACT over every element (`benchmarks/declared_matmul_gate.sh … f16`).
- **Caveat (why this is NOT sufficient):** all-ones is blind to a
  stale-fill race — every ring stage holds identical ones, so reading
  the wrong stage still sums to K. The original bug was proven with
  PATTERNED data (max_rel 5e-2). The gate's patterned mode is
  f32-only today; f16 fixtures run mode 0 only.
- **Remaining Phase 1 work — extend the gate with f16 patterned mode**
  (mirrors `benchmarks/gpu/gemm_h_bench.c`):
  1. Injected C gains `f32_to_f16` (copy of gemm_h_bench's) +
     `f16_to_f64`.
  2. `gate_seed` f16 branch: mode 1 → periodic exact seeds
     `a[i] = f32_to_f16((i%7)*0.25f)`, `b[i] = f32_to_f16((i%5)*0.5f)`
     (periods 7/5 differ across ring stages ⇒ stale fills detectable).
  3. `gate_verify`: f16 + mode 1 → 16 spread rows `(r*7919)%M`, f64
     reference from the f16-exact seeds, rel gate 5e-3 (1e-2 under
     `BRIEV_GEMM_F16ACC`; `ptx_tensor_f16acc` default false —
     src/config_tuning.rs:399).
  4. Shell: `modes="0 1"` for f16 as well as f32.
  5. Re-run all 7 shapes × 3 runs × both lanes, both modes. Record in
     `benchmarks/results/2026-10-03-shallow-k-tip-verification.md`
     (claim, tip SHA, per-shape verdicts, run counts, the all-ones
     blindness lesson).
- **Branch:** any patterned FAIL ⇒ genuine race regression → isolate
  with the existing `ws_debug` position-encoded instrument
  (`src/backend/ptx/tensor.rs:1985`, env-gated) and fix before docs.
- Timing (optional): one 1024²×64 point vs the 7.13 TF claim only if
  the existing rig is cheap; correctness is the gate.

## Phase 2 — ledger truth pass — PENDING

BUGS.md (status flips as dated addenda; bodies preserved; no
retroactive history edits):

1. :6568 header `[OPEN]` → `[RESOLVED 2026-09-16]` + addendum linking
   the new results file (tip re-verification).
2. :6651 header `[OPEN]` → `[RESOLVED — FALSE ALARM]` (body already
   says it).
3. :7687 `[OPEN]` dot! hunt → mark SUPERSEDED by the CLOSED entry at
   :7627 (keep the hunt record).
4. :7662 — exact duplicate of the f16 entry at :7602 (double-append) →
   remove the dup, note the dedup in the surviving entry.
5. :6707 prime full-upload `[OPEN]` — mitigation evidence:
   `02418f49` + `src/accel_rt.rs:854` snapshot/restore; compositions
   green in every gate since (5a decode, m3 harness both lanes). Do:
   run the repro (attention_decode.abv 3-kernel composition, both
   lanes) → if PASS, dated RESOLVED addendum; if FAIL, keep OPEN and
   file as its own follow-up (do not close without the repro).
6. :6740 CUDA 13.4 cuMemcpy2D — external driver, leave OPEN.

INDEX.md:

- Two shallow-K lines (~141 M4 paragraph, ~213 open-bugs bullet):
  remove/mark resolved with pointer to the results file.
- Two div-slowpath lines (~104 queue, ~239 starting-points): closed
  2026-10-02 (misattribution, tail-resident <0.5%).
- Open-bugs list otherwise aligned with BUGS.md truth.

## Phase 3 — handoff — PENDING

- INDEX recommended starting points → the GEMM fill-pipeline campaign
  (`2026-09-30-stage5b-structural-fill-campaign.md`, queue item 3,
  32→42 TF) becomes next.
- `bad<ptx>` payload ABI stays parked (only needed if a future
  row-form gap appears; measured emitter numbers on record).

## Per-commit checklist when edits resume

- `cargo test --lib` green (2855 baseline).
- `cargo build` no new warnings.
- Praetor on changed dirs (`--target` = DIRECTORY).
- Doc changes only expected in Phases 2–3; Phase 1 gate edit is
  `benchmarks/` (shell/C-in-string — no Rust, no Praetor).
- Commit gates: only after the patterned f16 matrix is green (or a
  genuine regression is filed).

## Cleanup owed

- `examples/gpu/.tmp_shallow/` (temp fixtures) — keep until Phase 1
  finishes, then remove (never commit).
