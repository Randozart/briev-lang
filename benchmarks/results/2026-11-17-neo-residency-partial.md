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

and the resident marker added to `launch_dev2d` never fires.

### CORRECTION (2026-11-18): the duplicate-table hypothesis was wrong

This file first blamed two `briev_dev_opencl` driver tables. `nm` disproves it:

```
target/compiler-in-briv/libbriev_gpu_rt.a : D briev_dev_opencl   (defined)
target/compiler-in-briv/libbriev_gpu_rt.a : D briev_dev_vulkan   (defined)
target/compiler-in-briv/libbriev_accel_rt.a: U briev_dev_opencl  (undefined ref)
```

There is **exactly one** table and it is the one this session edited, so link order
is not the explanation. The open question is narrower and testable: of the three
ways `briev_accel_launch_resident_2d` can silently fall back to the copy path —

1. `driver()` returned `None`;
2. `resident_ops_missing(d)` is true (`launch_dev2d` / `mapped` / `launch_dev` reads
   as `None` from Rust);
3. `mapped` is null after `prime_lazy_buffer`;

— the run cannot say which. Next step is one verbose line per branch, plus a check
that the C designated initialiser and the Rust `BrievDeviceDriver` declaration agree
on field order and count (a mismatch reads a live pointer as `None`, and would
explain Vulkan working while OpenCL does not).

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

## A second trap found while instrumenting: `build.rs` can ship a stale runtime

`build.rs` compiles `lib/runtime/*.c` into `target/compiler-in-briv/libbriev_gpu_rt.a`
and `src/accel_rt_standalone.rs` into `libbriev_accel_rt.a`, then prints the link
flags. When the C step or the rustc step fails it **prints a cargo warning and
returns**:

```
warning: briev-compiler@0.3.0: gpu-rt: accel_rt staticlib build failed — brievc run unavailable
```

and the build continues. The `brievc` binary then links against whatever archive
was already there. Reproduced on this machine: after editing `accel_rt.rs` the bin
failed to link with `undefined symbol: briev_dev_vulkan`, and the *only* fix was
`touch build.rs` to force the script to re-run (the subsequent build is green and
`libbriev_accel_rt.a` again carries `U briev_dev_opencl`).

Consequence for every measurement: **a runtime edit can be silently absent from the
binary you measured.** Before trusting any driver-level result, check that the
build did not emit that warning, and prefer a forced `touch build.rs` build after
editing `lib/runtime/**` or `src/accel_rt*.rs`. Whether `build.rs` should re-run on
those inputs (via `cargo:rerun-if-changed`) is the real fix; it is recorded here
rather than changed under a measurement session.
