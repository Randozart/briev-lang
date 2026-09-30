# Stage 5b — structural fill-path campaign (2026-09-30)

Follow-on to `docs/plans/2026-09-30-stage5b-cuda-gemm-s5.md` (Phase 3
verdict: the smem fill round-trip is the wall) and the measured record
`benchmarks/results/2026-09-30-5b-cuda-s5-ladder.md` (ship 27.1 TF;
no-fill A+B 45.4; no-A 38.5; no-B 38.9 @4096³, driver 615).

**Method (this campaign):** every candidate is a default-off
knob/mode — byte-identical when off. Each is measured before/after on
the same on-device rig, same shape matrix, interleaved A/B, with the
all-ones + f64 correctness gate first. Winners are wired into the
dispatch; losers are recorded as REJECTED and block their follow-ons
(Rule 20: negatives stand).

## Reach (where the fill round-trip lives)

| Surface | Emitter | Staged fills | In scope |
|---|---|---|---|
| PTX tensor GEMM (mw) | `tensor.rs::tensor_gemm_ptx_smem_mw*` | yes (cp.async A+B, bar.sync) | **primary** |
| PTX tensor GEMM (single-warp fallback) | `tensor.rs::tensor_gemm_ptx_smem` | yes (smaller) | yes |
| PTX naive GEMM (f32 / non-tileable) | `mod.rs::naive_gemm_ptx` | no (direct global) | not affected |
| PTX general reducers | `general.rs` `wpart[]` (L1622), `redm/redl/smacc` (L3243) | yes (merge bar) | Phase 4, measure |
| Vulkan/SPIR-V coopmat | `spirv/gemm.rs`, `kernel.rs` | yes (workgroup smem) | Phase 4, separate emitter |
| `fused_attention_*` | `mod.rs` | partial (Q direct, Kt smem) | gated OFF |
| General accel (nbody…) | `general.rs` elementwise | mostly direct | not affected |

Two structural facts:
- `gpu_strategy::select` is consumed **only** at `src/backend/ptx/mod.rs:2006`
  — the SPIR-V lane does not use the cost model.
- `Strategy::staged` (`gpu_strategy.rs:60`) is computed but **read by no
  emitter** — there is no register/direct load path; every tileable f16
  GEMM stages through smem. Wiring a real load path is the Phase-3 fix.

## Universality / gating (why it is NOT universal)

The fill cost is intrinsic (580: 46.2→35.5 = 23%; 615: 45.4→27.1 = 40%),
so the direction is general — but the mechanisms differ:

- **De-burst / schedule** (low register cost): safe to apply broadly
  within the staged path; magnitude unproven.
- **Register-staging one operand** (E1f ceiling 42.5): raises register
  pressure → risks the **4-CTA/SM sweet spot** (the E-series governing
  law; E8a lost 34% at 320T → 3 CTAs). Must be gated per shape × tile ×
  device. Which operand is tile-dependent (A is shared across nw warps →
  register-staging A duplicates global reads ×nw; B is per-warp).

| Case | Applies? | Note |
|---|---|---|
| large compute-bound square (2048³/4096³) | yes, target | fill burst dominates |
| attention composite QK/PV | likely | same staging pattern |
| small 64³–256³ | **gate off** | already 1.3–3.2× vs cuBLAS — canary |
| thin-K / decode (K=64–128) | separate (L4) | fill amortization differs |
| naive / f32 / non-tileable | n/a | no smem fills |
| Vulkan coopmat | Phase 4 | separate emitter; model not consumed |

The end state (only if a path wins): a real **load-path axis** in the cost
model replacing the dead `staged` flag, calibrated per device profile
(`GpuHardware`; its honest home is a future `config/gpu-targets.dbvl`).

## Shape matrix (every experiment)

`64³, 128³, 256³, 512³, 1024³, 2048³, 4096³, 8192³` plus thin-K
`4096×4096×64 / ×128` and rectangular `4096×512×512`. Canaries =
64³–256³ (regression must stay ≤1%).

## Phases

**Phase 1 — schedule/de-burst — REJECTED (2026-09-30).** Evidence in
`benchmarks/results/2026-09-30-5b-structural-fill.md`: `kps=2` neutral;
mode 4 (fills ON, commit/wait/membar/bar suppressed) = 23.6 TF, *below*
ship. The cost is the fill work itself, not the schedule. No schedule
follow-ons.

**Phase 2 — register-staging — REFUTED (2026-09-30).** Scaling the
ship-vs-no-fill gap across shapes
(`benchmarks/results/2026-09-30-5b-structural-fill.md`) shows it appears
only when operands exceed L2, and the kernel sits at the ~360 GB/s DRAM
limit: the "fill cost" is **operand re-read DRAM traffic**, and the
no-fill ceiling is unreachable (it removes necessary reads).
Register-staging would ADD redundant reads (A shared ×nw, B ×mw) —
bandwidth-doomed; the E-series E1f regime does not apply here.

**Phase 2' — reuse / L2 locality (the correct lever).**
- Larger CTA tile → fewer re-reads; the E-series rejected big tiles under
  a *compute-bound* assumption — re-measure under memory-bound reality.
- **L2-friendly CTA rasterization**: swizzle the 1D `ctaid.x`→tile decode
  so concurrent CTAs share a B slab in L2 (cheap; no kernel-body change).
- Small-K/decode: the L4 family (unchanged).
Each behind a knob, measured before/after, canaries (64³–256³) gated,
recorded in the same results file.

**Phase 2' results (2026-09-30):** column-major rasterization REJECTED
(2.4× worse). **Forced CTA tile is a WIN:** (2,8)@512T = tile 128×256
gives **+16% @4096³, +24% @8192³** (deep-K, operands > L2), all-ones
PASS; thin-K `4096×4096×512` regresses (not DRAM-bound) → gate by shape.
The model (`estimate_time`) under-rates this: it divides `memory_s` by
`stages` and applies the occupancy penalty unconditionally, so it picks
(2,4). **Phase 3 fix (own step, model calibration + tests):** model the
DRAM-bandwidth floor `max(compute, bytes/BW)` and apply the occupancy
penalty only when compute-bound, so the wider tile is chosen
automatically for DRAM-bound shapes (Golden Rule 2 — no keyword needed).

**Phase 3 caveat (must not regress thin-K).** A naive roofline swap is
NOT sufficient: for thin-K `4096×4096×512` the model would then still
prefer the wide tile (less modelled memory), but the measurement shows
the wide tile is *worse* there (15–20 vs 25.7 TF) — thin-K is short on
ksteps (32), so the wider tile's occupancy/prologue cost dominates and
the DRAM saving is small. The fix therefore needs an explicit short-K /
under-fill term, not just `max(compute, memory)`. Validate the model
against the full measured matrix (64³…8192³, thin-K, K=1024) before it
drives dispatch; the existing calibration tests (`e4c_tile_preserved_at_4096`
etc.) encode the old compute-bound answer and must be updated with the new
evidence.


**Phase 3 — model gating (winners only).**
- Load-path axis in `gpu_strategy` + per-device calibration parameter;
  model selects smem vs register-staged per shape × device. No
  unconditional application.

**Phase 4 — adjacent surfaces (own experiments, not assumed).**
- PTX general reducers; Vulkan/SPIR-V coopmat (proven-regressed lane,
  separate emitter). Each measured before/after, recorded.

## Recording & gates

- Results: this campaign's file
  `benchmarks/results/2026-09-30-5b-structural-fill.md` (new), per-shape
  before/after, explicit REJECTED rows.
- Per commit: `cargo test --lib` green, no new warnings, Praetor
  baseline-diff (note: `tensor_gemm_ptx_smem_mw_opt` is a pre-existing
  281-complexity row — changes add small deltas on the existing row, not
  new diagnostics).
- Suite regression guard: B1 `--runtime` table after any default change.
- Correctness gate before every timing: all-ones exact + f64 ref ≤6%,
  both lanes, at a real shape (kernel-index rule).

## Assumptions carried (adjust if wrong)

1. Sequencing: Phase 1 first, Phase 2 after (or in parallel if Phase 1
   is a quick REJECTED).
2. Vulkan lane: Phase 4 (deferred).
3. `GpuHardware` move to `gpu-targets.dbvl`: deferred until a path wins.
4. Keep bar: ≥3% at 2048³/4096³, no canary regression >1%.
