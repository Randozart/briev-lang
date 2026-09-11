# GPU Offloading — `accel` Keyword, Device-Agnostic Runtime

**Date added:** 2026-06-18 · **Rewritten:** 2026-08-06 (accel plan,
Design A)
**Status:** Active — the `accel` keyword + `briev_accel_rt` runtime replace the
removed `#gpu` pragma family.

---

## Model

GPU offloading is requested with the **`accel` keyword** on a `node`/`txn`
(`accel node name [i < N][i == N] { ... }`) or the module shortcut
`!> accel: try_all;`. The work-item counter `i` is a REAL state field
(`let i: Int = 0;` + `i = i + 1`); `accel` marks the native counted loop as a
parallel map over work-items. The compiler PROVES the map (disjoint affine
per-`i` writes, counter advance, flat types) and then either coalesces the
loop into one GPU dispatch or runs it natively on CPU.

See `docs/plans/2026-08-06-accel-gpu-offload.md` (design + phases),
`docs/plans/2026-08-06-endprogram-beginprogram.md` (process-boundary
keywords), and SPEC §9.7.

---

## Compilation Flow

```
Briev source
    │
    ├──→ Frontend analysis (src/analysis/accel.rs) →
    │       For each accel candidate:
    │         1. Eligibility proof (bound, disjoint writes, purity, flat types)
    │         2. Cost model (crossover) → AccelDecision { Gpu | Probe | Cpu }
    │         Stored in AnalysisResults.accel (frontend-driven dispatch)
    │
    ├──→ Native CPU binary (LLVM backend) — always emitted:
    │       • the counted loop runs natively (each firing = one work-item)
    │       • Gpu/Probe bodies get a dispatch wrapper @txn_<name> that calls
    │         briev_accel_launch (with device/verdict gate) or @txn_<name>_cpu
    │       • per-node entry flags for beginprogram entry loops
    │
    └──→ SPIR-V kernel emission (src/backend/llvm/kernel.rs) →
            Reuses the LLVM expression/statement emitter against a
            kernel-scoped %State (the buffer/scalar projection); work-item id
            bound to get_global_id; llc --mtriple=spirv64 → blob embedded.
```

---

## `accel` Keyword (Design A — no virtual variables)

```briev
let i: Int = 0;
accel node force [i < nb][i == nb] {
    dv[i] = force_on(i);       // per-work-item compute (disjoint affine write)
    i = i + 1;                 // native counted-loop advance
    term;
};
```

- `i` is a real state counter; every reference is valid runtime state.
- The precondition `[i < N]` is the loop bound and firing gate; the
  postcondition `[i == N]` is the goal ("loop until true").
- **Eligibility proof** (`accel.rs`): `i` is a state counter incremented in
  the body; every write is an array slot affine in `i` (disjoint across
  work-items); value types are flat (TypeUniverse, never name-matching);
  kernel statements are pure.
- **Dispatch**: the GPU path launches N work-items once and fast-forwards the
  counter to N (the loop exits after one firing); the CPU path runs the
  counted loop natively.
- Module shortcut: `!> accel: try_all;` / `force;` / `try_all_force;` — see
  SPEC §9.7.

---

## Process Boundary Keywords

- `endprogram [code];` — genuinely exits the process (SPEC §11.5). LLVM emits
  `call @__exit(i64 code)` (briev_rt.c, runs atexit cleanup). Unlike `term`
  (transaction end), a node with an always-true precondition cannot re-fire
  after `endprogram`.
- `beginprogram` — an optional precondition conjunct marking the program
  entry (SPEC §11.5.1). A `[beginprogram && <state>][<goal>]` node is an
  **entry loop**: entered once when its state conditions hold, the body loops
  until the goal. The goal must be provably reachable (compile error
  otherwise); at most one beginprogram node may be eligible at start
  (compile-time conflict proof). A per-node `@briev_begin_<name>` flag gates
  the precondition and is cleared when the goal is met.

---

## Runtime — `briev_accel_rt` (device-agnostic)

`lib/runtime/briev_accel_rt.c` is a **dispatcher over a pluggable
device-driver table**. The compiler never names a device: it emits SPIR-V
blobs + per-kernel layout descriptors + calls a stable `briev_accel_*` ABI.

```c
int  briev_accel_init(const BrievKernelDesc* descs, uint32_t n);
int  briev_accel_launch(uint32_t idx, const void* state, uint64_t work_n);
int  briev_accel_available(void);
int  briev_accel_probe(...);   // auto-tuning probe
```

- `BrievDeviceDriver` function-pointer table; drivers consume SPIR-V:
  `briev_dev_vulkan` (Vulkan compute) + `briev_dev_opencl` (OpenCL 3.0 IL),
  loaded via `dlopen`, selected by `BRIEV_ACCEL_DEVICE` env + fallback chain
  (Vulkan → OpenCL → CPU).
- Generic pack/unpack: the runtime packs the kernel `%State` projection
  (arrays + scalars, kernel field order) into one flat device buffer per
  launch; each driver only uploads, dispatches, downloads.
- **Auto-tuning probe** (`Probe` decisions): runs the CPU and GPU lanes on
  separate state copies, times each over `accel_probe_k` full-map runs, and
  commits GPU only when `GPU×(1+margin) < CPU` AND the outputs match within
  tolerance (the correctness gate). Tunables:
  `config/ir-lowering.dbvl` (`accel_probe_k`/`_tolerance`/`_margin`).

SPIR-V is the portable device format (NVIDIA NVK, AMD RADV, Intel ANV, Apple
MoltenVK, LLVMPipe). A CUDA driver would need a PTX emitter — a compiler
backend, never a glue change.

---

## CLI

- `--backend gpu` routes to the LLVM backend (the accel path is keyword-
  driven; no `--gpu-offload` flag — removed 2026-08-06).

---

## Performance Considerations

1. **Crossover**: the cost model's device constants belong in config
   (measured per device, not guessed) — calibration is a documented follow-up.
2. **Buffer coalescing**: array fields are emitted as one flat projection per
   kernel (SoA layout), so coalesced access falls out of the layout.
3. **Workgroup sizing**: tunable in the drivers; the kernel's global-id is the
   work-item counter.
4. **Probe overhead**: bounded by `accel_probe_k` full-map runs, once per
   process; Gpu-decision bodies skip the probe.

---

## 2026-09-01 — The GPU backend after the breakthrough day

Measured on RTX 3060 (the canonical device for every number below):

### Kernel synthesis tiers (automatic — the shape decides, never a keyword)

| tier | trigger | M1 GEMV | 4096³ GEMM |
|---|---|---|---|
| work-item (flat) | eligible body | 0.89ms | 6717ms |
| cooperative rows + vec4 | bare counter, K%32==0 | 0.245ms | — |
| tiled GEMM (shared mem) | counter decomposed (m=i/N, n=i%N), M/N/K%64 | — | 25.3ms |
| tensor (coopmat, f16) | Float16 fields + device gate | **FAULTED on device** — knob off | — |

ggml-cuda same-GPU anchors: GEMV 0.213ms (we win batched: 0.205ms), GEMM
10.9ms / 12.6 TFLOP/s (our tiled: 42%; the gap IS tensor cores — their
number on a SIMT-peak card is an fp16/tf32 path).

### `brievc run x.abv` (Track A)

The RT (lib/runtime/briev_accel_rt.c) is compiled into brievc by build.rs
(cc/ar probe-gated; the RT dlopens libvulkan at init — no link-time
dependency). `run` reuses the SPIR-V backend's kernels + `prepare_run`
(runner.rs — the run program shares ssbo_layout and dispatch geometry
with the C runner: one source of truth) and drives the node phase machine
in-process over FFI (src/gpu_rt.rs). Observable outputs: first+last
element of each written field. The two-pass floor exists because the
first resident launch takes the full-copy fallback (lazy staging alloc)
and never seeds.

### Resident-launch soundness gate for `.bv` (Track B)

The resident path leaves results in VRAM between launches — the staging
window is stale by design. `analyze_resident_safety` (accel.rs) proves
all-readers-are-kernels per array field: every reader/writer in the
program must be an eligible accel kernel body (host partitions, non-accel
txns, defns, contracts are walked; top-level initializers are the seed
and are excluded). Kernel-pinned programs emit
`briev_accel_launch_resident` + `briev_accel_download_written` (pull only
is_write fields); anything else keeps the exact full-copy path. The gate
is all-or-nothing per program: mixed resident/full-copy is unsound
(full-copy packs the stale staging into VRAM). CPU-fallback coherence:
`briev_accel_invalidate_resident` clears the seed after any CPU body.

### The buffer-contract Foreach fix (found BY the gate's negative test)

collect_stmt_buffers had no Foreach arm — cooperative kernels'
read/write_buffers were EMPTY since the cooperative rung landed, making
the eligibility array_is_flat checks vacuous. Fixed; regression tests in
accel.rs (resident_gate_tests).

### The f16 tensor fault (open — vendor-level)

Float16 joins the Float category (`type Float16 : Float, Float`), f16
shapes are on the kernel surface, and the tensor emitter produces
spirv-val-clean kernels — but the device path faults: writes stop at
~8.4MB into y (row 1027 of 4096; y-fill sentinel trace), workgroup ~2^12,
variant- and size-independent (256³ SEGFAULTs). The f32 coopmat kernel
(identical structure) ran 4096³ fully correct — the fault is in the
driver/NVVM f16 fragment path. `spirv_coopmat: 0` (off) until a vendor
tooling investigation (nsight-compute, DXC cross-compile) lands. Full
fingerprint: docs/plans/2026-09-01-m2-tensor-cores.md.

### Terminology

Canonical type spelling is `Bit<N>` (BUGS.md "Bits tripwire"); the
de-hashtag purge (fundamentals + protocols bare, hashword mechanisms
stay) is planned in docs/plans/2026-09-01-float16-float-join-and-purge.md
— Float16 : Float already landed as the first bare fundamental join.
