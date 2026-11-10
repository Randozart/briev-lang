# Intel lane ceilings + compiler efficiency (2026-11-09)

**Status:** active. Builds on Stage F slice 1 (`2026-11-08-intel-g1.5-stage-f-slice1.md`).

**Unified program:** `docs/plans/2026-11-10-unified-program-capability-and-ceilings.md` — this plan is a detail of the one program (capability and ceiling meters + trust substrate).
**Objective:** make the compiler fast enough to iterate, and evaluate the Intel
OpenCL/Level-Zero lane **without mistaking our emitter's quality for the lane's
ceiling**.

## Why (the principle this plan encodes)

Numbers from Briev-generated kernels measure the **emitter**, not the lane. A
Briev-OpenCL loss today is a compiler work item (missing general machinery),
not evidence about NEO/Level Zero. Ruling a lane out requires the lane's *own*
ceiling to be below the other lane's ceiling at matched shapes, with evidence
and a documented non-closable gap. This is the performance analogue of
derivation-not-recognition: the ceiling must not depend on which kernels we
happened to write well.

## W0 — Baseline (recorded before changes)

Probe-mode `.bv` accel programs (the g0 fixtures, const N = 1048576):

| metric | before |
|---|---|
| `g0_elem.bv` IR | 641 MB, 13.6M lines |
| `g0_elem.bv` link | >45 min (not finished in 2700 s) |
| IR cause | `emit_probe_ok_body` unrolls one basic block per write-buffer element (`probe_ok_checks`) |
| `.abv` lane smoke | fast; L0 + NEO compile/launch, results correct |
| runtime behavior | unchanged target (Probe gate compares every element) |

Full-suite gate for the emitted-IR change: `cargo test --lib`,
`benchmarks/ll_determinism_check.sh --quick`, and
`benchmarks/build_and_bench.sh --runtime` for the runtime-parity table
(probe emission must not change program behavior or speed).

**W1 status (2026-11-09): DONE.** IR 641 MB → 204 KB, build >45 min → 78 s;
full element-for-element comparison preserved. Level Zero promoted to REQUIRED
for the five non-GEMM gates; OpenCL reported (exact-fold hang, BUGS.md).
Evidence: `benchmarks/results/2026-11-09-intel-lane-ceilings.md`.

## W1 — Probe gate: constant IR per write buffer

**Problem:** `probe_ok_checks` flattens every element into its own block; the
gate's IR is O(elements), which is why the Intel `.bv` gates cannot link.

**Change:** emit a runtime loop per write buffer (scalar fields stay direct):

```
entry → header(k=0) → body(load a[k], load b[k], compare, br fail/cont)
                    → cont(k+1) → header
                    → done → next buffer … → ok.final
```

IR becomes O(#write buffers); runtime cost stays O(N) (one pass, vectorizable).

**Acceptance**
- IR for a 1M-element write buffer contains `phi i64` and no per-element label;
  IR size < 5 MB on `g0_elem.bv`; link < 120 s; output identical.
- Unit test: synthetic accel entry with `[1048576 x i64]` writes emits a
  bounded IR (assert no `ok.<large>` label, assert loop header/phi).
- Existing probe tests updated; `cargo test --lib` green.
- On-device: all six `g0.sh` fixtures pass on Vulkan; then L0/NEO smoke and the
  six gates (lane promotion only after correctness).

## W2 — Lane ceilings (four numbers, not two) — COMPLETE SET

### W2b — OpenCL C ceiling harness (primary)

`benchmarks/intel_ptl/ceiling/`:
- `cl_api.h` — minimal OpenCL typedefs/prototypes loaded via dlopen
  (`libOpenCL.so.1`; the node has no CL headers). Mirrors
  `lib/runtime/briev_dev_opencl.c`.
- `harness.c` — generic single-kernel runner:
  - loads any SPIR-V (hand-built reference or Briev OpenCL image),
  - creates a profiling command queue (`CL_QUEUE_PROFILING_ENABLE`),
  - packs inputs, warmup 5 + ITERS 20, **device time from event
    timestamps** (primary) and wall-clock (secondary),
  - correctness against a double-precision CPU reference (relative error,
    exact for integer shapes), sampled + full for small shapes,
  - reports min/median/spread; fails on wrong output.
- `kernels/` hand-written OpenCL C references:
  - `selfadd_f32`, `selfadd_f16`
  - `rowsum_f32`, `rowsum_f16` (serial + subgroup variants)
  - `gemm_512_f32`, `gemm_4096_f32` (tiled, register-blocked)
  - `gemm_512_f16`, `gemm_4096_f16` (fp16 storage/compute where supported;
    fall back to fp16-storage/fp32-accumulate and record it)
  - `copy_bw` and `flux` probes to pin the measured bandwidth/FLOPs ceilings.
- `run.sh` — compiles references (`clang --target=spirv64 -x cl`), runs
  reference and Briev blob through the SAME harness, emits a table/JSON row.

The same harness also runs the **Briev OpenCL image** (state ptr + n ABI,
Briev projection layout) so Briev-vs-ceiling uses identical timing and
packing. It doubles as the exact-fold hang diagnostic (IGC vs runtime).

### W2c — Vulkan GLSL ceiling harness

Same directory, `vk_harness.c`:
- minimal Vulkan compute: instance/device/queue, storage-buffer descriptor
  (set 0, binding 0), pipeline from any SPIR-V, dispatch,
  **timestamp queries** for device time; CPU reference correctness.
- `kernels/*.comp` hand-written GLSL mirrors of the W2b set (f32 + f16 via
  `GL_EXT_shader_16bit_storage`/`float16` where supported; record caps).
- Runs both the GLSL reference and the **Briev Vulkan image** (entry `main`,
  descriptor SSBO) through the same harness.

### W2d — Matrix + attribution

Shapes × dtypes: elementwise 1Mi, reduce 4096×1024, GEMM 512³, GEMM 4096³;
f32 and f16 (BF16 as a follow-up row). Columns: Vulkan ceiling (GLSL),
OpenCL ceiling (CL C), Briev-Vulkan, Briev-OpenCL. All device-time, same
packing/iteration policy, correctness-gated.

Then the attribution checklist must be answered before interpreting:
residency present? state addressing cost? vector width? subgroups? local
size? tiling/staging? same algorithmic shape as the reference?

### W3 — close the gaps the matrix exposes

- L0/OpenCL residency (`launch_dev`, `launch_dev2d`, batched dispatch) so
  end-to-end application numbers do not pay a full projection copy.
- Exact-fold `clFinish` hang (BUGS.md): harness-first isolation, then fix.
- OpenCL state addressing vs natural-layout experiment (one staging layout).
- Subgroup/vec4/local-size parity if the matrix shows a gap.

### W4 — promotion

Promote OpenCL to REQUIRED only when: W2 matrix exists, the exact-fold hang
is fixed, and the five non-GEMM gates pass repeatedly on the lane. Record
per-lane evidence before promotion.

## Sync with main

The `main` worktree (`../briev-lang`) is shared with another session. Protocol:
- Merge `main`'s **committed** tip into `intel-xpu-harvest` at each milestone
  (never touch the other worktree's uncommitted files; never stash/checkout).
- Resolve conflicts in favor of BOTH intents: keep main's refactors and this
  branch's Intel/probe changes; run `cargo test --lib` after every merge.
- Record the merge commit and base in this plan's results file.

## W2 — original framing (kept for reference)

Fixed kernel set: elementwise self-add, reduction (sum/minmax), GEMM 512³,
GEMM 4096³ (4096³ is gated by G3 only for the *coopmat* path; a SIMD reference
still gives a lane floor).

Per kernel:

| number | source |
|---|---|
| Vulkan ceiling | hand-written GLSL compute reference |
| OpenCL/L0 ceiling | hand-written OpenCL C reference |
| Briev-Vulkan | Briev `.bv`/`.abv` (existing lane) |
| Briev-OpenCL | Stage F projection |

Staged to avoid building two harnesses before they can inform anything:

- **W2a** roofline bounds (bandwidth / peak FLOPs) + Briev-vs-Briev with the
  attribution checklist. Cheap; tells us whether a ceiling harness is needed
  for both lanes.
- **W2b** OpenCL C ceiling harness (`benchmarks/intel_ptl/ceiling/`): direct
  libOpenCL dispatch, GPU timestamp timing, C references for the four kernels.
- **W2c** Vulkan GLSL ceiling harness, same shape, if W2b shows the OpenCL
  ceiling is competitive and Briev-Vulkan is the number to beat.

**Attribution checklist** (must be answered before any verdict): residency
(`launch_dev*` present?), state access (byte-offset PtrAccessChain cost),
vector width / vec4 groups, subgroup ops, local size / occupancy, tiling /
stage depth, and whether the body uses the same algorithmic shape as the
reference.

**Rule-out criteria:** OpenCL ceiling < Vulkan ceiling on matched shapes AND
the gap survives the checklist AND no general pass can close it (documented
like a derivation-retirement gate). Otherwise the gap is a work item.

## W3 — Close the known emitter gaps

- Residency for L0/OpenCL (`launch_dev`, `launch_dev2d`, `mapped` semantics) so
  iterative kernels do not full-copy the projection per dispatch.
- State access cost: compare byte-offset PtrAccessChain vs a natural-layout
  struct for OpenCL-only modules; keep one host staging layout.
- Subgroup reduction / vec4 windows for the OpenCL flavor where W2a shows a
  gap vs the ceiling.
- Local size selection parity with Vulkan.

## W4 — Gate promotion

Promote `levelzero` and `opencl` to REQUIRED in `g0.sh` only when:
1. W1 lands (gates link in time), and
2. all six fixtures pass on each lane (correctness), and
3. the lane smoke remains correct after any W3 change.

Record per-lane outputs in `benchmarks/results/` before promotion.

## Verification & documentation

- `cargo test --lib`; `ll_determinism_check.sh --quick`; Praetor on changed
  dirs; `build_and_bench.sh --runtime` parity table in this plan's results file.
- Results: `benchmarks/results/2026-11-09-intel-lane-ceilings.md`
  (baseline + after, four-number matrix, attribution notes).
- Architecture docs updated in the same commit as structural changes:
  probe-gate emission rationale (`backend-architecture` if the section exists),
  `intel-dispatch.md` pointer to the results file, `INDEX.md` status.
- BUGS.md: the probe-gate O(N) IR as a fixed compiler-efficiency defect.

## Non-goals

- No strategy keywords for lanes; no vocabulary matching.
- No lane verdict until W2 evidence exists.
- No weakening of the probe gate's correctness (full comparison is kept).
