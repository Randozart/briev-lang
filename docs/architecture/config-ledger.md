# Config ledger — epistemic status of every tuning knob

Every key in `config/ir-lowering.dbvl` (global IR-lowering tuning) and
`config/targets.dbvl` (per-target facts) classified: **derived** (computed
from a protocol or a machine fact), **measured** (a dated A/B or sweep —
pointer given), **policy** (a declared intent, not an optimization claim),
**provisional** (historical default, no derivation on file — derivation
queued to the followup-stages umbrella). Provisional is not shameful; it is
an honest TODO. Sweep-derive suspects before trusting them in a release.

Maintained by the stability-remediation plan S6
(`docs/plans/2026-10-16-stability-remediation.md`); update the row when a
knob's status changes. The GPU-family rows carry their own A/B history in
the plan docs — pointers given, not repeated.

## config/targets.dbvl (per-target facts)

| Key | Status | Notes |
|---|---|---|
| `float_registers` | derived | target register file (x86_64 16 GPR, aarch64 32, wasm flat) — machine fact |
| `dense_compute_density` | measured | `#11 → #0` downgrade threshold; A/B'd in the frontend-dispatch plan |
| `vector_min_width` | policy | vector-phi promotion width floor; a declared intent |
| `isr_mechanism` | derived | names the target's ISR row in `config/isr-targets.dbvl` — machine fact |
| `frame_limit_bytes` | derived | per-function frame ceiling — machine fact (displacement cliffs / RAM stack); enforced by the backend check (commit `63107709`) |

## config/ir-lowering.dbvl (global lowering)

| Key | Status | Notes |
|---|---|---|
| `arena_min_budget` | provisional | below-budget arena skip; no derivation on file |
| `arena_initial_size` | provisional | initial bump-arena bytes; no derivation on file |
| `stack_threshold` | provisional | transient-collection stack cutoff; no derivation on file |
| `max_fields_per_alloca` | derived | SROA decomposition friendliness — bounded by LLVM's practical field cap; rationale comment in-tree |
| `sso_max_bytes` | derived | SSO inline payload = alignment − tag bits; computed, not tuned |
| `callable_inline_weight_threshold` | measured | weighted auto-inline for callable txns; exercised across the benchmark suite since the frontend-dispatch plan (default 40) |
| `aggregate_zero_init_memset_bytes` | measured | 4096: aggregate-store pathology bound (LLVM 17 X86-IS crash, `probe_bigarray`); commit `a42de3c0` |
| `node_inline_weight_threshold` | measured | 600 = lowest-known-crash 1460 ÷ 2.4 safety (LLVM 17 defect ledger, BUGS.md); bisection + `benchmarks/ra_knee_sweep.sh`, results file 2026-10-16; retires with the defect |
| `accel_probe_k` | measured | GPU auto-tuning probe runs/lane (accel plan Phase 7) |
| `accel_probe_tolerance` | measured | probe output-equality tolerance (accel plan) |
| `accel_probe_margin` | measured | GPU-must-beat-CPU commit margin (accel plan) |
| `spirv_unroll` | measured | SPIR-V kernel unroll factor (VITRIOL GEMM O1) |
| `gpu_schedule_buffer_reuse` | measured | OFF by default — measured unsafe until per-kernel field packing lands (plan 2026-09-14) |
| `spirv_row_cooperative` | measured | OFF — wrong rows on device; re-enable after root-cause (plan 2026-09-01) |
| `spirv_coopmat` | measured | tensor-core fragments; build-time device capability (plan 2026-09-01 M2) |
| `spirv_coopmat_tile_rows` | measured | R=4 default after clock-locked A/B (plan 2026-09-02 run; 14.3 TF) |
| `spirv_coopmat_stages` | measured | software-pipeline stages (GEMM campaign) |
| `spirv_coopmat_subgroups` | measured | coopmat family — per-knob A/B in the GEMM plans |
| `spirv_coopmat_smem` | measured | coopmat family (shared-memory fragments) |
| `spirv_coopmat_fill_pairs` | measured | coopmat fill family |
| `spirv_coopmat_fill_quad` | measured | coopmat fill family |
| `spirv_coopmat_fill_prefetch` | measured | coopmat fill family |
| `spirv_coopmat_stagger` | measured | coopmat pipeline family |
| `spirv_coopmat_panels_per_stage` | measured | coopmat pipeline family |
| `spirv_image_storage` | measured | image storage plans (plan 2026-09-02) |
| `spirv_coopmat_f16acc` | measured | fp16 accumulate (plan 2026-09-01 M2) |
| `ptx_tensor_f16acc` | measured | PTX tensor fp16 accumulate |
| `ptx_tensor_b_lookahead` | measured | PTX tensor pipeline family (beyond-coopmat campaign, per-knob A/B) |
| `ptx_tensor_bsmem_pad` | measured | PTX tensor pipeline family |
| `ptx_tensor_ksteps_per_stage` | measured | PTX tensor pipeline family |
| `ptx_tensor_stages` | measured | PTX tensor pipeline family |
| `ptx_tensor_nofill` | measured | PTX tensor pipeline family |
| `ptx_deferred_skip_pass` / `ptx_deferred_online` | measured | deferred-normalizer knobs; kernel-selecting knobs A/B'd via the variant-diff gate |
| `ptx_gemm_grid_order` | measured | GEMM grid/warp specialization (followup-stages GPU re-rank) |
| `ptx_tensor_force_mw` | measured | GEMM grid/warp specialization |
| `ptx_tensor_force_nw` | measured | GEMM grid/warp specialization |
| `ptx_tensor_warp_spec` | measured | GEMM grid/warp specialization |
| `ptx_serial_unroll` | measured | warp-slicing family (followup-stages) |
| `ptx_warp_slice` | measured | warp-slicing family |
| `ptx_warp_slice_min_span` | measured | warp-slicing family |
| `ptx_warp_slice_warps` | measured | warp-slicing family |
| `ptx_deferred_region` / `ptx_deferred_split` | measured | deferred-region shaping |
| `ptx_plan_lowering` / `spirv_plan_lowering` | measured | plan-driven lowering toggles |
| `ptx_emit_cubin` | policy | emit cubin vs PTX — a deployment choice |
| `circt.firmem_min_depth` / `circt.firmem_max_ports` / `circt.clock_hz` | policy | FIRRTL memory/board facts — declared per design, not tuned |

## Status discipline

- **derived** rows cite the fact in-tree (comment or enforcing code).
- **measured** rows cite the plan/results doc with the A/B; if the doc is
  silent, the row is mislabeled — demote it.
- **provisional** rows: the honest TODO set. Derivation = sweep-style
  protocol per knob (see `benchmarks/ra_knee_sweep.sh` for the shape);
  queued to the followup-stages umbrella, ordered by suspected perf stakes.
