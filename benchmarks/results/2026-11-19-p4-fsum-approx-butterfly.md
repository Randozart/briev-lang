# `g0_fsum_approx` OpenCL: FIXED by the portable workgroup butterfly (P4)

**2026-11-19.** Supersedes the refuted 2026-11-18 workgroup-scope candidate
(`2026-11-18-fsum-approx-opencl-wrong.md`); that file's static diagnosis stands,
its mechanism was reverted, and this is the mechanism that survived the device.

## The fix (landed `6e147ada2` + `7ae635eb5`)

The cooperative 32-lane row-fold / softmax kernels lower their cross-lane
reduce to a **portable workgroup shared-memory butterfly** instead of the
hardware `OpGroupNonUniform*`:

- module-global WG scratch `OpArray [32]` of the fold's element type
  (`declare_wg_reduce_scratch`, kernel.rs), declared only on the cooperative
  reduction path, plumbed as `wg_scratch: Option<Word>`;
- `emit_wg_butterfly` (lower.rs): `scratch[lane]=v` → `OpControlBarrier`
  → five `OpSelect`-guarded tree steps (`offsets 16,8,4,2,1`,
  `scratch[lane] = sel(lane < o, scratch[lane] OP scratch[lane+o], scratch[lane])`,
  barrier after each) → uniform `scratch[0]`;
- combine ops: FAdd/IAdd/IMul by operand kind, Min/Max via the GLSL helpers
  (`glsl_fmaxmin`/`glsl_smaxmin`). **`SubgroupFMul#` keeps the hardware form** —
  floating multiply is not exactly associative and no lane exercises it;
- GEMM/coopmat untouched (it returns before the cooperative path and emits its
  own `OpGroupNonUniform` at 256 lanes).

The second commit (`7ae635eb5`) fixes what the device caught and the opcode
census could not: the WG scratch `OpVariable` must be **listed in the
entry-point interface** — spirv-val names it
(`Interface variable id <24> ... is not listed as an interface`), PTL's Vulkan
driver surfaces it as `compute pipeline failed`. New permanent gate:
`test_p4_wg_fold_module_passes_spirv_val`.

## Device results (PTL, 2026-11-19, post-`7ae635eb5`)

`bash benchmarks/intel_ptl/g0.sh` → **G0: PASS**:

| gate | vulkan | levelzero | opencl |
|---|---|---|---|
| g0_elem | PASS 0.016 ms | PASS 0.158 ms | bad=0 SUM=ok |
| g0_gemm | PASS 0.193 ms | — (G3) | — (G3) |
| g2_gemm_tiled | PASS 0.283 ms | — | — |
| g0_sum (int IAdd butterfly) | PASS 0.241 ms | PASS 0.573 ms | **bad=0 SUM=ok** |
| g0_minmax (SMin/SMax butterfly) | PASS 0.275 ms | PASS 0.574 ms | **bad=0 MIN=ok MAX=ok** |
| g0_fsum_approx (FAdd butterfly — THE fix) | PASS 0.368 ms | PASS 0.300 ms | **bad=0 SUM=ok (was WRONG)** |
| g0_fsum_exact (no reduce op) | PASS 0.346 ms | PASS 0.287 ms | bad=0 SUM=ok |

OpenCL minmax is intermittent under node load (one suite run printed
`WRONG (bad=?)` from a 90 s timeout with no output; 5/6 direct runs print
`G0_MINMAX_BAD=0` with GPU==CPU, the 6th timed out) — the known NEO clFinish
flake (BUGS.md 2026-11-09), not a butterfly defect: every run that produced
output was correct.

### Licensed-lane timing (the 0.106 ms budget, `briev_lane.sh`, linked runner)

| lane | `ceil_reduce_approx_f16` (licensed → butterfly) | `ceil_reduce_f16` (unlicensed serial) |
|---|---|---|
| opencl | **0.067 ms** (budget ≤ 0.106) | 0.031 ms |
| vulkan | 0.132 ms (old hardware form recorded 0.305 ms on g0) | 0.159 ms |

The butterfly **beats the budget** on the lane it was built for (0.067 vs
0.106 ms) and does not regress the serial lanes. GEMM/elem ceilings spot-checked
unchanged (gemm_512_f32 0.303 ms, elem_f32 0.047 ms — matches 2026-11-09).

## Local gates

- `cargo test --lib`: **3943 passed** (incl. the rewritten fold-lane tests and
  the new spirv-val gate; GEMM suite 22/22).
- `bash scripts/gate.sh quick`: PASS.
- Praetor on `src/backend/spirv`: no NEW diagnostics (the `wg_combine`
  6-parameter Rule-4 warning fixed with the `WgCombine` context struct).
- spirv-val on the P4 fold module: clean (the new gate).

## Lessons

1. **The interface-list rule is a class**: any new module-scope `OpVariable`
   must be chained into the entry-point interface — the opcode census and even
   in-process `emit` tests cannot catch it, only spirv-val or the driver can.
   The new gate pins it for the butterfly; the same rule now guards future
   WG-storage variables.
2. The 2026-11-18 note "the exact lane emits no subgroup op" was the reason
   `g0_fsum_exact` never flaked — its structural isolation carried through P4
   unchanged.
3. Interleaved A/B caveat honoured: single runs of the OpenCL fold ranged
   0.111–0.194 ms with one 0.000 ms outlier (device-idle artifact); the
   median-of-5 `briev_lane.sh` linked-runner protocol is the number above.
