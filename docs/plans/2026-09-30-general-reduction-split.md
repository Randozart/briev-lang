# General reduction-split pass (2026-09-30)

A **general, shape-driven** lowering: when a work-item kernel reduces over
an inner span and the resulting grid underfills the SMs, split the
reduction span across `S` grid slices → partial reductions → a combine.
One mechanism canonicalizes three known levers: the decode-attention `pv`
(5a, `2026-09-30-5a-decode-attention-parallel.md`), GEMM 2048³ split-K
(L3), and small-K decode GEMMs (L4).

**No special-casing (Rules 15/19/23/24).** The pass keys on structural
reduction recognition + a measured underfill cost — never on op/type names,
never `"attention"`/`"softmax"`/`NKV`/`D=128` literals. The efficient path
is the model-selected default (Rule 2); knobs are diagnostics only.

## Architecture (frontend-driven dispatch)

- **Analysis** `src/analysis/reduction_split.rs` (new): produces
  `ReductionSplit { factor: S, kind: Dot|Softmax|DeferredGemm, inner: Expr,
  partial_buf }`, attached to the existing `KernelShape`
  (`src/analysis/accel.rs:199`, beside `reduction` / `deferred_normalize`),
  plus a **combine node** appended to `GpuSchedule`
  (`src/analysis/gpu_schedule.rs`, topo-ordered — deterministic, no atomics;
  protects float determinism / house-rule §4).
- **Backend** consumes it in `build_ptx_kernels` (`src/backend/ptx/mod.rs:1699`)
  exactly as it already consumes `reduction`/`deferred_normalize`
  (~L1583/1788/1817) — a deterministic switch, no new heuristics.
- **Runtime**: grid = `work_items × S`; the `work_n`→grid path and
  `block_per_workitem` + 2D dispatch already carry a slice tag. Partial
  buffers live in the existing flat state/workspace.

## Detection (structural)

Reuse, all name-agnostic: `ReductionInfo { Dot | Softmax }`
(`accel.rs:130`), `deferred_normalize` (`accel.rs:151`), and the backend
detectors (`has_lane_reduction`, `has_warp_slice`, `detect_deferred_region`
in `general.rs`). Trigger = **underfill only**: `work_items < sm_count ×
ctas_per_sm` AND the inner span is chunkable.

**Integration refinement (found 2026-09-30).** For the deferred region the
`j` span is backend-detected (`DeferredRegionParts.la_end` = KV), not
present on `KernelShape`. So the split factor is computed **at the deferred
emission site** (`general.rs::emit_deferred_region`, where `kv` and
`self.count` are both in scope) via
`gpu_strategy::reduction_split_factor(count, kv)` — the same pattern the
backend already uses for the GEMM tile (`gpu_strategy::select` at
`mod.rs:2006`), so the "backend consumes a model decision" pillar holds.
No `KernelShape` field is needed for the deferred case; a `KernelShape`
field may still be added for the cooperative Dot/Softmax path.

## Cost model

Extend `src/analysis/gpu_strategy.rs` (the existing underfill physics):
`reduction_split_factor(work_items, inner, hw)` → `S`, bounded by inner
divisibility (`inner/S ≥ min_chunk`) and a combine-cost margin so it fires
only when the fill gain beats the partial writes + second kernel.

## Emission

- **Partial phase**: per `(work_item, slice)` — `Dot` → partial sums;
  `Softmax`/deferred → partial `(m, l, acc)` (the online-softmax merge
  algebra, already proven by `detect_deferred_normalizer`); tensor GEMM →
  split-K partial `acc`.
- **Combine**: general reduction-combine, scheduled after the partial
  kernel; combine kind derived from the reduction kind, not a benchmark.

### ARCHITECTURAL PREREQUISITE (found 2026-09-30): two kernels per node

The generated runner dispatches **one kernel per accel node**
(`// kernel node 'fattn'` → one `briev_accel_launch_resident`;
`src/backend/spirv/runner.rs:1233` for the deferred-region geometry). A
split phase needs `partial → combine` **two ordered launches for one
node**, so the change spans four surfaces:

1. **Runner codegen** (`runner.rs`, dispatch cases ~L1106–1240): a new
   geometry case emitting two ordered launches; the `RunnerKernel` must
   carry the split metadata (factor + combine kernel index).
2. **Combine emitter** (new, `general.rs`): per work item, merge `S`
   partials via the online-softmax algebra
   (`acc = Σ acc_s·exp(m_s − m*)`, `l = Σ l_s·exp(m_s − m*)`,
   `out = acc/l`) or plain sum for `Dot`.
3. **Partial emitter** (`emit_deferred_region`, `S > 1`): decode
   `(h = cid/S, slice = cid%S)`, reduce the slice's `j` sub-span, write
   per-slice `(m_s, l_s, acc_s)`.
4. **Workspace**: the accumulator buffer (`acc_buf`, e.g. `o1` sized
   `H·NKV`) holds the `S·(2+D)` partials per work item — no layout change,
   gated on `NKV ≥ S·(2+D)` (else `S = 1`).

**Ordered implementation (each committed + device-gated):**
- **S1** `RunnerKernel` split metadata + runner two-launch codegen (no
  math change — verify a two-kernel node launches in order).
- **S2** Combine emitter + a synthetic-region unit test.
- **S3** Partial emitter behind a default-off knob; validate the merge
  against the `S = 1` path on-device (`m3_attention_harness.sh`) **before**
  any timing claim.


## Phasing (each increment = Rule-20 pre-B + A/B + both-lane correctness)

1. **Cost estimator** (`reduction_split_factor`, pure + tests) — this commit.
2. **Detection** — `ReductionSplit` on `KernelShape`, analysis tests.
3. **PTX general partial+combine** for the deferred region (unblocks 5a).
4. **Dot / cooperative-softmax** variants.
5. **Tensor split-K** (L3/L4) against the GEMM grid.
6. **SPIR-V** consumes the same decision.

## Gates

Correctness both lanes (`m3_attention_harness.sh`, `softmax_gate.sh`,
GEMM all-ones); latency via the high-REPS microbenches; `cargo test --lib`;
Praetor no new diagnostics; docs in the same commit; delete
`fused_attention_*` only at decode parity (Rule 24).

## Decisions (resolved)

- Combine = **scheduled second kernel** (deterministic), not atomics.
- Estimator housed in `gpu_strategy` (shared underfill physics with GEMM).
- Tier order: **PTX-general first** (decode/5a), tensor next.
- Integration: **extend `KernelShape` + `GpuSchedule`** (minimal churn).
