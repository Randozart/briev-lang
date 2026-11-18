# NEO/OpenCL residency: what the lane actually does (2026-11-17, partial)

Follows `docs/plans/2026-11-17-accel-surface-neo-and-reduce.md` §S2. This records
measurements, not intentions: the driver work landed but **is not yet on the
dispatch path**, and the reason is structural.

## The copy path is the whole story today

`briev_dev_opencl_launch` (`lib/runtime/briev_dev_opencl.c`) does, per dispatch:

1. `clCreateBuffer(proj_bytes)`
2. `clEnqueueWriteBuffer` — the entire projection
3. `clEnqueueNDRangeKernel` (1-D, `local_x` from `OpExecutionMode`)
4. `clFinish` → `clGetEventProfilingInfo` (create + release an event)
5. `clEnqueueReadBuffer` — the entire projection back
6. `clFinish` + `clReleaseMemObject`

Two full projection copies plus an allocation and three driver round-trips per
launch. Measured on PTL (Arc B390) for `ceil_elem_f32.abv` (1 Mi f32):
`gpu_time: 0.039–0.058 ms` across runs. The device-side number is stable; the wall
number is dominated by the copies.

## What was implemented

`BrievOpenClKernel` gained a persistent `cl_mem dev_buf` / `dev_buf_bytes` /
`proj_bytes` / `dev_seeded`, and the driver table gained `launch_dev` and
`launch_dev2d`:

* the buffer is created once per projection size and reused;
* `full_sync` uploads the whole projection, otherwise only the runtime's
  `(offset,length)` dirty pairs go up (the scalar fields, not the arrays);
* results are left in the host staging window that `mapped` exposes.

Syntax-checked (`clang -fsyntax-only`) and correct against the ABI in
`src/accel_rt.rs` (`LaunchDev2dFn(handle, nx, ny, full_sync, dirty, n_dirty)`).

## Why it is not on the path (the measured blocker)

The runner emits `briev_accel_launch_resident(0, state, n)` for a pure-device
program, and that routes to `briev_accel_launch_resident_2d`, which calls
`launch_dev2d` only when `mapped` is non-null. For OpenCL `mapped` returns
`k->staging`, which is NULL until a *copy-path* launch has run — so the first
dispatch primes it and every later dispatch should take the resident path.

It does not. With a freshly built `target/compiler-in-briv` synced to the node,
`ceil_elem_f32.abv` on `BRIEV_ACCEL_DEVICE=opencl` still logs only the copy path:

```
[briev_accel/opencl] enqueue global=1048576 local=256 n=1048576
[briev_accel/opencl] enqueue res=0
# gpu_time: 0.039 ms
```

and the resident marker added to `launch_dev2d` never fires. Both
`libbriev_gpu_rt.a` and `libbriev_accel_rt.a` carry `briev_dev_opencl` symbols, so
the most likely explanation is that the **driver table the runtime resolves is not
the one this file defines** — the two archives overlap and the link order decides.
That is a one-hour investigation (dump the table's function pointers at load and
compare against the address of `briev_dev_opencl_launch_dev2d`), not a rewrite.

## The architectural finding, which matters more than the plumbing

Vulkan's residency win comes from `vkMapMemory`: the runtime writes the projection
into device-visible memory and only submits. OpenCL has **no equivalent** — with
regular `cl_mem` the host↔device copies remain, so "residency" there saves an
allocation and an event, not a copy. The honest lever is **fine-grained SVM**
(`clSVMAlloc` with `CL_MEM_SVM_FINE_GRAIN_BUFFER`, `clEnqueueSVMMemcpy`/direct
pointer use), which NEO supports on Arc but which must be *probed*, never assumed.

So S2's premise in the plan ("mirror L0's shape, gate on wall ≈ device") was
wrong: the shape is a mirror, but the ceiling is different, and the measurement
target should be "copies per dispatch", not "wall ≈ device".

## Next steps, in order

1. Resolve the duplicate driver table (dump the resolved pointers, or give the
   driver struct a single owner).
2. Probe SVM at init; when fine-grained is available, allocate the projection
   once with `clSVMAlloc` and make the resident path pointer-only.
3. Re-measure `ceil_elem_f32` with copies/dispatch as the metric, then re-check
   the GEMM/reduce lanes for regressions.
