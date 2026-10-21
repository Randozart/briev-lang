# Effect-shape progress report — 2026-10-21

## Executive summary

The `float_math` regression is understood and a working best-of-both policy
exists:

- escaped stateful print for loops where it wins;
- loop-carried effect state (`Carry`) for loops where it wins;
- stateless print for aggregate/temp-mediated loops;
- Local classification is sound across all top-level surfaces;
- the carry selector is gated on countdown eligibility;
- the remaining threshold is target config, not a hardcoded number.

Current full runtime suite: **all MATCH**. Current library tests: **3271
passed**. Quick and timing effect-shape guards: **PASS**.

The work is on `feat/local-effect-canonical`; `main` is unchanged.

## Branch / checkpoint map

| Branch/tag | Purpose |
|---|---|
| `feat/local-effect-canonical` | current canonical + interim selection work |
| `experiment/effect-state-carry` | original carry experiment |
| `checkpoint-effect-state-carry-2026-10-21` | rollback point for carry |
| `experiment/effect-carry-selection` | carry-selector experiment |
| `checkpoint-effect-carry-selection-2026-10-21` | rollback for selection |

## Commit log

| Commit | Change |
|---|---|
| `3c1611d2` | effect-state carry experiment |
| `67973adc` | carry checkpoint doc/tag |
| `6c8ba811` | Local classification every TopLevel surface; `Export.inner` walker; exported-field test |
| `c273eedf` | `StorageClass` + `LoopMaterializationPlan` in `AnalysisResults`; backend consumes plan |
| `df361b6b` | `BRIEV_DIAG_LOOP` temp-mediated diagnostic |
| `106e04dc` | canonical diagnostic commit pin |
| `63f0e8ca` | carry experiment ported onto canonical base |
| `157470c3` | structural carry selector + program mode + three-way guard |
| `910a9ca8` | selection docs pinned |
| `6b897e1c` | selection merged into canonical branch |
| `e38e3745` | timing guard ceilings for carry winners |
| `d1427095` | countdown-eligibility gate; `effect_carry_temp_min` target config |

## What was learned about the cause

The regression is not the FP arithmetic.

- Stateless countdown: LLVM spills the reduced accumulator to the stack each
  iteration (`vmovss 0x…(%rsp),%xmm0`).
- Escaped/stateful print: the accumulator stays in `%xmm0`.
- The escaped LTO loop carries stdout buffer/fill around the flush and performs
  loop rotation/splitting (`.cdl.thread` / `.cdb.backedge`).
- Dummy phis, an escaping alloca, and generic loop rotation do **not**
  reproduce it. The effect/print path must participate.
- Effect-state carry reproduces the good shape without `%State` escape or a
  print ABI split.

The carry shape has a real tradeoff:

- wins: `sweep_sparse`, `float_math_nonzero`, `kalman`;
- loses slightly: `float_math`, telemetry, accumulator, sweep_mid;
- neutral: nbody family.

## Selection policy

Program-level:

```
all direct-Float loops prefer carry and use countdown
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

Target config: `float_registers`, `vector_min_width`,
`effect_carry_temp_min` (targets.dbvl field 6, default 3).

## Selector fixes from this session

### Countdown-eligibility gate

`BatchShape` now carries `txn_name`. `program_carry_mode` requires every
direct-Float loop to be the owning batch txn. Without a batch shape, the
escaped fallback is used.

Synthetic no-batch direct 4-Float loop:

| Shape | Time |
|---|---:|
| stateless without carry (old risk) | 0.2406s |
| escaped fallback | 0.1821s |
| after gate | **0.182s** |

This closed a 32% regression path for unbenchmarked programs that dispatch
through version-DAG/per-field rather than countdown.

### Config threshold

The `temp_mediated >= 3` constant is now
`effect_carry_temp_min` in target config (field 6, default 3). No hardcoded
policy number remains in the selector.

## Full runtime results

Latest gated selection run: all rows MATCH.

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

The remaining spread is dominated by run-to-run variance in the noisy rows.
No selector-caused correctness failure was observed.

## Remaining work

1. **Per-loop mixed selection** — still program-level. A program with one
   carry-preferred and one escaped-preferred direct loop falls back to one
   shape. True per-loop needs the explicit effect-context work.
2. **Per-txn BatchShape** — `AnalysisResults.batch_shape` is a single
   `Option<BatchShape>`; `detect_batch_shape` returns `None` when more than one
   txn qualifies. Mixed direct-loop programs therefore have no countdown batch
   for the carry path. The next structural step is a per-txn
   `HashMap<String, BatchShape>`; `BatchShape.txn_name` already exists.
3. **Canonical Local/effect context** — the long-term replacement for
   carry/escaped. Local fields should stay SSA; effect state should be a
   separate `noalias` context; no program-wide mode.
4. **FP evaluation-order policy** — any shape that removes the guard from the
   hot loop changes float output under `fast` math. The canonical loop must
   pin or explicitly control this.
5. **Controlled A/B for noisy rows** — telemetry, sweep_mid, ring, queue_sym.
   Full-suite runs show a few percent variation; use interleaved A/B before
   calling any of them a regression.
6. **Public API review** — `AnalysisResults` gained a field and
   `LoopMaterializationPlan`; confirm no external constructor/plugin breakage.
7. **Merge review** — after the A/B pass, merge the branch or keep it as a
   feature default with the documented limitations.

## Merge assessment

The hard blockers found earlier are fixed:

- no-batch/version/per-field carry hole: gated;
- hardcoded threshold: config;
- full tests and guards green.

The remaining issues are architectural (per-loop mixed, canonical effect ABI,
FP policy) and not correctness blockers for the current program-level policy.
A final controlled A/B on the noisy rows should precede promotion to `main`.
