# Next: residency truth, the GEMM canary, and the pin (2026-11-18)

Follows D1's closure (`2026-11-18-d1-root-cause.md`, census at **0 of 336**). Two
device items stayed queued because that session's budget went into the investigation;
both now have every instrument they need.

## B2. Which residency fallback fires? (1 h, one run)

### The question

`briev_accel_launch_resident_2d` (`src/accel_rt.rs`) has three ways to silently take the
copy path, and 2026-11-17's run could not tell them apart — which is how a wrong
duplicate-table diagnosis survived a session. The logging now distinguishes them:

```
resident: kernel N not ready -> copy path
resident: driver missing ops (launch_dev2d=… mapped=… launch_dev=…) -> copy path
resident: driver mapped() still null after prime -> copy path
```

### Run it right

Residency is a **runner** feature: build the `.abv`, link the emitted runner, run *that*
(`brievc run` drives in-process and does not reach the same entry).

```bash
# on PTL, after syncing lib/, benchmarks/intel_ptl/ and target/compiler-in-briv
./bin/brievc build benchmarks/intel_ptl/ceiling/ceil_elem_f32.abv --stdlib-path lib \
    --config-dir config --out /tmp/res
cd /tmp/res && clang -O2 -I. -o bin ceil_elem_f32_runner.c -L. \
    -lbriev_accel_rt -lbriev_gpu_rt -ldl -lm -lpthread
BRIEV_ACCEL_VERBOSE=1 BRIEV_ACCEL_DEVICE=opencl ./bin
```

### Decision tree — each answer has one next action

* **`missing ops (…=false)`** — the C designated initialiser and the Rust
  `BrievDeviceDriver` in `src/accel_rt.rs` disagree on field order/count, so a live
  pointer reads as `None`. Compare `lib/runtime/briev_accel_rt.h` against the Rust
  declaration field by field; that mismatch would also explain Vulkan working and OpenCL
  not (Vulkan's fields sit before the tail).
* **`mapped() still null after prime`** — expected for regular `cl_mem`: OpenCL has no
  device-mapped host pointer, so residency buys an allocation, not a copy. Record it, and
  make the *next* OpenCL question concrete: does NEO report
  `CL_DEVICE_SVM_FINE_GRAIN_BUFFER`? If yes, the lever is `clSVMAlloc`; if no, OpenCL
  stays at two copies per dispatch and the metric is **copies per dispatch**.
* **nothing prints at all** — the resident entry is not on this path; find the call site
  the runner actually reaches (`briev_accel_launch` vs `…_resident`) and document which
  entry a runner uses. That alone closes B2 as a *documented* answer.

**No code lands** unless the answer is the first case (a concrete bug). The driver work
is written and marked inert; correctness of the copy path is unaffected either way.

## B3. The GEMM input-integrity canary (2 h)

### Why a value check cannot see the 16× class

`G0_GEMM_BAD=0` was **green before** the `LocalSize` over-dispatch fix: the surplus
workgroups wrote identical values into covered tiles and sprayed out of bounds elsewhere.
Output sampling cannot notice; an out-of-bounds write corrupts *neighbours*, so the
neighbours are what must be checked.

### The fixture (`benchmarks/intel_ptl/g0_gemm.bv`)

`a` and `b` are filled on the CPU lane by a closed form:

```briev
a[idx] = ((idx * 7) % 13) as Float * 0.125 - 0.5;
b[idx] = ((idx * 3) % 11) as Float * 0.25 - 1.0;
```

Add a `canary` node after `verify_done` (same sampling shape as `verify`):

* sample `NS` indices spread over both buffers, recompute the closed form on the CPU, count
  mismatches into `canary_bad`;
* also check `y[MN]`-adjacent state is untouched — the arrays are adjacent in `%State`, so a
  tile overflow past `y` lands in whatever follows; the field *order* decides which, and
  the canary should name that neighbour explicitly rather than hope;
* report `G0_GEMM_CANARY=ok|BAD` in the existing `report` node.

### Prove it bites — this is the part that matters

A canary never observed failing is decoration. Vendored copy route: re-create the pre-fix
`LocalSize` parse in a scratch copy of `lib/runtime/briev_dev_vulkan.c` (the two dispatch
sites that used `local_x` instead of `X*Y*Z`), build one `_runner` against it, and confirm
`G0_GEMM_CANARY=BAD` while `G0_GEMM_BAD` stays 0 — that contrast *is* the proof that the
canary sees what the value check cannot.

### Gates

`ok` on both levelzero and vulkan; `BAD` under the vendored old parse; and the fixture
still prints `G0_GEMM_BAD=0 GPU_SUM == CPU_SUM` (no regression in what it already checks).

## B4. Advance the kiosk pin (15 min, after B2/B3)

`briev-kiosk-avatar/compiler.lock` pins `adb92fb7b`; D1's fix (`db7912367`) is the first
commit that changes a *slot type*, which the kiosk's daemon (float-heavy) is the best
consumer test for. Bump, then run `harness/memcheck_ratchet.sh` **and** `harness/soak.py`
— the ratchet pins the census (73/0/2196) and the soak is what caught the RSS story
before; a pin bump without those is a text edit.

## Deliberately not planned yet

* **S3 (reduce)** — a cooperative tree-reduction is a 1-day tuning job and the
  `fsum_approx` lane still returns a wrong sum on OpenCL (the workgroup-scope fix was
  refuted on device). Tune only after correctness, and only with the licence respected.
* **`g0_fsum_approx` OpenCL** — two candidate mechanisms remain (make `LocalSize` equal
  the reported subgroup width, or lower through shared memory + barrier + lane-0 store);
  both are experiments with a device in hand, not a plan until one is measured.
* **`lib/` sweep in CI** — already covered by the workflow; the baseline is now zero, so
  the ratchet is a pure regression gate.

## Order, cost, gates

| # | item | time | gate |
|---|---|---|---|
| 1 | B2 run + read the branch | 30 min | the firing branch named, recorded |
| 2 | B2 follow-up only if it is the field-order case | 1 h | flags agree; residency engages |
| 3 | B3 canary added to g0_gemm | 1 h | ok on both lanes |
| 4 | B3 prove it bites (vendored old parse) | 1 h | `G0_GEMM_CANARY=BAD`, `G0_GEMM_BAD=0` |
| 5 | B4 kiosk pin + ratchet + soak | 30 min | census 73/0/2196, soak flat |

## Doctrine notes

* **B2's value is the *answer*, not a fix.** Two sessions have guessed wrong about this
  path; a printed reason beats another hypothesis.
* **B3 is a check that has been seen to fail** — the same standard the D1 gate had to meet
  twice today (a bespoke fixture that passed both ways, then the real reproducer that
  failed correctly).
* **Order matters**: B2 before B4, because if residency is about to engage, the kiosk
  should be pinned on a compiler whose device path is understood.
