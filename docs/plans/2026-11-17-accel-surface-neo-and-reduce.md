# Accel surface, NEO residency, reduce lane — the next three workstreams (2026-11-17)

Follows `docs/plans/2026-11-17-ptl-night-queue.md` (night 1: `brievc run` was the
instrument) and `2026-11-17-ptl-tiled-gemm-dispatch.md` (night 2: the Vulkan runtime
read only `LocalSize.x`, so the tiled GEMM ran 16× over-dispatched).

Six instrument defects have been found in this campaign — dead-pid soak curve,
shared `--out` in the corpus runner, `.s` reports written into the measured tree,
`brievc run`'s own dispatch arithmetic, the `LocalSize` parse, and now the silent
CPU fallback. Each one produced a plausible number for the wrong subject. This plan
finishes that class of work before adding more performance.

## S1 — close the accel surface (do first)

Two live lies, one missing gate.

### S1.0 — survey what actually falls back today (half a day, and it sets the agenda)

21 files in `tests/`, `benchmarks/`, `examples/`, `lib/` declare `accel node`. An
accel kernel that cannot be lowered to SPIR-V currently emits
`warning: accel 'X': kernel emission failed — … (staying CPU)`
(`src/backend/llvm/kernel.rs:194`) and runs **on the CPU lane**, with an empty
`AccelKernelBlob`. Every benchmark number for such a program is a CPU number wearing
a GPU label.

Deliverable: a table of all 21 — file, kernel, falls back?, reason — produced by a
one-shot script (compile each with the warning captured). This is the size of the
problem and it decides how urgent S1.1 is. Expect surprises: `benchmarks/recommender.bv`
and the `g0_*` fixtures are exactly the sort of program whose kernel surface was
written before the current lowering rules.

### S1.1 — emission failure is an error, not a warning

An `accel` node is a **declaration** that this work belongs on the device. If the
compiler cannot honour it, that is a compile error with what/why/fix — the same
shape as every other diagnostic in `src/errors.rs`:

```
error: accel kernel 'gemm' cannot be lowered to SPIR-V: unsupported statement
`when` in a kernel body. Kernel bodies admit let/assign/expression/term over
integer expressions. Move the data-dependent branch out of the kernel, or declare
the node without `accel` to run it on the CPU lane.
```

Three call sites, three decisions:

* `kernel.rs:194` (emission failed) → **error**, carrying the lowering reason;
* `kernel.rs:57` (no `TypeUniverse`) → **error**; this is a compiler-state bug, not
  a program property, and it must not be reproducible from source;
* an explicit escape hatch for intentional CPU work — `--accel-allow-cpu-fallback`
  — so a test or a deliberately CPU-only program can still say so out loud. Default
  off. The point is not to forbid the CPU lane; it is to forbid *silence*.

Blast radius = exactly the S1.0 survey. Anything that fails after this change was
already mis-measured, so the change converts hidden debt into a visible list.

### S1.2 — the f16 fallback store emits invalid IR

`a[i] = 1.0 as Float16;` inside a kernel body that falls back produces
`store half %t35` with `%t35 : float` (`BUGS.md`, 2026-11-17). Host code is fine —
`sitofp i64 … to half` works — so the trigger is the fallback re-emission, and the
narrowest repro is still to be pinned (constant index vs variable index inside a
`when` guard both need trying). Fix direction: the `Float`→`Float16` cast must emit
`fptrunc float → half`; the `FloatWidth` lane exists for this, so the question is
whether `Float16` resolves as a `Float` protocol member in the universe. If it does
not, the cast silently degrades to identity and lands a `float` in a `half` slot.

### S1.3 — the differential correctness gate

The gate that would have caught night 2's bug on day one. Two pieces:

* **S1.3a — `_runner.c` for `.bv` accel programs.** Today only `.abv` emits a
  runner, so the trustworthy measurement path (build → link the emitted runner →
  run) does not exist for mixed-lane programs. Without it, the only way to time a
  `.bv` accel program is `brievc run`, i.e. the instrument that lied for two weeks.
* **S1.3b — a `compare` facility in the runner.** Not a wider kernel surface: the
  runner is C, so the honest reference is **host-side**. Give the runner a
  `[check]` block that compares two projection arrays (the GPU result and a
  CPU-lane reference) and prints
  `[check] y max_abs_diff=<x> n_mismatch=<n>`.
  Rationale: `foreach` + local `let`s is outside the runner v1 surface
  (`Statement::Term/Tail/EndProgram` plus lets/assigns only), and widening that
  surface to express arbitrary reference kernels is a much bigger job than letting
  the CPU lane compute the reference and comparing buffers. The differential test is
  the *variant-diff* principle applied to kernels: same inputs, two
  implementations, machine decision.

Gate: a deliberately-wrong kernel (tile offset off by one) must be caught, and the
fixed tiled/coopmat GEMMs must pass at both `512³` and `4096³`.

## S2 — NEO residency (the last real performance lever)

`lib/runtime/briev_dev_opencl.c:267` is `briev_dev_opencl_launch`: it copies the
whole projection, enqueues 1-D, and returns. The Vulkan/L0 shape does better —
`BrievDeviceDriver` already carries optional `mapped`, `launch_dev`, `launch_dev2d`
and `launch_dev2d_batch` (`src/accel_rt.rs:188-192`), and `resident_ops_missing()`
falls back to the copy path when a driver leaves them `None`. So this is a
**driver-side fill-in**, not new compiler machinery:

* implement `mapped` (device-side buffer of the projection, written on `map`, read
  back on `unmap`) — for OpenCL either `clImportMemoryARM`/USM if available, or an
  explicit `cl_mem` plus range writes for the dirty scalars;
* implement `launch_dev2d` so a kernel is enqueued without a projection copy, and
  reuse **one** `cl_event` for the timestamp instead of creating one per dispatch
  (that alone is a large share of the current wall/device gap);
* gate: **wall ≈ device** per dispatch, the shape L0 reached, measured with the
  same instrumentation and the S1 differential gate green first.

Order matters: do not optimise a path whose correctness gate does not exist yet.

## S3 — the reduce lane, behind its licence

`ceil_reduce_f16` (unlicensed) is 0.229 ms against the reference's 0.0969, because
it refuses to reassociate: one invocation per row, serial K loop, 16 workgroups.
`ceil_reduce_approx_f16` (the `approx` keyword) is 0.106 ms — 1.09× of the
reference — so the licence already buys a vectorised path.

The work, *only* under `approx`: emit a cooperative workgroup reduction (one
workgroup per row, `smem` partials + `ControlBarrier`, tree fold) instead of the
serial per-row loop; target ≤ 0.0969 ms. The unlicensed lane stays exactly as it is,
and the results file records why.

Gate: numerical tolerance inside the declared licence, plus S1's differential check.

## Order, cost, gates

| # | item | time | gate |
|---|---|---|---|
| 1 | S1.0 survey of the 21 accel programs | 0.5 d | table recorded; count of silent CPU fallbacks |
| 2 | S1.1 error instead of warning + flag | 0.5 d | corpus/CI triaged; each failure traced to a real gap |
| 3 | S1.2 f16 fallback store | 0.5 d | repro fixture emits `fptrunc`; `cargo nextest --lib` |
| 4 | S1.3a `_runner.c` for `.bv` accel | 0.5 d | a `.bv` accel program measures identically via build+link and via run |
| 5 | S1.3b runner `[check]` compare | 1 d | wrong kernel caught; fixed GEMMs pass at both shapes |
| 6 | S2 NEO residency | 1–2 d | wall ≈ device per dispatch |
| 7 | S3 reduce behind `approx` | 1 d | ≤ 0.0969 ms, tolerance honoured |

## Risks

* **S1.1 could fail the corpus loudly.** That is the point, but it needs a triage
  session rather than a mass exemption. Any file kept on the CPU lane must say so
  with the flag and a comment, not by silence.
* **S2 touches the runtime ABI** (`briev_accel_*`), which the generated runners
  call. Fill in the new optional entries; do not change an existing signature.
* **S3 is the only place a numerical licence is spent** — the tolerance check must
  live in the gate, not in a comment.

## Doctrine notes

* *Prove it or say why*: S1.0/S1.1 turn a silent fallback into a list, S1.3 into a
  machine-checked diff. Neither is a performance feature; both are the precondition
  for trusting any GPU number after them.
* *One decision point*: night 2's `LocalSize` bug existed because the workgroup size
  was derived in three places. S1.3b deliberately puts the comparison in the runner
  rather than a fourth kernel-path special case.
* *The licence is source-level*: S3 keeps `approx` as the only thing that permits
  reassociation, so the language continues to price the difference visibly.

## Outcome log

### S1.0 + S1.1 — DONE (2026-11-17)

`scripts/accel_surface_survey.sh` classifies by **artifacts** (a non-empty
`@briev_kernel_<name>` blob or a written `.spv`), not by the absence of a warning —
the first pass made exactly the mistake it was hunting and reported `gpu 21, cpu 0`;
the artifact pass reports **`gpu 14, cpu 7`**, including all six `g0_*` Intel gate
fixtures and `benchmarks/recommender.bv` (three `async accel node`s, zero kernels).
Measured mode table on `g0_sum.bv`: `keyword-only` → no kernel, no warning;
`force` → kernel; `try-all` → no kernel. The kernel is emittable; the default
policy declines to dispatch it and said nothing.

`analysis::accel::undelivered_device_bodies` + a `--deny-accel-cpu-fallback` build
flag now report every such body with why+fix; `g0.sh` passes the flag so the G0 gate
cannot be fooled by a fixture that stops delivering. Results:
`benchmarks/results/2026-11-17-accel-surface-survey.md`.

**Deviation:** shipped as warning-by-default + deny flag rather than an unconditional
error (five harnesses build `recommender.bv` with no mode flag; the codebase already
uses this shape for `--deny-memory-ambiguity`).

### S1.2 — DONE (2026-11-17)

The f16 store bug was **two** defects, both name-testing where the rule is width:

1. the cast emitter's float-width branch listed only `float`/`double`, so
   `Float → Float16` returned the source register (no `fptrunc`) and the float
   landed in a `half` slot;
2. the loop engine's backedge identity copy emitted `add {ty} 0, %v` at four
   sites — valid for integers, invalid for every float (`add half 0, %t`).

Fixed with `fp_width`/`fp_cast_op` (width-derived, covering `half`/`float`/
`double`/`x86_fp80`/`fp128`) and one `emit_carry_copy` with a typed zero. The
original BUGS note scoped this to the accel CPU-fallback lane; it was general —
a constant index constant-folds to a literal `half`, a variable index did not.
`brievc build` never ran clang over the emitted `.ll`, so the invalid module
reported success. Gate:
`daily_use_smoke::float16_stores_narrow_instead_of_aliasing`.

**Unplanned but required:** the quick gate was already red on five binaries
(`verify_report`, `unit_test_blocks`, `interpreter_corpus_parity`,
`reduction_scripts`, plus the batch they share) because stdlib resolution was
anchored at the *source file's* directory: a fixture in a tempdir could not find
`lib/std` even with `BRIEV_HOME` set, so the gate only went green when `TMPDIR`
was inside the repo. Added two rungs to `resolve_stdlib_root()` — `BRIEV_HOME`
(same rung `glue::abi::abi_dir` already honours) and the invocation's own
working directory — and repaired the `reduction_scripts` "good" fixture, which
had stopped being good once the no-termination warning landed. `gate.sh quick`
now passes without TMPDIR tricks.

### S1.3 — RESOLVED, but not the way the plan said (2026-11-17)

**S1.3a (emit `_runner.c` for `.bv`) — cancelled: wrong shape.** A `.bv` program
compiles to a *host binary* with the kernels embedded (`@briev_kernel_<name>`), so
the trustworthy measurement path is already `build` → run the binary. There is no
runner to emit and nothing to link; the only thing `brievc run` adds is the
mis-dispatch risk the night-1 work removed. `.abv` keeps the runner because it
*is* the standalone artifact.

**S1.3b (differential correctness gate) — already existed as `g0_gemm.bv`.** The
device lane runs `matmul!(a, b, y, M, N, K, i, Float)`; a plain `node` recomputes
sampled outputs as a CPU dot product and folds the deltas into `G0_GEMM_BAD`. The
missing piece was that nobody had run it on the **Vulkan** lane, where the tiled
16×16 kernel lives (`g0.sh` gates it on levelzero/opencl, where `matmul!` has no
OpenCL projection and falls back with a self-check). Measured on PTL:

```
$ BRIEV_ACCEL_DEVICE=vulkan brievc run --accel=force --deny-accel-cpu-fallback g0_gemm.bv
[briev_accel] kernel 'gemm' compiled on 'vulkan'
G0_GEMM_BAD=0 GPU_SUM=131033.188 CPU_SUM=131033.188 Y0=28.46875 YN=35.0625
```

So the tiled tier's values are verified against a CPU reference on the device, and
the whole G0 suite passes with `--deny-accel-cpu-fallback` (all six previously
undelivered fixtures now deliver under `--accel=force`): levelzero g0_elem 0.153 ms,
g0_sum 0.574, g0_minmax 0.572, g0_fsum_approx 0.295, g0_fsum_exact 0.287.

**Two gaps this exposes, recorded rather than papered over.**

1. **A value check cannot see the 16× bug's harm.** Over-dispatch wrote each tile
   16 times with *identical values* and sprayed the surplus workgroups outside
   `y`. `BAD=0` was therefore true before the fix too — the damage was 16× the work
   plus out-of-bounds writes into neighbouring fields. The gate needs an
   **input-integrity canary** (checksum `a`/`b` after the GEMM), which is the only
   cheap way a value comparison catches an OOB write.
2. **`--deny-accel-cpu-fallback` is compile-time only.** It covers a body the
   analyser declines. It does not cover a body that *is* delivered but has no
   projection for the selected driver — `gemm` on levelzero prints
   "CPU fallback (no OpenCL projection, Vulkan-only until G3), self-check ok" and
   still passes. A per-lane projection gate is the next step for that class.

**Also found:** `g0_fsum_approx on opencl: WRONG (bad=?)` — an OpenCL-lane
correctness bug in the approximate fsum kernel, currently reported-not-required by
`g0.sh`. Recorded for the reduce work (S3).

### S2 — PARTIAL, with the premise corrected (2026-11-17)

Driver work landed (persistent `cl_mem`, dirty-range uploads, `launch_dev` /
`launch_dev2d`) and is correct against the ABI, but **it is not on the dispatch
path**: a freshly built archive still logs only the copy path for
`ceil_elem_f32.abv` on OpenCL, and the resident marker never fires. Two
`briev_dev_opencl` driver tables exist (`libbriev_gpu_rt.a`,
`libbriev_accel_rt.a`) and the link order appears to decide which the runtime
resolves — a one-hour investigation, not a rewrite.

The larger finding: **the plan's premise was wrong.** Vulkan's residency win comes
from `vkMapMemory` (device-visible host memory). OpenCL with regular `cl_mem` has
no equivalent, so "residency" there saves an allocation and an event, not the two
projection copies that dominate wall time. The honest lever is fine-grained SVM
(`clSVMAlloc` + `CL_MEM_SVM_FINE_GRAIN_BUFFER`, probed, never assumed), and the
metric should be **copies per dispatch**, not wall ≈ device.

Details and next steps: `benchmarks/results/2026-11-17-neo-residency-partial.md`.

### S3 — not started (2026-11-17)

Blocked behind S2's measurement plumbing in the sense that it needs the same
`copies per dispatch` instrument to tell a real win from a copy artefact. The
reduce numbers stand as recorded: unlicensed 0.229/0.304 ms (f16/f32), licensed
`approx` 0.106, reference 0.0969 — and `g0_fsum_approx` reports **WRONG on the
opencl lane**, which must be fixed before the licensed lane is tuned.
