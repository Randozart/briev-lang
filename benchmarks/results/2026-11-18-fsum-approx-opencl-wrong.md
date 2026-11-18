# `g0_fsum_approx` is WRONG on the OpenCL lane: the reduce assumes subgroup == workgroup

**2026-11-18, device-free.** The G0 gate reported, on PTL/Arc B390:

```
-- g0_fsum_approx on levelzero: bad=0 SUM=ok
-- g0_fsum_approx on opencl:    WRONG (bad=?)
-- g0_fsum_exact   on opencl:   bad=0 SUM=ok
```

The two fixtures differ by **one keyword** (`approx` vs `exact`) and the data is
integer-valued in f32, so reassociation is bit-exact. The licence is therefore not
implicated: the OpenCL lowering is wrong.

## The evidence (static, reproducible here)

`scripts/extract_embedded_spv.py` pulls the `_ocl` flavour out of the emitted `.ll`
(the blob is a hex-escaped constant), then `spirv-dis`:

| | `g0_fsum_approx` | `g0_fsum_exact` |
|---|---|---|
| `OpExecutionMode … LocalSize` | **32 1 1** | 256 1 1 |
| reduce instruction | `OpGroupNonUniformFAdd %float %uint_3 Reduce %523` | none (per-item loop) |

`%uint_3` is **ScopeSubgroup** (0 CrossDevice, 1 Device, 2 Workgroup,
3 Subgroup). The kernel launches one 32-lane workgroup per row and reduces with
*subgroup* scope.

## Why that is wrong, and where it came from

`src/backend/spirv/lower.rs::emit_subgroup_reduce` hard-codes

```rust
let scope = self.builder.u32_const(spirv::Scope::Subgroup as u32);
```

So the kernel is correct **only when the device's subgroup size equals the
workgroup size (32)**. On the Vulkan/levelzero lane the preflight measured
`subgroupSize = 32`, so it passes. NEO's OpenCL subgroup width on Xe is smaller
(16 is the common value), so a 32-lane workgroup is two subgroups: each reduces its
own 16 lanes and both store to the same row — last writer wins, and the row total is
about half the true sum. That matches the observed `WRONG` while `exact` (no
subgroup op, `LocalSize 256`, plain loop) stays correct on both lanes.

## Candidate fix (NOT landed — unverifiable without the device)

Emit the reduce at **workgroup** scope (`Scope::Workgroup = 2`), which is what the
row kernel actually means: every invocation of the row's workgroup participates.
Where `LocalSize == subgroup size` (levelzero) this is semantically identical, so it
cannot regress that lane; where they differ (OpenCL/NEO) it fixes it. The
alternative — deriving the lane count from the device's subgroup size at dispatch —
keeps subgroup scope but makes the emitted `LocalSize` driver-dependent, which is a
frontend decision the backend must not guess.

Why not land it now: any change to kernel lowering needs the on-device gate (the
project rule that timing/correctness claims about generated kernels require a real
shape on a real device). A plausible-looking patch that *changes* the one lane known
to be correct, with no way to test it, is how the 16× dispatch bug survived for
weeks.

## Instrument

```bash
mkdir -p /tmp/spv && brievc build <fixture>.bv --accel=force --out /tmp/k
python3 scripts/extract_embedded_spv.py /tmp/k/<fixture>.ll /tmp/spv
spirv-dis /tmp/spv/k_<kernel>_ocl.spv -o /tmp/spv/k.spvasm
```

Works for any `.bv` accel program: the `_ocl` image is the OpenCL flavour, the
unsuffixed one is the Vulkan flavour.

## REFUTED ON DEVICE (2026-11-18): workgroup scope breaks both lanes

The candidate above ("emit the reduce at workgroup scope") was implemented, built, and
run on PTL — and it is **wrong**:

| `g0_fsum_approx`, `--accel=force --deny-accel-cpu-fallback` | `G0_SUM_BAD` | gpu_time |
|---|---|---|
| `Scope::Subgroup` (as shipped) | **0** | 0.305 ms |
| `Scope::Workgroup` (the candidate) | **220** | 0.021 ms |

The `BAD=220` case also reported `GPU_SUM=0 CPU_SUM=0 Y0=0 YN=0` — the kernel produced
nothing at all, on **both** lanes (levelzero too), so this is not "the OpenCL half-sum"
in a different disguise: `OpGroupNonUniformFAdd` with `Reduce` at workgroup scope does
not behave as the subgroup form with a wider participation set on these drivers. The
change is reverted; the shipped code is `Scope::Subgroup` (0.305 ms, `BAD=0`).

**What this leaves.** The static diagnosis stands (the OpenCL lane returns the wrong sum
where levelzero does not, and the only difference between the correct and incorrect
fixture is the reassociation licence). What is refuted is the *mechanism I proposed*.
The remaining candidate is to make the workgroup and the subgroup coincide instead of
widening the scope — emit the row kernel's `LocalSize` equal to the device's reported
subgroup width (a frontend/dispatch decision, not a lowering one), or lower the fold
through shared memory + `ControlBarrier` + a lane-0 store. Both need the device to
check; neither is a one-liner.

Lesson, again: a plausible static diagnosis is a hypothesis until a device runs it, and
this one took four minutes to disprove on the hardware that had been unavailable for
two days.
