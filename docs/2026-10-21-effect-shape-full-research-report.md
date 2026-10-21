# Effect-shape full research report — 2026-10-21

**Status:** merged to `main`
**Scope:** the `float_math` / `queue_drain` effect-memory regression, all
experiments, the current selection policy, diagnostics, and future work.

## 1. Problem

Two benchmark classes wanted opposite effect-memory shapes:

- `float_math`: stateful/escaped print kept the reduced accumulator in
  `%xmm0`; stateless print spilled it every iteration (~2.9x C).
- `queue_drain` / `interval_step`: stateless print was required for the
  ring-buffer/int loops; escaped print was ~1.4x C.
- `matrix_pipeline`: stateless SROA/vectorization won.
- `sweep_sparse`: escaped was slow (1.3x) while a stateless loop with carried
  effect state reached ~0.98x.

The goal was one compiler policy with no benchmark names, field prefixes, or
user-facing performance keyword.

## 2. Historical baselines

| Benchmark | Fast era `3c15268a` | State/plain `5aa90185` | Current before this work |
|---|---:|---:|---:|
| float_math | 0.94x | 2.01x | 2.74x |
| telemetry_stream | 0.86x | 1.03x | 1.48x |
| pid_control | 0.99x | 1.23x | 1.23x |
| sweep_sparse | 1.30x | 1.44x | 1.49x |
| queue_drain | 1.35x | **0.45x** | 0.47x |
| interval_step | 1.23x | **0.61x** | 0.72x |

Controlled `5aa90185` variants:

| Variant | float_math | queue_drain | interval_step |
|---|---:|---:|---:|
| stateless print + stack state | 2.72x | 0.45x | 0.61x |
| stateless print + stateful stdout | **0.92x** | 1.44x | 1.46x |
| stateless print + global state | 2.71x | 1.68x | 1.67x |
| stateless print + launder barrier | 2.61x | 0.49x | 0.69x |
| stateless print + noinline print | ~2.7x | — | — |

No single historical build had both float and queue wins.

## 3. Cause investigation

### 3.1 LTO asm

- Stateless countdown: `vmovss 0x24(%rsp),%xmm0` → `vaddss` →
  `vmovss %xmm0,0x24(%rsp)` each iteration.
- Escaped countdown: `vaddss ...,%xmm0,%xmm0`; no per-iteration reload.
- Both had the same arithmetic and guard.

### 3.2 LTO save-temps

`clang -O3 -flto … -Wl,-plugin-opt=save-temps` + `llvm-dis` showed:

- stateless loop: simple `.cdb_45 → .cdl_45` loop;
- escaped loop: `.cdl_47.thread` / `.cdb_47.backedge`, with stdout
  `state.sroa` buffer/fill carried around the flush.

The escaped shape’s speed is an LTO loop-rotation/register-allocation effect
caused by the effect/print path, not by the FP math.

### 3.3 Refuted hypotheses

Under the real LTO pipeline:

| Probe | Result |
|---|---|
| one/two dummy loop-carried i64 phis | still stack reload |
| float/pointer phi | still stack reload |
| escaping dummy alloca | still stack reload |
| `opt -passes=loop-rotate` (metadata stripped) | still stack reload |
| batched pure inner loop | fast shape but **wrong float output** |

Batch output:
- 5M: `1,497,752` vs C `1,449,352.5`
- 10M: `2,992,644.5` vs C `2,699,352.5`

Pure inner loops let `fast` reassociation/vectorization change the float32
result. Output parity is mandatory, so the batch shape was rejected.

The non-LTO dummy-phi probe was an artifact; all codegen claims must use the
linked `-O3 -flto` pipeline.

## 4. Effect-state carry

Carry the plain effect fields (`stdout_buf`/`stdout_fill`, from `plain_lets`,
no names) through the loop as SSA phis; reload after the guard; merge at the
latch; no `%State` escape, no second buffer, no ABI split.

Checkpoint:
- branch `experiment/effect-state-carry`
- tag `checkpoint-effect-state-carry-2026-10-21`
- code `3c1611d2`, doc `67973adc`

Controlled results:

| Benchmark | carry/latch | carry/exit | escaped baseline |
|---|---:|---:|---:|
| float_math | 0.98x | 0.97–0.99x | 0.89–0.96x |
| float_math_nonzero | — | 0.97x | 1.21x |
| telemetry_stream | 0.99x | 1.01–1.03x | 0.96–0.98x |
| pid_control | 0.99x | 1.00–1.01x | 0.99–1.01x |
| sweep_sparse | 0.97x | 0.98x | 1.31–1.33x |
| queue_drain | 0.50x | 0.51–0.56x | 0.50–0.53x |
| interval_step | 0.69x | 0.75x | 0.71–0.75x |
| matrix_pipeline | 0.96x | 0.97x | 0.96–0.97x |
| kalman_filter_runtime | — | 0.86x | 1.00x |
| mixed fixture | 0.228s | — | 0.205s |

Carry confirmed the cause and produced the best shape for several loops, but
was not a universal replacement.

## 5. Current selection policy

Program-level:

```
all direct-Float loops prefer carry AND use countdown
    -> stateless print + carry selected loops
otherwise
    -> escaped fallback
```

Per direct-Float loop:

```
carried_float_count > 0
and not fully temp-mediated
and carried_float_count <= target.float_registers
and (temp_mediated_float_count >= target.effect_carry_temp_min
     or any vector group width <= target.vector_min_width)
```

Target config (`config/targets.dbvl`, `TargetSettings`):

- `float_registers`
- `vector_min_width`
- `effect_carry_temp_min` (field 6, default 3)
- `fp_reserved_temps` (capability gate for escaped)

No benchmark names, field prefixes, or vocabulary matching.

### 5.1 Countdown-eligibility gate

`BatchShape` now records `txn_name`. `program_carry_mode` requires every
direct-Float loop to be the owning batch txn. A no-batch program therefore
falls back to escaped instead of suppressing escaped and emitting a loop
engine that has no carry path.

Synthetic no-batch direct 4-Float loop:
- stateless without carry: **0.2406s**
- escaped fallback: **0.1821s**
- after gate: **0.182s**

This closed a 32% regression path.

## 6. Current full runtime results

Latest gated run: **all rows MATCH**.

| Benchmark | Ratio |
|---|---:|
| float_math | 0.97x |
| float_math_nonzero | 0.96x |
| kalman_filter_runtime | 0.86x |
| sweep_sparse | 0.97x |
| queue_drain | 0.50x |
| interval_step | 0.73x |
| telemetry_stream | 0.98x |
| pid_control | 1.02x |
| accumulator_flush | 0.90x |
| matrix_pipeline | 0.97x |
| sweep_mid | 1.40x |
| nbody_newton | 1.08x |

Library tests: **3271 passed**. Quick and timing guards: **PASS**.

No confirmed selector-caused correctness failure. Remaining small spread is
dominated by run-to-run variance.

Follow-up verification after F1–F4: `cargo test --lib` 3272 passed; quick and
timing guards PASS; full runtime suite all MATCH. Selected ratios: float_math
0.98x, float_math_nonzero 0.95x, kalman 0.88x, sweep_sparse 1.00x,
queue_drain 0.54x, interval_step 0.76x, telemetry 0.98x, matrix 0.97x.

## 7. Code map

| Area | Location |
|---|---|
| Loop facts, Local classification, carry predicate | `src/analysis/loop_shape.rs` |
| StorageClass + LoopMaterializationPlan | `src/analysis/materialization.rs`, `src/backend/mod.rs` |
| BatchShape owner (per-txn map) | `src/analysis/batch_shape.rs`, `AnalysisResults.batch_shapes` |
| Carry emission | `src/backend/llvm/loop_engine/counter.rs` |
| Carry/effect selection | `src/backend/llvm/mod.rs` |
| Target config | `src/config_tuning.rs`, `config/targets.dbvl` |
| Guard | `benchmarks/effect_shape_guard.sh` |
| Mixed fixture | `tests/tier1/mixed_effect_shape.bv` |
| Canonical direction | `docs/plans/2026-10-21-canonical-local-effect-model.md` |
| Selection record | `docs/plans/2026-10-21-effect-carry-selection.md` |

## 8. Regression playbook

1. **Run the guard**
   ```bash
   bash benchmarks/effect_shape_guard.sh --quick
   bash benchmarks/effect_shape_guard.sh --timing
   ```
2. **Wrong shape decision**
   ```bash
   BRIEV_DIAG_LOOP=1 ./target/release/brievc build benchmarks/float_math.bv --out /tmp/shape
   ```
   Inspect `floats`, `temp_mediated`, `aggregates`, `vector_groups`.
3. **Force shapes for A/B**
   ```bash
   BRIEV_DIAG_EFFECT=stateless|escaped ...
   BRIEV_DIAG_EFFECT_CARRY=off|1|latch|exit ...
   BRIEV_DIAG_LOOP_ENGINE=version|perfield|batch ...
   BRIEV_DIAG_PLAIN_CONTEXT=1 ...
   ```
4. **Right shape, wrong speed**
   - inspect the hot loop for per-iteration `vmovss 0x…(%rsp),%xmm0`;
   - check whether the loop is actually vectorized/rotated;
   - use the linked LTO commands, not `clang -S` without LTO:
     `clang -O3 -flto -march=native -ffast-math -fdata-sections -ffunction-sections -Wl,--gc-sections <ll> lib/runtime/briev_rt.c -o <bin>`
   - save LTO temps with `-Wl,-plugin-opt=save-temps`.
5. **Queue/interval regressions**
   - confirm no `effect shape escaped` or `effect state carried` line appears;
   - check the aggregate/Int gate in `prefer_effect_carry` /
     `escaped_effect_cause`.
6. **Mixed-program regression**
   - check `mixed_effect_shape` output multiset in the guard;
   - remember the program-level selection limitation (per-txn batch is done;
     per-loop carry/escaped mixing still needs effect-context work).
7. **Rollback points**
   - `checkpoint-effect-state-carry-2026-10-21`
   - `checkpoint-effect-carry-selection-2026-10-21`
   - branches `experiment/effect-state-carry`,
     `experiment/effect-carry-selection`

## 9. Remaining work

Follow-up plan: `docs/plans/2026-10-21-effect-shape-followups.md`.

1. **Per-txn `BatchShape` map — DONE (`118eb20c`).**
   `AnalysisResults.batch_shapes` is a `HashMap<String, BatchShape>`;
   `detect_batch_shapes` records every qualifying txn and countdown/carry use
   the current txn's entry. `detect_batch_shape` remains the unique-shape
   compatibility helper.
2. **Per-loop carry/escaped mixing** — current mode is program-level. True
   per-loop selection needs explicit effect-context work.
3. **Canonical Local/effect context** — loop-local fields stay SSA; effect
   state is a separate `noalias` context; machine `%State` only at observable
   boundaries.
4. **FP evaluation-order policy** — any shape that removes the guard from the
   hot loop can change `fast`-math results. Pin/emit the required order.
5. **Controlled A/B for noisy rows** — telemetry, sweep_mid, ring,
   queue_sym; interleave runs and compare medians before calling regressions.
6. **Public API review** — `AnalysisResults` gained a field; confirm plugins
   and external constructors are unaffected.

## 10. Merge record

Merged to `main` from `feat/local-effect-canonical` as a fast-forward at
`7fdb2b10`. The merge includes the Local/materialization foundation, carry
experiment, selection policy, guard updates, reports, and all checkpoint docs.

The long-term canonical direction remains open; this merge is the best measured
interim policy, with the limitations above documented and guarded.
