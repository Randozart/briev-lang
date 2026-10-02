# Plan: Stage-5 re-rank + 5c warp-slice threshold retirement (2026-09-30)

**Date:** 2026-09-30
**Status:** ACTIVE — B4 of `2026-09-28-daily-use-sweep-and-gpu-session.md`,
stage 5 of `2026-09-24-followup-stages.md`.
**Inputs:** B1 record `benchmarks/results/2026-09-30-b1-rebaseline.md`,
GEMM gates `benchmarks/results/2026-09-30-gemm-4096-gates.md`,
Front D verdict `benchmarks/results/2026-09-25-front-d-ab.md`.

## Re-rank (from fresh numbers)

| Rank | Lane | Why |
|------|------|-----|
| 1 | **5c — warp-slice thresholds → config** (chosen) | Environment-independent (no GPU timing needed), mechanical, doctrine (Rule 2 / proof-vs-shape: tuning heuristics leave the backend). B1 gave no reason to reorder vs the umbrella's 3b-first order. |
| 2 | **5b — S3b cp.async GEMM** | Biggest absolute prize, but gates measured while vendor driver 615 regression bounds the smem+barrier fill path (gates record). Relative same-driver A/B stays valid; absolute 38+ TF claims stay blocked on the vendor. Runs after 5c. |
| 3 | **5a — attention 198→125 µs + dead-family deletion** — **DONE 2026-10-01** | Composite + fused online: **72.5 µs p50** (target 125 BEATEN; ggml ~58 now 1.25× away). Dead-family deletion DONE (`a7871a27`, ~1300 lines; Rule 24 gate met). Remaining polish: float4 k/v loads, div slowpath. |
| 4 | **5d — M4 ladder + M3 chain fusion** | Depends on 5a first (umbrella) and M3 not started. |

Rule 12b pre-B check: no refuted hypothesis applies to 5c (it is
behavior-preserving at defaults, not a perf hypothesis); the baseline
to hold is *byte-identical IR at default config*, captured below.

## 5c — design

**Findings from investigation (2026-09-30):**

- `has_warp_slice` (`src/backend/ptx/general.rs:319`) hardcodes the two
  tunables: `span < 512` and `span % 4 != 0`. The same literals are
  DUPLICATED in the emission gate (`general.rs:1114-1116`) — two sources
  of truth for one decision (DRY, Rule 17).
- The `%4` is not an independent knob: 4 warps × 32 lanes = the
  128-thread block baked into `emit_warp_sliced` (`quarter = span/4`,
  `wpart[16]`, merge `1..4`), `general.rs:944` (block_threads 128) and
  `mod.rs:1806` (dispatch desc 128). A free-standing divisor config
  without deriving that geometry would desync detector from emitter
  (e.g. divisor 2 with quarter=span/4 drops iterations — miscompile).
  Therefore the config value is the WARP COUNT and every geometry site
  derives from it.
- Hardware facts stay in the backend, per umbrella: warp = 32
  (`shr.u32 … 5`), f32 slot = 4 B (smem addressing `mad.wide … 4`,
  loads `+ k*4`), `bar.sync`.
- The `ptx_warp_slice` ON/OFF knob (`config/ir-lowering.dbvl:106`) is
  consumed only by the emission gate (`general.rs:1114`); the geometry
  detector arms (`general.rs:927/944`, `mod.rs:1793`) are knob-free by
  design (block guard is coherent with either emission branch —
  verified: serial fallback under block guard is benign uniform
  redundancy, the known bug-14 shape). **That split is preserved,
  unchanged.**

**Changes (additive; defaults reproduce today's numbers exactly):**

1. `config/ir-lowering.dbvl` — two new tunables with rationale comments:
   - `ptx_warp_slice_min_span: 512;` — smallest reduction span that
     splits (tuning: MLP knee at M1, plan `2026-09-19-general-machinery`).
   - `ptx_warp_slice_warps: 4;` — warps per block for the slice
     (geometry: derived everywhere as `warps * 32` threads).
2. `src/config_tuning.rs` — `IrLoweringSettings` fields + defaults
   (512 / 4) + `load_ir_lowering` parse with clamps (`min_span` 0..=65536,
   `warps` 1..=8 — smem partials ≤ 32 B, block ≤ 256 threads).
3. `src/backend/ptx/general.rs`:
   - central predicate helper `warp_slice_span_ok(span, min_span, warps)`
     (single source for detector + emitter — kills the duplicated
     literals);
   - `has_warp_slice` reads `config_tuning::ir_lowering()` and uses the
     helper; `unroll: 4` match factor and body-legality gate unchanged;
   - emission gate (`:1114-1116`) uses the same helper;
   - `emit_warp_sliced` derives `quarter = span / warps`,
     `wpart[warps * 4]`, merge `1..warps` (4 B slot arithmetic untouched);
   - `block_threads` arm (`:944`) derives `warps * 32`.
4. `src/backend/ptx/mod.rs:1806` — dispatch `block_threads` derives
   `warps * 32` via the same config read (keeps desc ↔ emission in
   lockstep; `maxnreg = block_threads` heuristic keeps its relationship).
5. Out of scope, recorded: `ptx_warp_slice` default stays `0`
   (ships disabled — `2026-09-19-general-machinery.md:64`); SPIR-V lane
   has no warp-slice counterpart (verified `has_warp_slice` is PTX-only).

**Tests (Rule 9):**

- Existing: `warp-sliced` + `serial_unroll_fires_when_the_slice_does_not_apply`
  (`general.rs:2490-2550`) must pass with their ASSERTIONS UNCHANGED —
  they pin default behavior at both sides of the 512 threshold. (Call
  arity changed with the plan struct `WarpSliceEmit`; assertion
  values — `wpart[16]`, quarter 1024, merge `1..4` — were NOT edited.)
- New unit: min_span override flips `has_warp_slice` (span 256 true at
  min_span 64, false at default 512); warps=2 emission → `wpart[8]`,
  quarter `span/2`, merge `1..2`; warps clamp edges (1, 8); detector and
  emitter agree on the SAME config (helper shared — asserted via both
  paths at one non-default config).
- New dispatch: mod.rs desc block_threads follows config (unit at the
  desc-construction level).
- Config parse: defaults when fields absent; clamps honored.

**Pre-B IR capture (Rule 12b):** before editing, record the emitted PTX
assertion set for the `pv_shape` fixture (NKV=4096 slice arm with
`ptx_warp_slice: 1`, NKV=256 fallback arm) — the existing test bodies
ARE the capture (they pin `wpart[16]`, quarter 1024, 4 partial reads,
8 loads at 4× unroll). Post-change run must pass them under default
config with zero assertion edits (behavioral gate — only the call
shape follows the plan struct).

**Gates:** `cargo test --lib` green; `cargo build` no new warnings;
Praetor `--target src/backend/ptx` + `--target src` (config_tuning) —
no NEW diagnostics; docs: this plan + `config/ir-lowering.dbvl`
rationale comments + `docs/architecture/agent-reference.md` tunables
mention if present (grep before editing); no timing A/B needed
(defaults byte-identical — if ANY existing gate fails, the change is
wrong, not the gate).

**Documentation (Rule 13):** this plan; stage-5 entry in
`2026-09-24-followup-stages.md` points here (same commit);
`2026-09-19-general-machinery.md` is a timestamped record — not
edited; BUGS.md only if a defect surfaces.

**Commit strategy:** one logical commit (config + tuning + backend +
tests + docs together — they are one behavior-preserving unit).
Determinism defects the 5c pre-B gate uncovered are committed FIRST
as their own commit (they are independent §4 fixes — see BUGS.md).

## Gate results (2026-09-30, post-change)

- `cargo test --lib`: **2777 passed** (2771 baseline + 5c tests +
  2 determinism tests). `cargo build`: no new warnings (the 14
  lib warnings are the untouched baseline; `main.rs:747` pre-existing).
- Praetor baseline diff (HEAD vs working, all 7 changed files): no NEW
  diagnostics; `emit_warp_sliced` rule-4 (7 params) + function-complexity
  rows REMOVED; `emit_stmt` complexity rows improved 26→24 / 18→17.
- Full-corpus IR A/B (Rule 12b): every `benchmarks/*.bv` compiled with
  the pre-5c worktree (`ccfbadd9`) vs post-5c — **55 byte-identical**,
  4 build-fail on both sides (`meld-bridge`, `nbody_newton_soa`,
  `popcount_derive` ×2 — no source), 2 diffs = OLD-side hash
  nondeterminism (fixed first, see below).
- Determinism sweep: post-5c compiler built twice over all 61 sources →
  **57/57 `.ll` byte-identical** (0 unstable).
- Defaults byte-identical end-to-end: `gemm_h.abv` pre-5c vs post-5c →
  `gemm_h.spv` AND `gemm_h_runner.c` **byte-identical** (knob ships
  OFF; shared predicate evaluates the same literals).
- On-device gate (kernel-index rule): `gemm_h` 4096³ all-ones,
  both lanes — Vulkan **PASS** and CUDA **PASS** (`maxrel=0`,
  4096/4096 cells, corners + center exact). The gated harness
  (`/tmp/opencode/gemm_h/harness2.c`) is the generated runner with a
  gate `main` spliced in: its pre-`main` content is byte-identical to
  the post-5c runner, so its results are this build's results.
- **Determinism defects found by this gate (fixed before 5c, own
  commit, BUGS.md):** `async_txn_names` HashSet order (async-body call
  order in main), `counter_ge_bounds` hash order (synthetic exit AND
  order), `field_index_map` hash order (`sa*` alloca types). The
  long-standing `async_counters_idio` 10-vs-1 harness MISMATCH
  (recorded since 2026-07-19) root-caused to its C companion modeling
  a converged state that ignored the source's observable prints —
  companion rewritten as a semantic mirror; harness now MATCH.
