# PTX Tier Execution — S1 driver + S2 emitter

**Date:** 2026-09-08
**Design:** `docs/plans/2026-09-04-beyond-coopmat.md` §Stage 2 (RE-ARMED 2026-09-08, condition #2 met)
**Doctrine:** `docs/architecture/abv-gpu-doctrine.md` — Briev-owned codegen only, no nvcc/cuBLAS in the compiler path.

## Baseline (Rule 12 — recorded BEFORE changes)

Source: `docs/plans/2026-08-31-vitriol-gemm-comparison.md` ledger + this session's GEMM campaign closure.

| cell | number |
|------|--------|
| GEMM f16 4096³ SPIR-V tensor tier (R=4, smem, fused fill) | **0.708 ms avg / 24.3 TFLOP/s** (95% of 25.6 HW FP16-acc peak) |
| Portable tier structural limit (Stage 1 exhaustion, 2026-09-08) | 4.55 ms / 30.2 TFLOP/s |
| mma m16n16 coopmat ceiling (L2-load-bound) | 50.1 ms/launch, ≥107 TFLOP/s mma rate |
| ggml-cuda 4096³ f16 (same box, device 0 = RTX 3060) | 3.2 ms / 42.0 TFLOP/s |
| **S5 gate** | match 42.0 TFLOP/s at 4096³, then beat |

Target GPUs: RTX 3060 (GA106, sm_86), GTX 1070 Ti (GP104, sm_61) — both present
(Device 0 / Device 1). Driver 580.178.04, CUDA 13.0, libcuda + ptxas present.

## S1 — CUDA driver module `lib/runtime/briev_dev_cuda.c`

Mirror `briev_dev_vulkan.c`'s shape (dlopen, dlsym table, no CUDA headers —
raw numeric enums verified against `cuda.h`):

1. **dlopen** `libcuda.so.1`; resolve driver API: `cuInit`, `cuDeviceGetCount`,
   `cuDeviceGet`, `cuDeviceGetName`, `cuDeviceGetAttribute`, `cuCtxCreate`,
   `cuModuleLoadData` (PTX text → module; driver-internal ptxas = the JIT),
   `cuModuleGetFunction`, `cuMemAlloc`, `cuMemFree`, `cuMemcpyHtoD`,
   `cuMemcpyDtoH`, `cuLaunchKernel`, `cuStreamCreate`, `cuStreamSynchronize`,
   `cuFuncSetAttribute` (CU_FUNC_ATTRIBUTE_MAX_DYNAMIC_SHARED_SIZE_BYTES).
2. **BrievDeviceDriver** registration: `available()` (dlopen + cuInit + a
   compute-capable device), `init()`, `create_kernel(ptx_bytes, size, ...)`
   (module + function), `launch`, `launch_dev`, `launch_dev2d`,
   `launch_dev2d_batch`, `download_dev`, `device_name`, shutdown.
3. **Projection semantics** identical to Vulkan: kernel gets ONE flat buffer
   (the packed `%State` projection). CUDA kernel signature: `f(const float* proj)`
   with a `proj_bytes`-sized `__constant__`-like read? No — pass the pointer as
   kernel param (single param). GEPs match the SPIR-V field offsets (arrays
   then scalars, name-sorted, `proj_offset` per field).
4. **Residency**: `cuMemAlloc` VRAM working set + `cuMemcpyHtoD` staging sync
   (dirty ranges) + `cuMemcpyDtoH` download — same contract as `launch_dev2d`.
5. **LocalSize**: PTX uses block dims at launch; default 64×1×1 (keep in step
   with `VK_LOCAL_SIZE_X`), overridable later via config.
6. **Init-time diagnostic** if `ptxas`/JIT absent: the driver's module load
   fails → chain falls through (verbose prints the CUDA error string via
   `cuGetErrorString`).

**Chain order:** `{ &briev_dev_cuda, &briev_dev_vulkan, &briev_dev_opencl }` —
CUDA first (the perf tier), `BRIEV_ACCEL_DEVICE` env override unchanged. The
SPIR-V path stays byte-identical when the probe selects Vulkan.

**Gate:** a hand-written PTX saxpy blob runs through `create_kernel` + `launch`
in a scratch C harness and matches the host reference. Then the REAL test:
existing `.abv` runner compiled with the PTX path? NO — the runner emits SPIR-V
blobs; the PTX path needs the S2 emitter to produce PTX. So S1's correctness
gate is the hand-written PTX harness (mirrors how the coopmat tier was proven:
general path first).

## S2 — PTX emitter `src/backend/ptx/`

Consumes `GemmPlan` + tier eligibility + dispatch predicates (frontend-driven,
backend-contracts §2). Surface declared in `capabilities.rs`. `--backend ptx`.

- **S2a** minimal: non-tensor GEMM/naive + saxpy-shaped kernels in PTX, enough
  to route an existing `.abv` benchmark through the CUDA driver end-to-end and
  match SPIR-V output (the correctness bridge before tensor ops).
- **S2b** tensor family (S3): `mma.sync.aligned.m16n8k16`, `ldmatrix`,
  `cp.async` — per-op microtests first.

## Gates

| Step | Gate |
|------|------|
| S1 | PTX saxpy harness: exact vs host reference on BOTH devices (3060 + 1070 Ti) — **PASS (RTX 3060, 1D + 2D + batch)** |
| S2a | `--backend ptx` routes an existing GEMM `.abv`; y == SPIR-V path (rel 0.0) — **PASS (gemm_bench on PTX blob: max_rel_err 0.000e+00 @4096³)** |
| S3 | single-mma / single-ldmatrix microtests with known fragments — **PASS** |
| S3b | full tensor GEMM kernel into `--backend ptx` — **PASS (f32-y and f16-y exact vs double ref across 16/48/64/96/128/256 shapes; end-to-end runner dispatches once + fast-forwards)** |
| S3b | full tensor GEMM kernel into `--backend ptx` — **PASS** |
| S3b+ | smem-staged kernel (ldmatrix.x4 + ldmatrix.x2.trans) exact across 32×16×16/32×32×16/64×16×16/64³/128³/256³ — **PASS (2026-09-09; rel 0 vs double ref)** |

### S3b+ fragment fixes (2026-09-09, device-verified)

The smem-staged kernel shipped two layout bugs invisible to all-ones / periodic
seeds, found by seeding A/B with non-periodic `(j%23)+1` f16 values:

1. **A fragment (ldmatrix.x4)**: the x4 yields the four 8×8 tiles as
   `{M0=rows0-7/c0-7, M1=rows0-7/c8-15, M2=rows8-15/c0-7, M3=rows8-15/c8-15}`.
   The mma.m16n8k16 A operand interleaves rows with k-halves — `a1` must be
   the OTHER row-block's k0-7 (M2), `a2` this row-block's k8-15 (M1). Fix:
   swap a1↔a2 and a5↔a6 after each x4. (Seeded A-side was ~5% off.)
2. **B fragment (ldmatrix.x2.trans)**: the x2.trans's second 8×8 (`b1`) is the
   COL-shifted +16-byte tile (side-by-side), so `b1 = {B[2t][8+g]}` — but the
   mma needs rows 8-15 `{B[2t+8][g]}`. Fix: store the B tile as 2×2 8×8 blocks
   (ng=0 block in bsmem rows 0-7, ng=1 in bsmem rows 8-15) and use ldmatrix
   bases bsmem / bsmem+256. The old `(j%5)*0.5` seeds coincidentally satisfied
   `B[k][8+g]==B[k+8][g]`, masking this at 32×16×16; the 64³/128³ failures
   traced to the same root cause.

### S3b+ perf baseline (Rule 12 — recorded BEFORE the perf rungs)

`gemm_h_ptx_timed` harness (resident path, `launch_resident_2d(idx, state, 32,
count/512)`, 30 iters after 5 warmup, RTX 3060):

| Kernel | 4096³ f16 |
|--------|-----------|
| PTX smem-staged tensor (32×16 tile, 1 warp, single-buffer, R=1) | **64.98 ms avg / 2.11 TFLOP/s** |
| SPIR-V tensor tier (R=4, smem, fused fill) | 0.708 ms / 24.3 TFLOP/s |
| **S5 gate** | 42.0 TFLOP/s |

The 2.11 TFLOP/s is the arithmetic-intensity floor: R=1 gives ~10.4 FLOP/byte
(≈3.7 TFLOP/s HBM ceiling), and the single-warp single-buffer loop has no
overlap. The rungs below (register blocking R≥2, cp.async multi-stage,
multi-warp CTA) target the gap to the SPIR-V 24.3 and the 42 gate.

### S3b+ MW kernel correctness + dispatch wiring (2026-09-10)

**B-fill row-byte doubling bug** (commit `1a85a575`): the mw emitter's
B-fill computed `global = rd3 + (kstep*b_row + B_row*b_row + col)*2`,
doubling the row terms that are already bytes (`b_row = N*2 = 256`).
Fix: split row and column — only the column part `(slice*64 + b*8 +
c_dest%8)` gets `*2`. Verified exact: mw(1,1) 64³, mw(4,2) 128³×64K.

**Dispatch wiring**: `tensor_gemm_ptx_smem_mw` now called from
`build_ptx_kernels` via `select_mw_nw` heuristic (scales nw then mw,
caps at 1024 threads). Multi-warp dispatch formula corrected from
`n/16` to `n/64` (CTA covers mw\*32 × nw\*64 = mw\*nw\*2048 elements,
64 per thread). Grid: `gx = M*N/(64*block_threads)`, `gy = 1`.

### S3 fragment layout (device-verified, RTX 3060, exact rel 0)

`mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32` — the locked lane mapping:

- **A fragment** (4 × .b32, each packs 2 f16):
  `reg0={A[g][2t],A[g][2t+1]}  reg1={A[g+8][2t],A[g+8][2t+1]}  reg2={A[g][2t+8],A[g][2t+9]}  reg3={A[g+8][2t+8],A[g+8][2t+9]}`
  (rows interleave with k-halves: reg1 is the OTHER ROW's k0-7, not this row's k8-15.)
- **B fragment** (2 × .b32): `reg0={B[2t][g],B[2t+1][g]}  reg1={B[2t+8][g],B[2t+9][g]}`
- **C/D fragment** (4 × .f32): `reg0=C[g][2t] reg1=C[g][2t+1] reg2=C[g+8][2t] reg3=C[g+8][2t+1]`

where `g = lane>>2`, `t = lane&3`. Address rule: `row stride × row + col × elem` — the row stride is ALREADY in bytes (32 for A/C, 16 for B); never left-shift the row part. Both the driver-JIT path and the pre-compiled cubin path load and run exactly.

Debug pitfalls recorded: (1) a naive hand-rolled f16 encoder corrupts 0.0/small values (exp underflow wraps to 0x4000=2.0) — use proper RNE; (2) the "driver-JIT rc 218" was a symptom of a bad-address PTX, not a driver version issue — both JIT and cubin work once the PTX is correct.
| S4 | shape portfolio correctness (2048³/4096³/8192³/skinny-K/small), tier gates 5e-3/1e-2 |
| S5 | ≥42.0 TFLOP/s at 4096³ (ledger row) |
| S6 | `derive --stochastic` tile sweep → config cache |

### S4 gate — PASS (2026-09-10)

All shapes pass the 5e-3 f32-acc tier gate (externally verified via
CUDA driver API harness `test_s4.c`):

| Shape | mw×nw | block_threads | gx | max_rel_err | Gate |
|-------|-------|---------------|-----|-------------|------|
| 256×128×64 (small) | 8×2 | 512 | 1 | 0.000e+00 | OK |
| 2048³ | 2×8 | 512 | 128 | 3.261e-04 | OK |
| 4096³ | 2×8 | 512 | 512 | 2.442e-04 | OK |
| 8192³ | 2×8 | 512 | 2048 | 3.254e-04 | OK |
| 4096×4096×16 (skinny-K) | 2×8 | 512 | 512 | 0.000e+00 | OK |

**Register budget fix:** `select_mw_nw` capped at 512 block_threads
(108 regs × 512 = 55,296 ≤ 65,536 per-SM on sm_86 GA106). Previously
tried mw=4×nw=8=1024 threads → 110,592 > 65,536 → CUDA_ERROR_LAUNCH_OUT_OF_RESOURCES.

**Dump tests** added: `dump_mw_2048`, `dump_mw_4096`, `dump_mw_8192`,
`dump_mw_4096_k16` in `tensor.rs:r16_dump`. 2106 lib tests green.

## Docs to update (with S2, same commit)

`docs/architecture/backend-contracts.md` (PTX charter row), `docs/HANDOFF-2026-08-31-gpu.md` (status block), `spec/SPEC.md` §9.8 (already written — verify), `AGENTS.md` reference index, `docs/plans/INDEX.md`.

## Undo

New module + new driver file only; SPIR-V path byte-identical under `--backend spirv` and under probe selection. Config changes reverted via git; losers get VERDICT rows in the ledger.
## S4 re-verification + cp.async pipeline — PASS (2026-09-10, later)

The S4 gate above was recorded from harness runs that today proved
unreliable (hand harnesses hit the high-VA driver trap, see BUGS.md
2026-09-10; one config also passed a 3.2e-3 seeded-data off-by-one under
the 5e-3 gate). Re-validated everything on the corrected kernel with a
retry-alloc (low-VA) harness:

**Kernel corrections (tensor.rs `tensor_gemm_ptx_smem_mw`):**
1. B slab made K-MAJOR (`k*128 + n*2`) with 16B-chunk XOR swizzle
   `((n>>3)^(k&7))<<4`; fill and ldmatrix share the contract. A 4-byte
   cp.async sources two adjacent columns of one B row — an n-major tile
   was unsourcable, period.
2. KLOOP fill prefetches stripe kstep+16 with a `FILL_DONE` skip on the
   final iteration (an off-by-one reused stripe k, silently doubling
   stripe 0 and dropping the last; and skipping unguarded read past K).
3. Prologue B fill uses `k*b_row` (r2 is NOT kstep during the prologue —
   it still holds a setup product; the old `(kstep+k)` form read
   `B[(n_cta*512+k)*b_row + ...]` — massively out of bounds).
4. `membar.cta` between `cp.async.wait_group 0` and `bar.sync` — on
   driver 580.178/sm_86 the documented wait+bar pattern alone let
   ldmatrix read stale smem natively (zeros out; masked by cuda-gdb and
   compute-sanitizer). See BUGS.md 2026-09-10.

**Register budget (mod.rs `select_mw_nw`):** cap 512 → 256 threads.
Natural allocation is 136 regs; `-maxrregcount=108` cubins fault IMA on
both ptxas 12.8 and 13.3, so capped cubins are unshippable — the thread
cap carries the budget instead. Growth made balanced (nw-first
alternation): on-device sweep 4096³ gave (1,8) 8.9, (2,4) 17.3,
(4,2) 17.4 TFLOP/s.

**Re-verified S4 (2×4, block 256, low-VA harness):**

| Shape | mw×nw | gx | max_rel_err | TFLOP/s |
|-------|-------|-----|-------------|---------|
| 2048³ | 2×4 | 256 | 3.261e-04 | 15.01 |
| 4096³ | 2×4 | 1024 | 2.442e-04 | 16.98 |
| 8192³ | 2×4 | 4096 | 3.254e-04 | 17.84 |
| 4096×4096×16 | 2×4 | 1024 | 0.000e+00 | 1.16 (launch-bound) |

vs pre-cp.async baseline 9.55/10.44/~10.5 — **+60–70%**. S5 (42
TFLOP/s) still open: next levers are bank-conflict audit on the swizzle,
wider tiles via ≤128-reg kernel trims, and occupancy tuning.

2106 lib tests green.

## Perf rungs on the staged mw kernel — 17.8 → 18.3 TFLOP/s peak (2026-09-10, evening)

Post-S4 re-verification, four rungs landed on the same day (all on-device
A/B'd at 4096³ unless noted):

1. **Compute scheduling trim (136 → 128 regs, 0 spills):** A fragments
   load per-mh (4 live, not 8), each B fragment loads immediately before
   its mma (2 live, not 16). Pure scheduling — identical math per
   accumulator.
2. **4-stage cp.async pipeline + dynamic shared:** one outstanding fill
   could not hide DRAM latency; stages now live in ONE `.extern .shared`
   array sized (mw·1024 + nw·2048)·4, plumbed end-to-end
   (RunnerKernel.shared_bytes → BrievKernelDesc → set_shared_bytes →
   cuFuncSetAttribute + launch sharedMemBytes). Prologue fills stages
   0..S-2; the KLOOP fills stage (i+S-1)&S-1 with stripe
   kstep+16·(S-1), guarded skip past K; `wait_group S-2`.
3. **Coalesced fill mapping:** D = tid·4 + j·threads·4 (consecutive lanes
   touch consecutive 4B) — the per-thread-stride mapping read 4B out of
   separate 32B sectors. 8192³ 17.7 → 18.3; 2048³ 15.0 → 15.8.
4. **Config sweep (256 vs 512 threads):** with 128 regs, (2,4)@256T keeps
   TWO CTAs per SM (memory parallelism) and beats (4,4)@512T (one CTA):
   18.4 vs 16.2 @4096³. select_mw_nw capped back to 256 threads with the
   measurement recorded; balanced growth lands on (2,4).

| Shape | (2,4) 4-stage coalesced | gate |
|-------|-------------------------|------|
| 2048³ | 15.81 TFLOP/s | 3.261e-04 OK |
| 4096³ | 17.34 TFLOP/s | 2.442e-04 OK |
| 8192³ | 18.27 TFLOP/s | 3.254e-04 OK |
| 4096×4096×16 | 1.1 (launch-bound) | 0.000e+00 OK |

vs synchronous-fill baseline 9.55/10.44/~10.5 — **+66–74%**. SPIR-V tier
(~30) and ggml anchor (42) still ahead: next levers are the D2-style
register-prefetch fill (fill loads issue at loop top, smem stores after
the barrier) and an A-panel L2 sweep (launch order so consecutive CTAs
share A). 2106 lib tests green.

## Evening session: two VERDICT-REJECTED experiments + DVFS caveat (2026-09-10)

1. **N-major CTA rasterization — REJECTED.** Swapping the CTA decode so
   consecutive CTAs share the B panel (2MB, L2-hypothesized) measured
   16.6/16.2 vs m-major 17.3/18.3 @4096³/8192³. The concurrent 56 CTAs
   span TWO B panels (4MB > 3MB L2) and A re-reads (16×) thrash — the
   traffic model ignored that both panels cannot simultaneously fit.
   Reverted.
2. **Loop-invariant hoist of the compute address math — REJECTED.**
   Halving the per-iteration instruction count (~200 → ~90) DROPPED
   8192³ 18.3 → 16.4: the redundant uniform address math was filling
   issue slots that now stall on ldmatrix/mma dependency chains. ptxas
   was already scheduling correctly; instruction count was not the wall.
   Reverted; the comment in `tensor_gemm_ptx_smem_mw` records it.

3. **DVFS caveat:** sustained-load windows read 5-10% below fresh-boost
   windows on the same binary (16.6 vs 18.3 @8192³, clocks 1612MHz
   unlocked, root lost 09-02). Cross-window TFLOP/s comparisons are ±10%.
   Same-window interleaved A/Bs remain valid (all verdicts above are
   same-window).

Compute-ceiling measurement: with all fills stripped the kernel runs
6.06ms (22.7 TFLOP/s) vs 8.2-8.4ms full — the fill+DRAM side costs ~28%
on top of the ldmatrix/mma/barrier path, i.e. the 4-stage pipeline hides
most but not all of the fill. Remaining gap to SPIR-V (~30) and ggml
(42): fill-side efficiency (D2-style register prefetch is the named
next experiment) and, beyond that, the f32→f16 accumulation contract.

## f16-acc contract tier implemented — VERDICT: gated OFF (fill-bound) (2026-09-10, night)

The f16-acc variant (`ptx_tensor_f16acc`, default 0) is implemented and
correct: mma.sync.f16.f16.f16.f16 with f16x2 packed accumulators (64 f32
acc regs → 32 b32), 16-iteration (256-k) chunks promoted into the
CTA-private f16 y tile via read-modify-write (y zeroed by the kernel
prologue; each thread RMVs exactly its own fragments — no atomics, no
cross-thread hazard). Precision measured **8.14e-04** @4096³ — an order
under the 1e-2 contract gate (chunk-internal f16 walk + per-chunk f16
rounding both bounded).

Measured @4096³: **13.6 TFLOP/s vs f32-acc 16.6 same-window — a
regression.** Cause: the kernel is FILL-DRAM-bound (~2.7GB traffic,
~90% of peak), so the 2× mma rate buys nothing while the y RMV adds
~0.5GB (+19%) traffic. The packed accumulators DID cut registers
128 → 64, funding (4,4)@512T×2-CTA (24KB smem × 2-stage) — but the
2-CTA sweep verdict repeated: even maximal occupancy does not beat the
fill wall.

**What f16-acc is for:** the moment the fill becomes subordinated
(smaller tiles + deeper reuse, or an L2-friendlier problem), the 2×
mma rate is the only path to the 42-TF anchor (whose kernel is
f16-acc double-pumped). The infrastructure is config-gated and ready —
flip `ptx_tensor_f16acc=1` once a traffic rung lands. Config sweep for
the f16acc path selects (4,4)@512T automatically.

2107 lib tests green.

## Evening: fill/compute decomposition + pipelined B + driver-JIT wedge (2026-09-10, late)

**Decomposition @4096³ (2,4) f32:** fills stripped → 6.06ms (22.7
TFLOP/s compute ceiling); fills only → 4.39ms (31.3 TFLOP/s — the L2
sharing across concurrent CTAs lifts effective fill bandwidth to
~610GB/s, i.e. DRAM is NOT the wall the models assumed); full →
8.25ms. The 4-stage pipeline hides ~72% of the fill; the residual gap
is compute-phase dependency stalls (the failed hoist confirmed
latency-bound, not issue-bound).

**B-fragment software pipeline (f16acc):** ldmatrix→mma serialized
per-g through the shared %b0/%b1 pair. f16acc's 68-reg budget funds 4
B regs: preload g0/g1, then mma(g) alternates pairs while the ld-ahead
for g+2 issues — removes the per-g serialization. f32 stays serial
(its exact 128-reg 2-CTA budget cannot fund +2 regs). RMV chunk
widened 16 → 32 iterations (8 f32 rounding, ~2e-3 projected).

**Driver-JIT wedge (environment):** the driver's PTX JIT (rc 218,
INVALID_PTX) began failing on kernels it had JIT'd cleanly hours
earlier — deterministic per-binary within a window, trivial PTX still
JITs. Hundreds of faulted contexts today (all the IMA debugging)
degraded it. triton ptxas 13.3 assembles everything at 128/64 regs 0
spills, so the PTX is legal; validation of the pipelined B is blocked
until the driver is reloaded (sudo rmmod/modprobe nvidia_uvm or
reboot). Production runtime hardening landed regardless:
cuModuleLoadDataEx + CU_JIT_MAX_REGISTERS when the PTX carries
.maxnreg (directive stripped before JIT — the driver JIT rejects the
directive text itself), and the batch-loop shared-bytes fix (literal 0
→ k->shared_bytes — the production IMA).

2107 lib tests green.

## 2026-09-11: cubin shipping + canonical measurement contract (plan)

**Canonical measurement (user-set power cap):** both GPUs are power
limited to **110W**. Sustained-load clocks are therefore the honest
steady state; early-window boost numbers (~+10-15%) are not comparable
across sessions. Ledger rule going forward: warm the GPU to steady
state (a ~2s burn) before timing; record the sustained number.

**Why the driver JIT is the problem (measured):** production ships PTX
text; the driver's internal ptxas (13.0-era) compiles it at load. Three
failures: (1) CU_JIT_MAX_REGISTERS ignored — requested 128, got 166
regs → 1 CTA/SM, −27% vs the offline-ptxas 128-reg cubin; (2) the JIT
rejects the `.maxnreg` directive text itself (rc 218); (3) after today's
fault storms the driver JIT returns rc 218 on kernels it compiled hours
ago (deterministic per-binary; trivial PTX still compiles — wedged
state, cleared only by driver reload).

**The run: ship cubins.** `build_ptx_kernels` compiles the emitted PTX
through offline ptxas (PATH, `$TRITON_PTXAS`, or the triton install
path) and ships cubin bytes as the kernel blob; graceful fallback to
PTX text when ptxas is unavailable. `cuModuleLoadData` loads cubins
without invoking the driver JIT — no wedge exposure, no version skew,
and the build-time register contract (`.maxnreg`, validated at compile)
is exactly what runs. Config knob `ptx_emit_cubin` (default on).

Validation: unit check (ELF magic when ptxas present) + end-to-end
`gemm_h_bench <f32.cubin>` on-device at power-steady state; both
correctness gates; f32 (2,4)@128 and f16acc (4,4)@64 sweep.

## 2026-09-11 morning: cubin shipping live; f16acc fragment-layout census

**Cubin shipping LANDED:** `build_ptx_kernels` compiles the emitted PTX
through offline ptxas (flag-form `-maxrregcount`: the `.maxnreg` PTX
directive is rejected by every local ptxas) and ships cubin bytes;
PTX-text fallback when ptxas is absent (`ptx_emit_cubin`, default on).
The production blob path measured end-to-end on-device:
**f32 (2,4)@128 regs = 16.38 TFLOP/s, 2.4e-04, through
`gemm_h_bench` on BRIEV_ACCEL_DEVICE=cuda — bypassing the wedged driver
JIT entirely.**

**f16acc (4,4)@64 regs × 2-stage × 2-CTA measured: 18.4–18.5 TFLOP/s**
(pipelined B, 32-iter chunks) — the fastest configuration — but the S4
portfolio census exposed an **f16x2 accumulator fragment-layout
discrepancy**: at 4096×4096×16, m=0 n=8..15, got pairs = ref pairs of
ADJACENT column-pairs permuted ({ref3,ref4} at n8-9, {ref0,ref1} at
n10-11, …) — the register-half → (row, col) mapping I assumed
({row g: cols 2t,2t+1}, {row g+8: same}) does not match the hardware
layout for .f16-acc mma. K-sweep error ∝ 1/K (0.226 @K=16 → 1.4e-03
@K=4096) = a constant absolute displacement per element, consistent
with a fixed permutation. Identity-matrix probe + PTX ISA doc check
needed (next session). **f16acc stays config-gated OFF; f32-acc ships.**

Also: 110W power caps on both GPUs — sustained-state benchmarks are the
canonical ledger numbers (boost windows read +10-15%); benches warm to
steady state before timing.

2108 lib tests green (k-sweep dump tests, cubin ELF unit test added).

## 2026-09-11: f16acc fragment-layout mystery SOLVED — B-pipeline pair bug

The "f16x2 accumulator fragment-layout discrepancy" was NOT a fragment
layout issue. The identity probe (prime-coded seeds: y[m][n] decodes
uniquely to its own coordinates) showed the promotion ADDRESSES were
correct — the accs held the wrong products. Root cause: the pipelined-B
ld-ahead refilled the OTHER register pair, the one mma(g+1) was about
to consume — clobbering g+1's preloaded fragment after every even g, so
every mma(g>=1) computed with group-(g+1)'s B. The 1/K-shaped sweep
error disguised it as chunk rounding (groups g and g+2 collide in the
16-prime probe cycle; at 0.25 seeds the collision read as a column
permutation). Fix (7fa5a42a): refill pair(g&1) — the pair mma(g) just
released. f32 path serial, never affected.

**Post-fix S4 portfolio (all green):** K-sweep 4096x4096xK f16acc:
K=16..256 EXACT (0.0); K=512..4096: 2.6e-3..1.4e-3 (chunk rounding,
under the 1e-2 gate). 2048/4096/8192: 3.8e-3/1.4e-3/1.6e-3. f32 path
unchanged (3.3e-4). 2111 lib tests green.

**Sustained perf (110W steady state, interleaved A/B):**

| shape  | f32 (2,4)@256T | f16acc (4,4)@512T | delta |
|--------|----------------|-------------------|-------|
| 2048^3 | 16.4-16.6      | 16.9-17.8         | +5%   |
| 4096^3 | 13.3-13.4      | 18.2-18.3         | +37%  |
| 8192^3 | 13.3           | 18.0-18.1         | +36%  |

f16acc is the fastest correct configuration on every shape. Default
flip = numeric-contract decision (1e-2 vs 5e-3 tier) — pending owner
call; the config knob (`ptx_tensor_f16acc`) fully selects it either way.

## 2026-09-11 later: three f16acc VERDICTs — (4,4)@2-stage@32-iter stands

1. **4-stage f16acc: REJECTED (−2.5%)**. 17.8 vs 18.2 TFLOP/s, 3
   interleaved reps. The (4,4)@512T mma phase (16 warps) already hides
   the fill latency behind 2 stages; deeper pipelining only lengthens
   the prologue. (f32 keeps 4 stages — its 256T mma phase is shorter.)
2. **chunk_iters 32→64: REJECTED (no effect)**. 18.2/16.9/18.0 at
   4096/2048/8192³ — identical to 32-iter within noise; the RMV pass
   overlaps fills, it is not on the critical path. Reverted to 32.
3. **Config re-probe post-pipeline-fix: (4,4) CONFIRMED**. (4,4)
   17.9-18.0 > (8,2) 17.2 > (2,8) 13.7 TFLOP/s at 4096³, 64 regs, 0
   spills. The pre-fix ranking was measured on broken kernels but the
   instruction schedule was identical — ranking holds.

Also: tharness_f16s2 gained MW_STAGES (smem must match the kernel's
stage count — 2-stage allocation on a 4-stage kernel faults IMA on the
stage-2/3 fills).

## 2026-09-11 end: f16acc through the PRODUCTION runtime — 19.3 TFLOP/s

The full production chain validated on-device for the f16-acc tier:
config knob → select_mw_nw (4,4)@512T → compile_cubin → blob →
briev_accel_rt batched submission (gemm_h_bench
BRIEV_ACCEL_DEVICE=cuda, MW_BT=512 MW_SMEM=24576):

| tier | per-call | throughput | max_rel_err |
|------|----------|------------|-------------|
| f16acc (4,4)@512T cubin | 7.135 ms | **19.26 TFLOP/s** | 1.2e-3 OK |
| f32 (2,4)@256T cubin    | 10.344 ms | 13.29 TFLOP/s | 2.4e-4 OK |

Batched submission beats the direct-launch harness (18.2) — no launch
gap between iterations. **+45% sustained over the f32 default tier**;
the tier remains opt-in per the numerics decision (doctrine §3).
cuModuleLoadData sniffs ELF vs PTX, so the cubin blob needs no runtime
change. Sustained-state (110W) numbers throughout.
