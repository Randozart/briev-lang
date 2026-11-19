# R1: the OpenCL resident path is reachable — and it dumps core (FIXED 2026-11-19)

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

## Root cause (found 2026-11-19)

`gdb` backtrace on PTL:

```
Thread 1 "bin3" received signal SIGSEGV, Segmentation fault.
#0  0x00007ffff7bb2107 in ?? () from /usr/lib/libc.so.6
#1  0x000055555556a679 in briev_accel_rt::accel_rt::seed_or_sync_scalars ()
#2  0x000055555556d043 in briev_accel_launch_resident_2d ()
#3  0x000055555556a1ce in main ()
```

The fault is inside `seed_or_sync_scalars`, which `copy_nonoverlapping`s the scalar fields
into the pointer returned by the driver's `mapped()`. The OpenCL copy path
(`briev_dev_opencl_launch`) did `k->staging = proj_out`, where `proj_out` is the
runtime's **local `Vec`** in `briev_accel_launch` (`accel_rt.rs:799`) — freed when that
priming launch returns. The resident path then re-fetches `mapped()` and writes the dirty
ranges through that dangling pointer. **Use-after-free**, exactly where the crash landed.

The earlier "uaf hypothesis refuted" note was wrong: it assumed `proj_out` lives for the
process, but it is a call-local `Vec`.

## Fix (2026-11-19)

The driver now owns a persistent host window `own_staging` (grown with `realloc`, freed in
`destroy_kernel`), and `staging` aliases it — mirroring the levelzero driver's host-USM
staging. A new `opencl_stage_result()` helper copies the device result into `own_staging`
after each launch, so `mapped()` always returns valid, driver-owned memory. The two
resident entries are **re-advertised** in the driver table.

## Verified results (2026-11-19, PTL)

| Device | Path | Time (ms) | Runs |
|--------|------|-----------|------|
| OpenCL (fixed) | resident | 0.028 | 6 |
| levelzero | resident | 0.198 | 3 |

- 5× `exit=0`, no crash. Verbose confirms `resident dispatch global=1048576 local=256 dirty=0 full=1`.
- OpenCL output matches levelzero output (identical `coll_arena` fields).

## Status: FIXED (2026-11-19)

The crash is fixed. OpenCL residency is live and 7× faster than levelzero. See `BUGS.md`
for the updated entry.
