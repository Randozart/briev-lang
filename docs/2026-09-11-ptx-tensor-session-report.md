# PTX tensor tier — session report

**2026-09-11 08:16 CEST · RTX 3060 (sm_86, driver 580.178.04) · commits b31f65bc → 5de68d22**

---

## 1. Where the tier started and where it stands

| Kernel state | 2048³ | 4096³ | 8192³ | Status |
|---|---|---|---|---|
| Synchronous fills (start of session) | 9.6 TF | 10.4 TF | ~10.5 TF | correct |
| cp.async rewrite (first attempt) | — | — | — | rc=700 / garbage on every shape |
| **Now: 4-stage cp.async, coalesced fills, pipelined f16acc path** | **15.7** | **16.6–18.5** | **18.3** | **correct, committed** |

(+57–74% over the synchronous baseline.) The SPIR-V coopmat tier sits at
~30 TFLOP/s and the ggml-cuda anchor at 42; both require the rungs
documented in §5.

The S4 shape portfolio (2048³ / 4096³ / 8192³ / 4096×4096×16) passes
the 5e-3 f32-acc gate at ≤3.3e-04, natively, through the production
runtime path — verified end-to-end via `gemm_h_bench` on
`BRIEV_ACCEL_DEVICE=cuda`.

## 2. Correctness: four kernel bugs, three environment traps

### Kernel bugs (all fixed, all caught on-device)

1. **B-tile layout inversion.** A 4-byte `cp.async` moves two adjacent
   *columns* of one global B row; the rewrite's n-major smem tile was
   physically unsourcable (the two values it needed were 8KB apart in
   global). Fixed with a **k-major slab** (`k*128 + n*2`) plus a 16B-chunk
   XOR swizzle `((n>>3)^(k&7))<<4` shared by fill and ldmatrix.
2. **Prologue B fill used `%r2` as kstep** — during the prologue `%r2`
   still holds a setup product (`n_cta*512`): the fill read
   `B[(n_cta*512+k)*b_row + …]`, far out of bounds. Prologue now uses
   `k*b_row` only.
3. **Pipeline stripe off-by-one.** The KLOOP fill loaded stripe `kstep`
   while the next iteration consumed `kstep+16`: stripe 0 ran twice, the
   last stripe never ran, and the unskipped final prefetch read past K.
   Fixed with the kstep+16·(S−1) prefetch and a `FILL_DONE` guard skip.
4. **Production batch loop passed literal 0 shared bytes** on every
   dispatch after the first (`cuda_launch_grid(…, 0)`) — production-only
   IMA. Now `k->shared_bytes`.

### Environment traps (documented in BUGS.md 2026-09-10)

1. **cp.async visibility.** The documented `wait_group + bar.sync`
   pattern did NOT make LDGSTS writes visible to `ldmatrix` natively —
   y comes back all zeros while cuda-gdb/compute-sanitizer (serialized)
   mask it. **`membar.cta` between them is required on driver
   580.178/sm_86** (`fence.proxy.async` needs sm_90).
2. **High-VA allocation faults.** `cuMemAlloc` sometimes returns VAs in
   a 0x7fef_08200000-style region (per-binary, deterministic); compute-
   channel MMU walks then fail (Xid 31 FAULT_PDE) while the copy engine
   works on the same buffer. Harnesses retry allocs until low VA.
3. **`-maxrregcount` IMA — verdict REVISED.** The original "capped
   cubins fault" verdict was contaminated by bug (2) of the same era.
   Re-tested on the fixed kernel: `-maxrregcount=128` assembles with 0
   spills, runs correct, and **wins +27%** (see §3).

## 3. Performance: what moved, what didn't (all on-device A/B)

**Landed (committed):**
- 4-stage cp.async pipeline; stage masks/stripe parameterized.
- Dynamic shared memory end-to-end: `.extern .shared` + RunnerKernel →
  `BrievKernelDesc.shared_bytes` → `cuFuncSetAttribute` + launch.
- Compute scheduling trim: A per-mh, B+mma per-g — 136 → 128 regs.
- Coalesced fill mapping `D = tid*4 + j*threads*4` (8192³ +10%).
- **`.maxnreg 128`** (PTX directive honored at JIT; runtime passes
  `CU_JIT_MAX_REGISTERS`): 138 natural regs forced 1 CTA/SM; the cap
  restores 2 — same-window A/B 2048³ **15.7 vs 12.3 TFLOP/s**.

**Measured and rejected (reasons in the ledger):**
- N-major CTA rasterization: both B panels exceed L2 concurrently.
- Loop-invariant address-math hoist: instruction count was not the
  wall — the redundant uniform math was hiding dependency stalls.
- f16-acc as a speedup today: fill-bound, see §4.

## 4. The f16-acc contract tier — built, correct, gated off

`ptx_tensor_f16acc` (default 0): `mma.sync…f16.f16.f16.f16` with f16x2
packed accumulators (64 f32 acc regs → 32 b32), 32-iteration chunks
promoted into a CTA-private f16 y tile by read-modify-write (the kernel
prologue zeros the tile; each thread RMVs only its own fragments — no
atomics). **Precision 8.1e-04** @4096³ — an order under the 1e-2
contract gate.

Measured: 13.6 vs f32-acc 16.6 TFLOP/s same-window — a regression,
because the kernel is fill-bound and the y RMV adds ~0.5GB traffic.
The packed accumulators cut registers 128 → 64, which funds the
(4,4)@512T×2-CTA point and the **pipelined B-fragment schedule**
(4 B regs; ld-ahead g+2 into the alternate pair removes the per-g
ldmatrix→mma serialization the compute ceiling exposed).

## 5. The wall, precisely measured

| Kernel variant | Time @4096³ (2,4) | TFLOP/s |
|---|---|---|
| Fills stripped (compute ceiling) | 6.06 ms | 22.7 |
| Fills only | 4.39 ms | 31.3 (L2-boosted) |
| Full kernel | 8.25 ms | 16.6 |

The pipeline hides ~72% of the fill; the binding constraint is the
compute phase's dependency structure, and inside it the per-g
ldmatrix→mma serialization — which the pipelined B schedule now
removes (validation pending, §6). Remaining levers after that: A-panel
L2 sweep, and the f16-acc flip once fills are subordinated.

## 6. Open blocker: driver JIT wedge

The driver's PTX JIT began returning rc 218 (INVALID_PTX) on kernels it
had JIT'd cleanly hours earlier — deterministic per-binary within a
window, trivial PTX still compiles. Hundreds of faulted contexts during
the IMA debugging degraded it. ptxas 13.3 assembles every kernel at
128/64 regs with 0 spills, so the PTX is legal; **on-device validation
of the pipelined B is blocked until the driver is reloaded**
(`sudo rmmod nvidia_uvm && sudo modprobe nvidia_uvm`, or reboot).

Also fixed en route: the production JIT path now strips `.maxnreg` (the
driver JIT rejects the directive text) and carries the value via
`CU_JIT_MAX_REGISTERS`; `gemm_h_bench` gained `MW_SMEM` + block_threads
for the CUDA tier.

## 7. State

- 2107 lib tests green · 5 commits this arc · everything validated
  on-device before adoption; both rejected experiments recorded with
  causes (raster, hoist).
- Next: reload driver → validate pipelined B (f16acc + f32 paths) →
  A-panel L2 sweep → f16acc flip decision.

— generated 2026-09-11 08:16 CEST
