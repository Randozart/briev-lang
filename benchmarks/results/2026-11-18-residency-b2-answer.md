# B2 answered: levelzero is resident, OpenCL refuses — and the Rust-side log can't say why

**2026-11-18, on PTL.** `ceil_elem_f32.abv` → emitted runner → both lanes, with
`BRIEV_ACCEL_VERBOSE=1`.

## What the device says

| lane | entry reached | path taken | gpu_time |
|---|---|---|---|
| levelzero | `briev_accel_launch_resident` → `_2d` (probe confirmed) | **resident**: `launch_dev2d` called, staging buffer allocated (12.6 MB) | 0.19–0.21 ms |
| opencl | `briev_accel_launch_resident` → `_2d` (probe confirmed) | **copy**: `enqueue global=1048576 local=256` | 0.026 ms |

So the residency question has a behaviour-level answer: **residency engages on
levelzero and is refused on OpenCL**, and it is refused *inside* `_2d` (the entry runs;
the copy path follows).

## Why the answer is not a branch name

None of the three instrumented branches logs, even after replacing the cached
`g_verbose` with a direct `BRIEV_ACCEL_VERBOSE` read in both `_2d` and the copy path:

```
[briev_accel] kernel 'elem' compiled on 'opencl'
[briev_accel/opencl] enqueue global=1048576 local=256 n=1048576     ← copy path
[briev_accel/opencl] enqueue res=0
# gpu_time: 0.026 ms
```

No `resident: … -> copy path`, no `prime failed`, no `driver missing ops`.

**The instrument is the defect, not the question.** Rust's `g_verbose` is written once by
`briev_accel_init` and demonstrably reads as 0 in the generated runner (the C drivers read
the env themselves, which is why their `enqueue`/`staging` lines appear and the Rust ones
do not). An unconditional `eprintln!` did print from `_2d`, so the code path exists — the
*gated* logs are what fail.

## Next step, precisely

Stop instrumenting from the Rust side. `lib/runtime/briev_dev_opencl.c` is demonstrably
verbose (`[briev_accel/opencl] enqueue …` reaches the terminal), so put the markers there:

* an unconditional `fprintf` in `briev_dev_opencl_mapped` (does it get called at all?), and
* one at the top of `briev_dev_opencl_launch_dev2d` (does the resident dispatch arrive?).

If `mapped` is never called, `_2d` returns before the mapped block — which the code reads
as impossible without one of the logged branches, so a `kernel_ready` or descriptor
problem is the likely culprit. If `mapped` *is* called and returns the staging pointer,
then `prime_lazy_buffer` succeeded and the refusal is in `seed_or_sync_scalars`.

## What this cost, and the lesson

Three sessions carried a hypothesis ("the resident path is inert / duplicate driver tables")
that `nm` refuted and this run finally replaced with a measurement. The remaining unknown is
narrow, and the honest state is: **residency works on the lane with device-mapped memory and
is refused on the lane without it — plus an unresolved sub-question about where, whose
instrument is broken.** Both facts belong in the code comments, not in a results file only.
