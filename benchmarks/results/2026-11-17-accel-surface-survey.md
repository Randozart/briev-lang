# The accel surface: 7 of 21 declared device programs never reached the device

**2026-11-17.** Survey of every file in `tests/`, `benchmarks/`, `examples/`,
`lib/` declaring `accel node`, produced by
`bash scripts/accel_surface_survey.sh` (logs in `/var/tmp/accel-survey`).

## Why this survey exists

The accel eligibility gate spoke only through a **codegen warning on the emission
path** (`src/backend/llvm/kernel.rs:194`): a candidate whose SPIR-V lowering
failed said `staying CPU`, kept an empty blob, and ran on the CPU lane. A body the
analyser *declined before* becoming a candidate said nothing at all. Either way a
program could declare `accel` and execute on the CPU with no diagnostic, so its
timing was a CPU number wearing a GPU label.

This campaign has already found five instrument defects of exactly this shape
(dead-pid soak, shared `--out`, `.s` reports in the measured tree, `brievc run`'s
own dispatch arithmetic, the `LocalSize.x` parse). This is the sixth.

## The instrument had the same defect twice

The first pass classified by **the absence of a warning** and reported
`gpu 21, cpu 0`. That was the same mistake in a new place. Classifying by
**artifacts** — a non-empty `@briev_kernel_<name> = private constant [N x i8]`
in the emitted IR (`.bv`) or a written `.spv` (`.abv`) — changed the answer to
`gpu 14, cpu 7`. A program is on the device when a kernel exists, not when
nothing complained.

## The table

| file | kernel | note |
|---|---|---|
| `benchmarks/gpu/nbody_force.bv` | yes | |
| `benchmarks/gpu/pairs.bv` | yes | 1824 B + 1628 B (object-CL) |
| `ceil_*_f16/f32.abv` (10 files) | yes | `.spv` written per program |
| `benchmarks/intel_ptl/g0_elem.bv` | yes | 1624 B + 1388 B |
| `benchmarks/nbody_newton_accel.bv` | yes | |
| **`g0_fsum_approx.bv`** | **no** | declared `accel`, zero kernels, zero diagnostics |
| **`g0_fsum_exact.bv`** | **no** | idem |
| **`g0_gemm.bv`** | **no** | idem |
| **`g0_minmax.bv`** | **no** | idem |
| **`g0_sum.bv`** | **no** | idem |
| **`g0_sum_general.bv`** | **no** | idem |
| **`benchmarks/recommender.bv`** | **no** | **three** `async accel node`s, zero kernels |

`gpu 14 / cpu 7`.

## Root cause, measured

`--accel` is a compiler mode. In the default (`keyword-only`) mode a
keyword-marked body is a candidate only if the shape proves eligible **and** the
try-mode policy proves a device win; otherwise `decision = Cpu` and the body is
compiled for the CPU. Three modes against `g0_sum.bv`:

| mode | kernel emitted | `staying CPU` warning |
|---|---|---|
| `keyword-only` (default) | no | none |
| `force` | **yes** | none |
| `try-all` | no | none |

So the kernel is emittable — the default policy simply declines to dispatch it,
silently. `g0_sum`'s row-sum shape is a try-mode miss with an empty
`shape.reasons`, which is why nothing was printed even at the `reasons` level.

The six `g0_*` fixtures are the **Intel G0 gate**: `benchmarks/intel_ptl/g0.sh`
builds them with `--accel=force`, so the gate itself is honest — the fixtures are
only device programs *under the gate's flag*. Outside it (a plain `brievc build`,
a corpus sweep, an ad-hoc benchmark) they are CPU programs that claim otherwise.
`benchmarks/recommender.bv` has no such flag anywhere, so its three accel nodes
have never run on a device.

## The fix

`src/analysis/accel.rs::undelivered_device_bodies(items, accel)` returns a
what/why/fix message for every source-marked `accel` body that will not run on
the device. `src/compile.rs` reports them as warnings, and
`--deny-accel-cpu-fallback` turns them into a hard error (exit 1) — the same
warn-by-default / deny-in-CI shape as `--deny-memory-ambiguity`.

Measured on `g0_sum.bv`:

```
$ brievc build g0_sum.bv                       # exit 0
warning: accel node 'rowsum' will run on the CPU lane, not the device
  why:  the kernel shape is valid, but this mode could not prove a device win for it (try-mode miss)
  fix:  restructure the body to the kernel surface (...), or pass `--accel=force`
        to dispatch it anyway; every timing for this node is a CPU number

$ brievc build g0_sum.bv --deny-accel-cpu-fallback   # exit 1
error: accel node 'rowsum' will run on the CPU lane, not the device
```

Gate: `tests/daily_use_smoke.rs::accel_declaration_is_never_dropped_silently`
with fixture `tests/tier1/accel_lane_report.bv` (the strided row-sum shape that
reproduces the hole). The invariant asserted is *never silent*: either a kernel
is emitted, or the node is named with why+fix, and deny fails.

## Deviation from the plan, and why

The plan called for an error by default. Shipped as **warning by default +
`--deny-accel-cpu-fallback`**, because (a) five benchmark harnesses build
`recommender.bv` without a mode flag and an unconditional error would have
turned a silent wrong number into a broken build in the same commit, and (b) the
codebase already carries this exact shape for `--deny-memory-ambiguity`. The
silence is gone either way; the deny flag is what CI enables.

## Follow-ups (recorded, not silently fixed)

1. **`g0.sh` should assert the kernel was delivered per fixture** — it currently
   asserts `BAD=0` and a `compiled on '<device>'` line for *some* fixtures; the
   six `g0_*` fixtures only deliver under `--accel=force`. The gate should run
   `--deny-accel-cpu-fallback --accel=force` together so a fixture that stops
   delivering fails the gate instead of quietly timing the CPU.
2. **`recommender.bv`** should either restructure its three bodies to the kernel
   surface or be built with `--accel=force` by its harnesses — otherwise its
   numbers belong in a CPU results file.
3. **Try-mode misses should carry a reason.** `shape.reasons` is empty for an
   eligible-but-declined body; the message now says so in prose, but the analyser
   should record which crossover test failed.
