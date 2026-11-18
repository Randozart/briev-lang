# R1: the OpenCL resident path is reachable — and it dumps core

**2026-11-18, PTL.** Six *unconditional* prints (the verbose gate is proven unreliable in a
generated runner), then removed. `ceil_elem_f32.abv` → emitted runner, `BRIEV_ACCEL_DEVICE=opencl`:

```
[R1] _2d entered idx=0
[R1] driver ok
[R1] kernel_ready ok
[R1] ops present, mapped_is_null=true
[R1] C mapped() called          (×3)
[R1] past prime, about to seed+launch_dev2d
timeout: the monitored command dumped core
```

## What this settles

1. **The resident entry is reached** and every precondition passes: driver resolved, kernel
   ready, all three resident ops present, `mapped()` returns after the prime.
2. The crash happens **between `seed_or_sync_scalars` and the driver dispatch**:
   `[R1] C launch_dev2d called` — the first statement of
   `briev_dev_opencl_launch_dev2d` — never fires. So either `seed_or_sync_scalars` crashes,
   or the call never arrives.
3. **My OpenCL driver work (2026-11-17) is therefore broken, not merely inert.** It was
   written blind, marked inert when it appeared not to be reached, and the reason it
   appeared unreached was a *stale archive*: earlier runs linked a driver whose tail was
   zero, so `resident_ops_missing` was true and the copy path ran — which I twice read as
   "residency is refused by design".

## Action taken

The two entries are **no longer advertised** in the driver table, with the measurement
written into the comment beside the commented-out lines. Verified after the change: OpenCL
is back on the copy path (0.027 ms, no crash) and levelzero is unchanged (0.190–0.204 ms,
staging buffer allocated). Nothing is left claiming a residency the code cannot deliver.

## Next step, precisely

Debug in this order:

1. `seed_or_sync_scalars` — it takes `mapped` (the driver's staging pointer) and the
   descriptor's dirty ranges. OpenCL's `mapped()` returns `k->staging`, which is only set by
   the *copy path*; if the prime set it to a `proj_out` buffer that
   `briev_dev_opencl_launch` has already released, the first dirty-range write is a
   use-after-free — the most likely cause given the crash lands exactly there.
2. Only then re-check `launch_dev2d`'s own assumptions (persistent `cl_mem`, `proj_bytes`).

Certificate for whoever picks this up: the prints are the instrument, and the instrument now
works — three sessions of hypothesis were replaced by one run once the gate stopped lying.
