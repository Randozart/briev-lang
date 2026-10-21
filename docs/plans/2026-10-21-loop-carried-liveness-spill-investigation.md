# Loop-carried liveness and the float accumulator spill

**Date:** 2026-10-21
**Status:** INVESTIGATION — cause isolated, minimal trigger being ablated.
**Related:** `docs/plans/2026-10-21-per-loop-effect-abi.md`,
`docs/plans/2026-10-21-target-pressure-effect-shape.md`,
`benchmarks/results/2026-10-21-target-pressure-effect-shape.md`.

## Observation

`float_math` has two output-correct shapes:

| Shape | float_math | Hot-loop accumulator |
|---|---:|---|
| stateless print, countdown | ~2.9x | `vmovss 0x24(%rsp),%xmm0` every iteration |
| escaped/stateful print, countdown | ~0.9x | stays in `%xmm0` |

Both loops have the same arithmetic, the same guard, and the same FP flags.
The difference is in LLVM's register allocation, not in the Briev math.

## Optimized-IR comparison

Stateless loop (clang `-O3 -march=native -ffast-math`):

```
.cdb_45:
  %reduced.phi18 = phi float [ %t145, %.cdl_45 ], [ 0.0, %entry ]
  %cdr527       = phi i64   [ %cdl53, %.cdl_45 ], [ 5000000, %entry ]
  %cdc506       = phi i64   [ %cdn51, %.cdl_45 ], [ 0, %entry ]
  %t145 = fadd fast float %reduced.phi18, 3.0e-1
  ...
```

Escaped loop:

```
.cdb_47:
  %reduced.phi112 = phi float [ %t147, %.cdl_47 ], [ 0.0, %entry ]
  %cdr5411        = phi i64   [ %cdl55, %.cdl_47 ], [ 5000000, %entry ]
  %cdc5210        = phi i64   [ %cdn53, %.cdl_47 ], [ 0, %entry ]
  %state.sroa.0.09 = phi i64  [ %state.sroa.0.1, %.cdl_47 ], [ 0, %entry ]
  %state.sroa.6.08 = phi i64  [ %state.sroa.6.1, %.cdl_47 ], [ 0, %entry ]
  %t147 = fadd fast float %reduced.phi112, 3.0e-1
  ...
```

The escaped shape keeps the stdout buffer/fill as **loop-carried `%State`
projections**. SROA removes those phis in the stateless shape.

## Controlled probe — first attempt was misleading

Injecting one extra loop-carried `i64` phi into the optimized stateless IR
compiled **without `-flto`** changed the asm to keep `%xmm0`. That suggested
“phi count/liveness” was the trigger.

**The LTO run refutes that.** Recompiling the same variants with the harness
pipeline (`clang -O3 -flto -march=native -ffast-math …`) gives:

| Variant | per-iteration accumulator reload |
|---|---:|
| stateless baseline | 1 |
| one/two dummy i64 phis, latch or guard-live | 1 |
| float phi | 1 |
| pointer phi | 1 |
| escaping dummy alloca | 1 |

So simply adding loop-carried phis does **not** reproduce the escaped shape
under the real pipeline. The earlier non-LTO observation was an artifact, and
this is an instance of the project’s LTO lesson: `-O3`/raw IR does not reflect
the linked `-O3 -flto` result.

## What LTO actually shows

Using `-Wl,-plugin-opt=save-temps` and `llvm-dis` on the LTO `.opt.bc`:

- Stateless loop: `.cdb_45` → `.cdl_45` → `.cdb_45`; the only loop-carried
  values are the accumulator, rem, and counter.
- Escaped loop: the print path produces `.cdl_47.thread` and
  `.cdb_47.backedge`; the stdout `state.sroa` values are loop-carried, and the
  loop is rotated/split around the guard’s flush path.

The escaped shape’s speed is therefore tied to **the stateful print path
changing LTO loop rotation / register allocation**, not to a simple phi count.
Adding phis or an escaping dummy alloca is insufficient; the guard/print path
must participate.

## Open question

What is the minimal trigger?

- one phi vs two?
- `i64` vs float vs pointer?
- store every iteration vs guard-only?
- is the store itself (a hot global write) the real trigger, or the phi shape?
- does a semantically real loop-carried value (`stdout_buf`/`stdout_fill`)
  reproduce it without an artificial dummy?

## Aims and plan

Experiments are evaluated on **output parity + regressions first**. A shape is
not rejected for being unusual if it is correct and does not regress the
suite. The only acceptable outcome is measured; no “desirable/undesirable”
decision is made before the data.

1. **Ablation matrix** on the LTO pipeline (every claim must be linked):
   - add/remove the stateful print path and stdout `state.sroa` carries;
   - force/disable loop rotation around the flush path;
   - one/two dummy `i64` phis; guard-only vs every-iteration use;
   - one float phi; one pointer phi; escaping alloca;
   - real `stdout_fill`/`stdout_buf` phis.
2. **Executable A/B** for the minimal candidate:
   - same output at 5M/10M;
   - `float_math`, `queue_drain`, `interval_step`, matrix, telemetry, pid;
   - full suite only if the small set is clean.
3. **Implementation** if a real loop-carried value works:
   - keep the guard-bearing FP order;
   - add the effect state as SSA phis without escaping `%State`;
   - per-txn decision; no stdout split.
4. **Guard**:
   - asm check: no per-iteration `vmovss 0x…(%rsp)` for `float_math`;
   - shape decision and output parity in `effect_shape_guard.sh`;
   - regression ceilings unchanged.

## Non-negotiables

- Output parity at print boundaries is mandatory.
- One stdout buffer.
- No benchmark names.
- No decision based on aesthetics alone; compare the suite.
- If a candidate only works through a hidden compiler-owned dummy, record it
  as such and decide from measured regressions.
