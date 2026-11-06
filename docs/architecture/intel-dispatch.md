# Intel XPU dispatch — Briev's own SPIR-V lane on Level Zero / Vulkan / OpenCL

**2026-11-06.** Status: **ACTIVE design; implementation deferred** to the GPU
roadmap waves (`docs/plans/2026-09-24-followup-stages.md`, multi-vendor
matrix). This is Workstream D of
`docs/plans/2026-11-05-intel-xpu-openvino-harvest.md`.
**Source of facts:** `docs/architecture/intel-backend-harvest.md` (harvested
from OpenVINO `77915ab6` + the PTL node) and the verified node results in
`benchmarks/results/2026-11-06-intel-ptl-first-pass.md`.
**Guardrails:** OpenVINO is a competitor/reference and an FFI escape hatch,
never a codegen path (§14). No source keyword selects a device or a tile
(Golden Rule 2); no algorithm-name recognition (Rule 23). The frontend decides
once; backends and drivers consume the decision.

---

## 1. What is already true (measured, not assumed)

### 1.1 On the Panther Lake node

- GPU: **Arc B390** (Xe3, PCI `8086:b082`, OpenVINO `arch=v30.0.4`, 96 EU)
  — harvest §10.
- **Vulkan `VK_KHR_cooperative_matrix` rev 2** is present: compute stage,
  `shaderBFloat16CooperativeMatrix`, six `VkCooperativeMatrixPropertiesKHR`
  configs, all subgroup scope — `M8 N16 K16` (f16→f16/f32, sf16) and
  `M8 N16 K32` (s8/u8→s32/u32) — harvest §3.1a.
- **Level Zero SPIR-V IL works**: `zeModuleCreate(ZE_MODULE_FORMAT_IL_SPIRV)`
  and `zeKernelCreate` returned `ZE_RESULT_SUCCESS` for a clang-emitted,
  `spirv-val`-clean module — harvest §3.1a.
- OpenCL (NEO 26.31.39395): subgroup sizes 16/32, `cl_khr_il_program`,
  `cl_intel_subgroup_matrix_multiply_accumulate` (+tf32),
  `cl_intel_split_work_group_barrier`, USM — harvest §3.1a/§10.
- Per-arch constants: **subgroup 16** on Xe2/Xe3, **GRF 64 B**, Xe3
  register-file-size option — harvest §3.1.
- OpenVINO competitor ceilings (first pass, gated): **14.7 TFLOP/s** GEMM
  4096³ f16, **16.1 TFLOP/s** SDPA (B8 H32 S1024 D128 f16), elementwise
  ~49–73 GB/s — results record §"Collector timings". These are
  *competitor-effective*, not hardware peaks; they are temporary floors.

### 1.2 In this repository

- Briev's SPIR-V backend already emits cooperative-matrix kernels:
  `Capability::CooperativeMatrixKHR` + `SPV_KHR_cooperative_matrix` +
  Vulkan memory model + 16-bit storage
  (`src/backend/spirv/gemm.rs:965-971`); the portable tier is the committed
  race road (`docs/architecture/abv-gpu-doctrine.md` §2).
- The accel runtime is a **driver table**, not a device-specific runtime:
  `BrievDeviceDriver` in `lib/runtime/briev_accel_rt.h:156-...` with
  `available/init/create_kernel/launch/mapped/launch_dev2d/...`.
- Two SPIR-V consumers already ship: **Vulkan**
  (`lib/runtime/briev_dev_vulkan.c`) and **OpenCL 3.0 IL**
  (`lib/runtime/briev_dev_opencl.c`, `clCreateProgramWithIL` at line 133 and
  line 38). The header already names Level Zero as a third SPIR-V consumer
  (`lib/runtime/briev_accel_rt.h:6-9`).
- Default chain: Vulkan → OpenCL → CPU, overridable via
  `BRIEV_ACCEL_DEVICE` (`docs/architecture/gpu-offloading.md` §"Runtime").
- The Vulkan driver enables coopmat only when the device advertises the
  extension (`briev_dev_vulkan.c:494-515`), and probes/feature-chains the
  storage/float16/coop feature structs
  (`briev_dev_vulkan.c:589-650`).
- Kernel strategy is frontend-owned: `GpuHardware` + `candidate_strategies`
  in `src/analysis/gpu_strategy.rs`; SPIR-V knobs in
  `config/ir-lowering.dbvl:61-78`; dispatch routing in `config/targets.dbvl:22`.

**Path correction to the plan:** the tuning files are
`config/targets.dbvl` + `config/ir-lowering.dbvl` (not `targets.toml`), and
GPU hardware rows currently exist only as Rust constants
(`GpuHardware::SM86`); there is no config-backed Intel GPU row yet.

**The consequence:** Intel needs **no new kernel emitter** for the first
step. It needs (a) the existing Vulkan lane verified on PTL (coopmat already
advertised), (b) the existing OpenCL-IL lane verified as a second option, and
(c) a Level Zero driver added behind the same `BrievDeviceDriver` table to
remove OpenCL/Vulkan submission overhead and use USM.

---

## 2. Architecture fit

```
Briev source (.abv / accel nodes)
        │
        ▼
frontend analysis  ── device capability inputs (probe / config) ──┐
  accel decision (Gpu | Probe | Cpu)                              │
  GemmPlan / strategy (tile, stages, load path)                   │
  kernel image plan (tiled | cooperative)                         │
        │                                                         │
        ▼                                                         │
SPIR-V emitter (one module family)                                │
        │                                                         │
        ▼                                                         │
BrievKernelDesc { spirv, fields, images?, block_threads, ... }    │
        │                                                         │
        ▼                                                         │
briev_accel runtime ── driver chain: Vulkan → LevelZero → OpenCL → CPU
        │                                                         │
        ▼                                                         │
Intel driver: probe caps ◄────────────────────────────────────────┘
  (arch, subgroup, GRF, coopmat tiles, EU count, driver version)
```

Rules carried from the existing architecture:
- **Analysis once** (`docs/architecture/backend-contracts.md` §2): the Intel
  device probe contributes *inputs* to the frontend plan; the driver never
  re-derives strategy from the SPIR-V text.
- **Same plans in, same contract out** (`abv-gpu-doctrine.md` §3): every
  Intel lane consumes the same `GemmPlan` / dispatch predicates as the
  portable tier and verifies against the naive reference.
- **Probe-driven, config-disclosed** (`abv-gpu-doctrine.md` §3): the author
  sees which lane ran in diagnostics; the source never encodes it.

---

## 3. Design decisions

### D1 — Device capability model is explicit and probed once

Add an Intel capability record (Rust struct; config-serializable) populated
by the runtime probe and consumed by the frontend cost model:

| field | source | PTL value |
|---|---|---|
| `arch` | GMDID via `cl_intel_device_attribute_query` / L0 IP version (harvest §3.1) | Xe3 (`v30.0.4`) |
| `subgroup_size` | probe / arch table | 16 |
| `grf_bytes` | arch table (harvest §3.1) | 64 |
| `eu_count` | device query | 96 |
| `coopmat_tiles` | `vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR` | M8×N16×K16 (f16/f32, sf16), K32 (s8/u8) |
| `max_workgroup` | device limits | measure (not recorded yet) |
| `smem_bytes` | device limits | measure (not recorded yet) |
| `drm_gbps` / `peak_tflops` | measured calibration | provisional competitor floors (§10) |
| `driver_version`, `device_id` | device query | NEO 26.31.39395, `0xb082` |

**Consumer:** `src/analysis/gpu_strategy.rs` (new Intel `GpuHardware` row +
cost-model fields), `config/ir-lowering.dbvl` (knob defaults),
`backend/capabilities.rs` (declared surface). No type-name matching: the
record is (protocol, hardware) data, never a Briev type check (Rules 15/19).

### D2 — Driver chain: Vulkan first, Level Zero second, OpenCL third

Default order on Intel: **Vulkan → Level Zero → OpenCL → CPU**, with
`BRIEV_ACCEL_DEVICE` override.

- **Vulkan first** because it is the **only path with verified cooperative
  matrix on PTL** (§1.1) and Briev's coopmat emitter already targets it.
- **Level Zero second** because IL module creation is verified and it removes
  the Vulkan dispatch layer (lower submission overhead, USM); its coopmat
  acceptance by IGC is **not yet verified** (G3 gate below).
- **OpenCL third** because Briev's OpenCL driver accepts IL
  (`clCreateProgramWithIL`) and the NEO device advertises `cl_khr_il_program`,
  but NEO's SPIR-V coopmat support is unverified; it is the conservative
  fallback.
- CPU remains the last lane and the correctness reference.

The chain is data, not heuristics: a driver is skipped when `available() = 0`
or when its capability probe rejects the kernel's requirements. A rejected
driver falls through; it never fails the program (`abv-gpu-doctrine.md` §3).

### D3 — One module family; the device picks the image

The SPIR-V emitter keeps producing one module per kernel image plan
(tiled vs cooperative). The **frontend decides which images to build** from
the probed device capabilities + shape; the **runtime decides which image
runs** from the same capabilities at init. Two operational consequences:

1. Cooperative kernels are emitted only when the target declares the
   cooperative-matrix capability (config/target row + probe); otherwise the
   tiled/subgroup kernel is the only image.
2. If a device claims coopmat but rejects the module, the runtime must have a
   fallback image to remain correct. **Design proposal:** extend the kernel
   descriptor tail with an optional fallback SPIR-V blob (mirroring the
   existing dual-image PTX/SPIR-V pattern,
   `lib/runtime/briev_accel_rt.h:40-...`), zero-filled for existing
   descriptors. This is an ABI change and must land with both-side pins
   (`test_kernel_desc_abi_layout` + `accel_rt::desc_abi`) in one commit.

**Consumer:** `src/backend/spirv/gemm.rs` (image selection already exists),
`src/accel_rt.rs` (descriptor ABI), runtime driver chain.

### D4 — Memory: USM device projection, one program buffer, delta sync

The existing contract already gives the right shape:
- one **program-level** device projection per program
  (`BRIEV_DEV_CAP_SHARED_STATE`), producer writes visible to consumers;
- seed only input arrays (`seed_fields`), download only dirty outputs
  (`launch_dev2d` dirty pairs / `download_dev`);
- zero-copy only where the driver can map (`BRIEV_DEV_CAP_ZERO_COPY`).

Level Zero implementation: `zeMemAllocDevice` for the projection;
`zeMemAllocHost` staging for deltas; on iGPU, host USM zero-copy is allowed
only for arch ≥ Xe2 (harvest §2.1: OpenVINO's own rule — note it as a
measured vendor fact, not an instruction). Allocations happen at init, never
inside the timed dispatch; the benchmark lane showed per-call binding/copy
distorts measurement (results record).

**Consumer:** `lib/runtime/briev_dev_levelzero.c` (new), `briev_accel_rt.h`
caps, `gpu-offloading.md` residency contract.

### D5 — Queues and synchronization

- Level Zero: **immediate command lists** (`zeCommandListCreateImmediate`),
  one per stream, `ZE_COMMAND_QUEUE_FLAG_IN_ORDER`; `stream_id % num_ccs`
  round-robin if multiple compute queues (harvest §2.1).
- Do **not** rely on counter-based events on Xe3/Xe3P — disabled for
  performance by the vendor stack (harvest §7.3). Use regular events, or
  queue stamps/barriers for in-order lists.
- Batch identical dispatches into one submission where the host scalar state
  is launch-invariant (mirror `launch_dev2d_batch`): per-launch fence wake is
  the measured overhead on the NVIDIA lane; the same amortization applies.
- Host sync is explicit at the program boundary (`full_sync`); no implicit
  per-kernel host round-trips.

### D6 — Kernel cache keyed by the right facts

Cache in-memory and optionally on disk keyed by:
`hash(SPIR-V bytes) + driver_version + device_id/GMDID + arch + build flags +
runtime tag`. This matches the vendor keying facts (harvest §2.1: driver
version and device name are part of the key) and prevents stale modules
after a driver upgrade. The cache is sorted-key deterministic (no HashMap
iteration into IR/linkage).

**Consumer:** Level Zero driver, benchmark harness (repeat runs), diagnostics.

### D7 — Numerics contract

Default: **f32 accumulate** (the doctrine default, `abv-gpu-doctrine.md` §3).
f16-accumulate is an opt-in numeric-contract choice, never a speed keyword.
On Xe3 the reported coopmat tiles include f16-input→**f32-accumulate**, so
the default tier is directly expressible. int8 uses the K32 tiles
(`s8×s8→s32`, `u8×u8→u32`).

### D8 — XMX strategy on Xe3: portable KHR cooperative matrix first

Decision: target **`SPV_KHR_cooperative_matrix`** (the same capability the
emitter already uses), with `M8×N16×K16` f16 tiles and subgroup scope.

- It is verified on PTL through Vulkan; the identical SPIR-V family is the
  candidate for Level Zero.
- `SPV_INTEL_joint_matrix` is **not** a dependency: no OpenVINO use in-tree,
  and no PTL measurement. If IGC later proves to prefer it, it becomes a
  *projection* of the same frontend plan, not a new strategy.
- Subgroup size 16 is required for the M8×N16 tile interpretation; the
  row-cooperative (non-tensor) kernels may choose 16 or 32 from the probe.
- **Gate:** if Level Zero rejects the KHR coopmat module, Vulkan remains the
  XMX lane and Level Zero serves the tiled kernels; the frontend must not
  assume XMX on a lane that has not passed the gate.

### D9 — Cost-model inputs are measured, never guessed

`GpuHardware` grows the fields Intel needs (subgroup size, coopmat tile set,
register-file bytes, workgroup limits). The first-pass competitor numbers are
**temporary floors/ceilings used only to seed the cost model**, not hardware
peaks:

| input | provisional value | status |
|---|---|---|
| achievable GEMM f16 | 14.7 TFLOP/s (OpenVINO, 4096³) | measured competitor floor |
| achievable SDPA f16 | 16.1 TFLOP/s (scaled geometry) | measured competitor floor |
| elementwise bandwidth | ~49–73 GB/s | measured competitor, read-count dependent |
| DRAM peak / L2 bytes / smem / EU scheduling | **not measured** | must be probed/benchmarked before config commit |

A config-backed GPU device row (schema proposal, not yet implemented):
`gpu.device.<name>: peak_tflops; dram_gbps; l2_bytes; smem_per_sm;
smem_cta_cap; regs_per_sm; sm_count; subgroup_size; coopmat_m; coopmat_n;
coopmat_k_f16; coopmat_k_i8`.
Absent fields must produce a diagnostic, never a silent x86/NVIDIA default
(the existing `target.*` rule).

### D10 — OpenVINO is not on this path

The FFI lane (`lib/std/ai/openvino.bv`) exists so users can call OpenVINO
explicitly, and so the benchmark lane can measure the competitor. The Intel
dispatch never lowers a Briev program to an `ov_model`, never calls an
OpenVINO op, and never consults an OpenVINO heuristic. This preserves
`proof-vs-shape.md`: the compiler derives; the vendor library is an escape
hatch.

---

## 4. Diagnostics and capability declaration

- Declare the Intel surface in `src/backend/capabilities.rs` before emitting
  (`docs/architecture/backend-contracts.md`): SPIR-V scalar/indexed compute,
  f16 storage, coopmat when the device declares it.
- Runtime diagnostics name what ran and why: driver, device name, arch,
  image (tiled/cooperative), fallback reason. `brievc` keeps the source free
  of device names.
- Capability failures follow the house style: what is unsupported, the
  evidence (probe result / missing extension), and the concrete fallback
  (tiled image, OpenCL/Vulkan lane, CPU).

---

## 5. Verification ladder (implementation gates, in order)

Every rung is correctness-first, same-geometry, before any timing; no rung is
skipped because a lower one passed.

| gate | what | pass condition |
|---|---|---|
| **G0** | Existing Vulkan + OpenCL Briev drivers on PTL (no new code) | elementwise + 512³ GEMM output matches the CPU reference; coopmat image runs on Vulkan (extension already advertised) |
| **G1** | Level Zero driver skeleton | `available/init/create_kernel` succeed; a trivial kernel dispatches and writes the expected sentinel |
| **G2** | L0 projection launch | packed projection round-trips exactly (seed + dirty + download), single and multi-kernel |
| **G3** | L0 coopmat | same GEMM module accepted by IGC and numerically equal to the Vulkan tiled path within the tier gate; if rejected, record the verdict and keep Vulkan as the XMX lane |
| **G4** | Dispatch integration | frontend plan → driver chain under `BRIEV_ACCEL_DEVICE`; rejected drivers fall through; no program fails on probe failure |
| **G5** | Performance | measured vs the first-pass competitor floors with the collector; only then promote a config row/default |

Each gate records geometry, driver/driver-version, image, gate evidence, and
the collector timing source in a `benchmarks/results/` record (house rule:
timing needs same-commit, same-geometry correctness).

---

## 6. Non-goals

- No OpenVINO graph lowering, no `ov_model` construction, no vendor library
  in the kernel path (§14).
- No source-level device/tile keywords; a strategy keyword never wins a
  benchmark (Golden Rule 2).
- No algorithm-name or type-name matching in the driver/backend (Rules
  15/19/23).
- No NVIDIA-specific assumptions projected onto Intel: subgroup 16, GRF 64,
  Xe3 event behavior, and register-file options are recorded Intel facts.
- No PTX-style vendor emitter for Intel in this workstream: SPIR-V IL is the
  Briev-owned path (verified) and keeps one frontend plan.

---

## 7. Open questions / prerequisites

1. **Does IGC (Level Zero `ZE_MODULE_FORMAT_IL_SPIRV`) accept
   `SPV_KHR_cooperative_matrix`?** Vulkan (ANV) does; L0 is unverified.
   Gate G3.
2. **Does L0 IL execute correctly beyond module creation?** G1/G2.
3. **Measured Intel hardware parameters** — DRAM bandwidth, L2, shared
   memory, workgroup limits, EU throughput — required before any
   `GpuHardware` row is committed (D9). The first-pass numbers are competitor
   effective, not peaks.
4. **Dual-image descriptor ABI** (D3) — design + both-side pins before the
   runtime fallback exists.
5. **Subgroup size policy for row-cooperative kernels** — 16 vs 32; PTL
   advertises both (harvest §3.1a). Measure per kernel family.
6. **Kernel cache/replay interaction** — static-model record/replay does not
   exist in Briev today; if added, its dispatcher must consume the same
   frontend plan.

---

## 8. Consumer map (harvest → design)

| Harvest fact | Design consumer |
|---|---|
| Layouts: 4-D bfyx carrier, blocked fsv, reorder vetoes (harvest §1) | D9 (access/coalescing inputs); FFI boundary notes (C) |
| Dispatch: L0 immediate lists, queue/stream model, IL module creation, no OV SPIR-V (harvest §2) | D2/D4/D5/D6 |
| Extensions: Xe3 coopmat tiles, subgroup 16, GRF 64, DPAS-via-CL (harvest §3) | D1/D8 |
| Per-arch SDPA/GEMM tile catalogs, offline tuning (harvest §4) | D9 cost-model shape family; verification portfolio |
| Op coverage / LLM kernels (harvest §5–6) | Kernel coverage sequencing for Briev composites |
| Pitfalls: counter events Xe3, register pressure, reorder traps (harvest §7) | D5/D9/D8 fallbacks |

---

## 9. Implementation sequencing (deferred)

1. **Wave 0 (no compiler change):** run G0 with the existing Vulkan/OpenCL
   drivers on PTL; record the first Briev-on-Intel numbers against the
   OpenVINO lane. This may already deliver a usable Intel lane.
2. **Wave 1:** Level Zero driver behind `BrievDeviceDriver` (G1/G2), reusing
   the GPU projection contract; no emitter change.
3. **Wave 2:** coopmat image selection + dual-image fallback (D3/G3), fed by
   the probed capability record (D1).
4. **Wave 3:** cost-model row + config schema (D9), then the benchmark
   portfolio reruns (G5).

No wave starts before the previous gate passes; every step commits the
verification record with it.
