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
