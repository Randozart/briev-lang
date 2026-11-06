# Intel XPU backend harvest — layouts, dispatch, extensions, kernels

**2026-11-06.** Status: ACTIVE (Workstream A of
`docs/plans/2026-11-05-intel-xpu-openvino-harvest.md`).
**Source:** `openvinotoolkit/openvino`, shallow clone at commit **`77915ab6`**
(2026-11-06). Paths below are relative to the clone root; the GPU plugin root
is `src/plugins/intel_gpu`.
**Method:** read-only harvest. This document is the only deliverable; no
Briev compiler code derives from the harvest in this workstream.
**Exit criterion (hard):** every fact names its Briev consumer. A fact with no
consumer is archived, not harvested (§8 is the auditable ledger).
**Guardrails:** `docs/plans/2026-11-05-intel-xpu-openvino-harvest.md` §1 —
OpenVINO is a reference and a competitor, never a codegen path; harvest facts,
never op lists; the vendor runtime never enters Briev's kernel path.

---

## 0. Provenance and the recognition receipt

### 0.1 Repo facts verified against the clone (plan §3 amended)

The plan's paths are off by one `src/`: the real layout is
`src/plugins/intel_gpu/src/{graph,kernel_selector,plugin,runtime}` (147 files
in `graph/`, including `CMakeLists.txt`; 138 `.cpp` op/topology files).
Verified present: `gemm.cpp`, `grouped_matmul.cpp`,
`scaled_dot_product_attention.cpp`, `paged_attention.cpp`, `rope.cpp`,
`rms.cpp`, `kv_cache.cpp`, `moe_gemm.cpp`, `moe_3gemm_fused.cpp`,
`gated_delta_net.cpp`, `paged_gated_delta_net.cpp`, `vl_sdpa.cpp`, `msda.cpp`,
`paged_causal_conv1d.cpp`, `paged_selective_ssm.cpp`, `layout_optimizer.cpp`,
`reorder.cpp` (all under `src/plugins/intel_gpu/src/graph/`).
`kernel_selector/` hosts 77 op-kernel directories.
The C API lives at **`src/bindings/c/`** (not `src/c_api/`), library name
`libopenvino_c` — the Workstream C FFI target.

**Briev consumer:** this section calibrates the plan and the Workstream C
link target; no compiler consumer (provenance only).

### 0.2 The GatherElements receipt (why recognition loses)

The repo owner's PR [openvinotoolkit/openvino#39432](https://github.com/openvinotoolkit/openvino/pull/39432)
("[GPU] Fix rank-1 GatherElements with a constant input") is open against the
Intel GPU plugin. Facts (from the plan's verified repro, §0):

- The plugin stores a 1-D tensor in a 4-slot `(batch, feature, height, width)`
  layout. A 1-D **constant** lands in the FEATURE slot as `(1, N, 1, 1)`;
  a 1-D **parameter** lands in BATCH as `(N, 1, 1, 1)`
  (verified: `src/plugins/intel_gpu/src/plugin/ops/constant.cpp:87` —
  `case 1: cldnn::tensor(1, TensorValue(constDims[0]), 1, 1)`).
- Gather, GatherND and Split carry **hand-written** entries in a per-consumer
  list that flips 1-D constants to batch interpretation
  (`constant.cpp:98-109` `ConstProperties::needsBatchInterpretation`; decision
  sites `constant.cpp:263-337`; rationale comment `constant.cpp:259-262`:
  "WA to inconsistency between input and const 1d tensors").
- **GatherElements was missing from that hand-curated list.** Its axis 0
  therefore pointed at the feature slot whose batch stride is 0 — the index
  was multiplied away and every rank-1 row returned element 0.
  On the stock 2026.4.1 wheel the minimal repro gave GPU `[7,0,0,0]` vs CPU
  `[7,42,99,0]` pre-fix; matching post-fix; 12/12 new
  `GatherElementsLayerTest` instances failed pre-fix and pass post-fix.
- The current clone (HEAD `77915ab6`) does **not** contain the fix, consistent
  with the PR being open in review. The list at `constant.cpp:263-337` is the
  live evidence of the hand-curation pattern.

**Why this is the strongest derivation receipt we have:** a mature vendor
maintains a hand-curated per-op layout table and a missing row silently
miscomputed every rank-1 row. Briev's uniform `[data, cap, len]` collection
layout plus derived element access has no slot-assignment table a row can be
missing from. Record this in the doctrine citations; it is real-world evidence
from the vendor's own code for Rule 23 (no recognition) and Rule 15/19 (no
knowledge of specific types).

**Briev consumer:** `docs/architecture/derivation-not-recognition.md`
(citation in the enforcement/worked-example section); this document §7
pitfalls ledger; Workstream C's boundary tests (a 1-D tensor crossing the FFI
must not inherit an implicit slot convention).

---

## 1. Layout system — how Intel wants tensors shaped

### 1.1 Facts

**Dimension vocabulary.** Format letters: `b` batch, `f` feature/channels,
`x` width, `y` height, `z` depth, `w`/`u`/`v` further spatial, `g` groups
(`include/intel_gpu/runtime/format.hpp:50-56`, doc `82-87`). Planar formats
have an explicit "logical order" vs "internal order"; e.g. `bfyx` stores
`bfxy` (x innermost) — `src/runtime/format.cpp:33,56`.
`DataLayout` in the kernel selector mirrors this with rank comments
(`src/kernel_selector/tensor_type.h:27-82`).

**Blocked formats.** The plugin's performance layouts are blocked:
- `b_fs_yx_fsvN` (feature-sliced blocked by N; 2/4/8/16/32) —
  `format.hpp:109-118`; `b_fs_yx_fsv16` "used for blocked convolution",
  `fsv4` for IMAD, `fsv32` for blocked int8.
- Double-blocked `bs_fs_yx_bsv16_fsv16`, `bs_fs_yx_bsv32_fsv32` —
  `format.hpp:119-145`; `fs_b_yx_fsv32` for fp16 ("32 features to avoid
  partial writes", `src/kernel_selector/tensor_type.h:75`).
- Weights formats `o_is_yx_isv*`, `os_*`, MMAD/IMAD variants, `_swizzled_by_*`
  — `format.hpp:175-254`.
- The physical packing is defined by the traits table: block sizes are
  `(inner-order axis, block size)` pairs (`format.cpp:42-83`);
  address math in `include/intel_gpu/runtime/tensor.hpp:536-551`
  (`get_linear_offset`) appends an innermost blocked axis.
- Predicates: `is_blocked`, `is_single_blocked`, `is_multi_blocked`
  (`format.hpp:361-366`).

**Default format per rank** (`format.cpp:233-262
format::get_default_format`): data ranks 1–4 → `bfyx`; rank 5 → `bfzyx`;
rank 6 → `bfwzyx`; 7 → `bfuwzyx`; 8 → `bfvuwzyx`. Weights (non-grouped):
rank 5 → `oizyx`, else `oiyx`; grouped: rank 5 → `goiyx`, rank 6 → `goizyx`.
So on Intel a rank-1 or rank-2 tensor is **physically a 4-D bfyx tensor**;
ranks are metadata over a fixed 4-slot carrier.

**1-D placement is consumer-dependent.** Rank-1 constants default to the
FEATURE slot (`constant.cpp:87`); a hand-curated consumer list flips them to
BATCH (`constant.cpp:98-109, 263-337`). Rank-1 *parameters* go to BATCH.
There is no general rule; this is the §0.2 bug class.

**layout_optimizer decides reorders** (`src/graph/layout_optimizer.cpp`):
- Per-node preferred formats come from the implementation factory
  (`graph/graph_optimizer/select_preferred_formats.cpp:211-219`), then
  `reorder_inputs.cpp` inserts a reorder iff formats differ
  (`reorder_inputs.cpp:436, 444-486`).
- Conv selection is a heuristic tree: dynamic shapes → `b_fs_yx_fsv16`
  (`layout_optimizer.cpp:1293-1300`); oneDNN i8 → `fsv32`
  (`1308-1315`); IMAD rank-4 grouped non-depthwise → `fsv4`
  (`imad_case`, `981-1008`); network-wide `fsv16` gated by a support-ratio
  check (`776-798, 674-749`); NHWC `byxf` only under 1x1/no-padding/feature
  divisibility constraints (`647-672`); Winograd only f16 + 3x3 + stride 1 +
  feature%64==0 (`572-599`).
- **Reorder-size veto:** `has_reorder_before_mvn` (`751-774`) rejects a
  layout when the induced reorder exceeds a size threshold (comment: "planar
  format Convolution without Reorder is better than blocked format
  Convolution in case Reorder is larger than [1, 512, 128, 128]").
- **Register-pressure veto:** `b_fs_yx_fsv16` is rejected when
  `input_block_size > 64` to prevent `CL_OUT_OF_RESOURCES` on
  GRF-constrained GPUs (`layout_optimizer.cpp:717-731`, Xe-LPG 128 GRF/thread).
- Some ops demand plain formats: reshape ("blocked to simple is not
  acceptable", `1587-1595`), FC and GEMM under new shape infer
  (`1650-1655, 1666-1671`).

**Reorders are often free.** `layout::compatible`
(`src/runtime/layout.cpp:485-579`) makes default↔default a reinterpret;
`check_redundant_1d_along_feature` (`682-726`) makes a rank-1-along-feature
pair compatible when `feature == inner block size`. At runtime the reorder
primitive skips the kernel and aliases/reinterprets memory
(`src/graph/reorder.cpp:232-234, 276-313`). `remove_redundant_reorders.cpp`
folds reorders into producer/consumer, dedupes, and drops format-preserving
ones (passes at `69-159, 282-386, 388-445, 447-517, 557-683`).

**XMX does not steer format choice.** No XMX/matrix-engine term appears in
`layout_optimizer.cpp`, `reorder.cpp`, `format.cpp`, `layout.cpp`, or
`tensor_type.h`. XMX gates *transformations* one level up
(`plugin/transformations/convert_matmul_to_fc.cpp:227-268`, `supports_immad`)
and kernel-impl selection (`impls/cm`, XeTLA). Blocked-format choice is driven
by dtype, channel counts, kernel availability, and reorder size.

### 1.2 Briev consumers

| Fact | Consumer | How |
|---|---|---|
| Intel carries tensors as 4-D bfyx regardless of rank | `docs/architecture/slicing.md`; Workstream C FFI boundary notes | A Briev `[data, cap, len]` list crossing into OpenVINO must be explicitly reshaped; never infer a slot |
| 1-D constant feature-slot vs parameter batch-slot | §0.2 receipt; Workstream C gate test | Test must assert 1-D round-trip values, not just shapes |
| Blocked fsv16/fsv32 layouts want feature-major, blocked access | `src/analysis/gpu_strategy.rs` Intel entries; `config/targets.toml` | Vector width / access-order knobs must be derived from layout facts, not type names (Rule 19) |
| Reorder vetoes: size > threshold, register pressure > 64 blocks, plain-format ops | Workstream D `intel-dispatch.md` | Budget reorders explicitly in the Intel cost model; treat "plain is faster" as a measured fact |
| Free reinterpret when feature == block size / rank-1-along-feature | `intel-dispatch.md`; slicing doc | Mirrors Briev's zero-copy slice rule (contiguous rebase is free) |
| XMX never changes format selection | Rule 23/19 guard | Confirms format and matrix-engine decisions are separable; do not couple them in our Intel projection |

---

## 2. Dispatch — Level Zero and OpenCL machinery

### 2.1 Facts

**Backends.** Three runtime backends exist: OpenCL (`runtime/ocl/`), Level
Zero (`runtime/ze/`), SYCL (`runtime/sycl/`). Compile-time `GPU_RT_TYPE` =
`ZE|OCL|SYCL|COMBINED` (`cmake/utils.cmake:17-27`); build default is **OCL**
(`cmake/features.cmake:45-49`). Runtime preference: ZE if compiled, else OCL,
else SYCL (`runtime/engine_configuration.cpp:9-31`). `OV_GPU_RUNTIME`
overrides per process (`plugin/plugin.cpp:1175`). Probe scoring prefers ZE on
Intel devices that are Xe2+ (`gfx major >= 20`) **and** expose Level Zero↔OpenCL
interop (`LEO`) (`runtime/dispatch_probe.cpp:35-73`).

**Queues/streams.**
- Default queue type is **out-of-order** (`OV_CONFIG_RELEASE_INTERNAL_OPTION
  queue_type, QueueTypes::out_of_order`, `include/intel_gpu/runtime/options.inl:62`).
  Sync method derives from queue type: profiling → events, OOO → barriers,
  in-order without profiling → none (`runtime/stream.cpp:11-16`).
- OCL builds `clCreateCommandQueueWithProperties`; OOO is silently demoted
  if unsupported; priority/throttle hints use `cl_khr_priority_hints` /
  `cl_khr_throttle_hints` (`runtime/ocl/ocl_command_queues_builder.cpp:21-131`,
  `ocl_stream.cpp:224-231`).
- ZE uses **immediate command lists** (`zeCommandListCreateImmediate`,
  `ZE_COMMAND_QUEUE_FLAG_IN_ORDER` when in-order, ASYNCHRONOUS mode),
  index = `stream_id++ % num_ccs` (`runtime/ze/ze_stream.cpp:216-242`).
- Stream count: LATENCY → 1, THROUGHPUT/AUTO → `max(num_ccs, 2)`
  (`runtime/execution_config.cpp:446-465`). oneDNN forces in-order
  (`execution_config.cpp:377-379`).

**Kernel binary handling.** The OCL/ZE paths do **not** import SPIR-V:
`clCreateProgramWithIL` has zero hits. OCL builds from source
(`clCreateProgramWithSource`) or driver-native binary
(`runtime/ocl/ocl_kernel_builder.hpp:37-56`). ZE calls `zeModuleCreate` with
`ZE_MODULE_FORMAT_NATIVE`; when the device cannot compile OCL source through
L0, the plugin builds via OCL and feeds the OCL program binary to
`zeModuleCreate` as NATIVE_BIN (`runtime/ze/ze_kernel_builder.cpp:35-125`).
SPIR-V appears **only** on the SYCL path: OpenCL-C → SPIR-V through
`ocloc -spv_only`, then submitted as SPIR-V
(`runtime/sycl/intel/sycl_kernel_builder.cpp:78-145`,
`runtime/sycl/sycl_kernel.cpp:22-99`).

**Caching/compilation.** Kernels ship as an embedded source DB
(`kernel_selector/primitive_db.cpp`, generated from `cl_kernels/*.cl`).
Build batches default to 8 kernels (`GPU_MAX_KERNELS_PER_BATCH`,
`runtime/impls/ocl/kernels_cache.cpp:115-173`, `options.inl:73`). Disk cache
key = hash(options + driver version + device name + runtime tag + sources),
file `<cache_dir>/<hash>.cl_cache`
(`kernels_cache.cpp:175-183, 237-245`).

**Device enumeration.** OCL enumerates platforms, rotates the Intel platform
first, requires OCL ≥ 2.0; sub-devices (tiles) via affinity partitioning
(`runtime/ocl/ocl_device_detector.cpp:40-227`). ZE enumerates drivers/devices;
sub-devices via `zeDeviceGetSubDevices` (`runtime/ze/ze_device_detector.cpp:16-109`).
Captured device info: EU count, threads/EU, slices/subslices, `num_ccs`,
cacheline, UUID/LUID, arch generation
(`include/intel_gpu/runtime/device_info.hpp:91-195`,
`runtime/ze/ze_device.cpp:186-260`, `runtime/ocl/ocl_device.cpp:222-352`).

**Events/sync.** ZE event kinds: regular `ze_event` (pooled), counter-based
(in-order only; **disabled on Xe3/Xe3P**, `runtime/ze/ze_stream.cpp:245-250`),
empty event (queue stamp only), grouped events
(`ze_stream.cpp:244-262, 331`; `ze_events.cpp:16-24`). Dependency chaining:
events when profiling/sync=events, else barrier stamping
(`ze_stream.cpp:310-344`; `ocl_stream.cpp:291-334`). Host sync =
`zeEventHostSynchronize`/`clWaitForEvents` (`ze_stream.cpp:424-440`).

**Memory.** ZE default allocation `usm_device`; OCL default `cl_mem`
(`runtime/ze/ze_engine.hpp:37`, `runtime/ocl/ocl_engine.hpp:38`).
`usm_shared` is disabled for allocation by policy
(`runtime/engine.cpp:97-105`). No bindless path exists (zero grep hits).
Memory reuse is a host-side pool keyed by byte size / padded layout
(`runtime/memory_pool.cpp:169-330`).

### 2.2 Briev consumers

| Fact | Consumer | How |
|---|---|---|
| Two production dispatch backends (OCL, ZE); ZE preferred on Xe2+ with LEO | Workstream D `docs/architecture/intel-dispatch.md` | Design Briev's dispatch to select L0 vs OCL by the same probe facts; record LEO as an interop dependency |
| ZE: immediate command lists, in-order flag, `num_ccs` queue round-robin | `intel-dispatch.md` | Briev's Intel dispatch shape: one immediate list per stream, CCS-count streams |
| Sync model: events / barrier stamps / none by queue type | `intel-dispatch.md` | Reuse the proven event-chain + queue-stamp design; do not invent a parallel sync scheme |
| No SPIR-V import in OCL/ZE; SPIR-V only via SYCL+ocloc | `intel-dispatch.md`; `config/targets.toml` | **Verified on PTL (2026-11-06):** `zeModuleCreate(ZE_MODULE_FORMAT_IL_SPIRV)` + `zeKernelCreate` succeed for a clang-emitted SPIR-V module (§3.1a). Briev's Intel IL path is viable |
| Kernel cache key includes driver version + device + runtime tag | Workstream B benchmark doc; `intel-dispatch.md` | Mirror the keying so Briev kernel caches are not stale across driver upgrades |
| Counter-based events broken on Xe3/Xe3P (GSD-13050) | §7 pitfalls; `intel-dispatch.md` | Do not depend on counter-based events on Panther Lake |
| Device info: EUs, threads/EU, num_ccs, arch enum | `src/analysis/gpu_strategy.rs`; `config/targets.toml` | Inputs to the shape-driven cost model; no type-name matching (Rule 19) |

---

## 3. Extension / IGC matrix — what may a portable tier rely on?

### 3.1 Facts

**Architecture taxonomy.** `gpu_arch { unknown, gen9, gen11, xe_lp, xe_hp,
xe_hpg, xe_hpc, xe2, xe3, xe3p }`
(`include/intel_gpu/runtime/device_info.hpp:28-39`; duplicated in
`src/kernel_selector/kernel_selector_params.h:367-378`).

**Detection is not a device-id table.** The plugin maps the architecture from
OneDNN ngen's `detectHWInfo` (`runtime/ocl/ocl_device.cpp:365-379`,
`runtime/ze/ze_device.cpp:41-56`, `runtime/sycl/sycl_device.cpp:65-70`).
If the family is `Unknown`, `supports_immad` is force-cleared
(`ocl_device.cpp:376-378`, `ze_device.cpp:310-312`). **XeHPC throws**
"not supported" on the OCL and ZE paths (`ocl_device.cpp:71-72`,
`ze_device.cpp:48`). Without OneDNN, arch = unknown.

**Finer IP version.** `CL_DEVICE_IP_VERSION_INTEL` (GMDID) via
`cl_intel_device_attribute_query`, parsed as
`{architecture, release, revision}` (`ocl_device.cpp:162-184`). Comment:
"Xe2 (e.g. Battlemage) and newer (e.g. Xe3 Panther Lake) have IP version >= 20"
(`tests/functional/shared_tests_instances/skip_tests_config.cpp:29`).

**Per-arch constants:**

| Property | gen9..xe_hpg | xe_hpc | xe2 / xe3 / xe3p | Citation |
|---|---|---|---|---|
| Subgroup size | 8 | 16 (`default` branch) | **16** | `graph/impls/ocl_v2/sdpa/sdpa_gen_micro.cpp:25-38`; `gathermatmul_batched_gemm_gen.cpp:24-41` |
| GRF size | 32 B | 64 B | 64 B | `graph/impls/ocl/kernels_cache.cpp:283-300` |
| CM GRF width | 256 | 256 | 512 (`arch > xe_hpc`) | `graph/impls/cm/vl_sdpa_opt.cpp:173` |
| Register-file-size option | — | — | Xe3+ `-ze-exp-register-file-size 128` | probe `impls/ocl/kernel_selector_helper.cpp:216-240`; consume `kernel_selector/kernel_base_opencl.cpp:184-186` |

**XMX/DPAS is `supports_immad`.** OCL: `CL_DEVICE_FEATURE_FLAG_DPAS_INTEL`
(`runtime/ocl/ocl_device.cpp:316`); ZE: `ZE_INTEL_DEVICE_MODULE_EXP_FLAG_DPAS`
(`runtime/ze/ze_device.cpp:228`). `supports_bf16 = supports_immad`.
Public `HW_MATMUL` capability iff `supports_immad` (`plugin/plugin.cpp:947-949`).
DPAS paths: OneDNN default iff immad (`runtime/execution_config.cpp:373-376`),
CM/XeTLA SDPA + XAttention gated on immad
(`plugin/transformations_pipeline.cpp:825-832, 1042-1060`), SDPA micro
requires `arch >= xe_hpg` + microkernels (`impls/ocl_v2/sdpa/sdpa_opt.cpp:243-244`),
DPAS GQA multi-token requires `supports_immad && arch >= xe_hpc`
(`impls/ocl_v2/sdpa/paged_attention_opt.cpp:189-211`).

**DPAS access uses OpenCL extensions, never SPIR-V matrix types.**
Extension defines emitted for micro kernels:
`cl_intel_dot_accumulate`, `cl_intel_global_float_atomic`,
`cl_intel_subgroup_matrix_multiply_accumulate`,
`cl_intel_subgroup_split_matrix_multiply_accumulate`
(`sdpa_gen_micro.cpp:1028-1033`, `moe_3gemm_gen_micro.cpp:384-387`,
`gather_matmul_gen_micro.cpp:351-354`). Grep for
`joint_matrix|cooperative_matrix|coopmat` across the plugin yields **zero
matches**.

**Other extensions actually probed/used:** `cl_khr_fp16`, `cl_khr_fp64`,
`cl_khr_subgroups`, `cl_intel_subgroups`, `cl_intel_subgroups_short|_char|_long`,
`cl_intel_required_subgroup_size`, `cl_intel_unified_shared_memory`,
`cl_intel_command_queue_families`, `cl_khr_device_uuid`,
`cl_khr_pci_bus_info`, `cl_intel_device_attribute_query`,
`cl_intel_planar_yuv` (`runtime/ocl/ocl_device.cpp:251-302`). Subgroup
block read/write S/L variants and SLM block IO in
`kernel_selector/cl_kernels/include/batch_headers/sub_group_block_read.cl:153-245`.
`cl_khr_priority_hints` / `cl_khr_throttle_hints` for queues
(`ocl_stream.cpp:227-231`). Split barrier
`intel_work_group_barrier_arrive/wait` in `impls/ocl_v2/sdpa_micro.cl:1331+`
(the providing extension is not declared in-tree — **unclear**, do not
assume).

**Microkernel/CM probes are driver-version + compile probes, not arch-only.**
Gemmstone microkernel support: fast driver-version thresholds then a raw-vISA
compile probe (`impls/ocl/kernel_selector_helper.cpp:147-214`); CM support:
JIT a kernel with `static_assert(CM_HAS_DPAS)` (`71-101`).

### 3.1a Panther Lake / Xe3 — verified on the node (2026-11-06)

Measured on `Overlord-x8664` (Arc B390, PCI `8086:b082` rev 04, OpenVINO
`arch=v30.0.4`, 96 EUs). Full environment in §10.

**Vulkan cooperative matrix is present and usable:**

- `VK_KHR_cooperative_matrix` extension revision **2**; feature
  `cooperativeMatrix = true`; supported stage = **`SHADER_STAGE_COMPUTE_BIT`**;
  `shaderBFloat16CooperativeMatrix = true`.
- The device reports **6 `VkCooperativeMatrixPropertiesKHR` configurations**,
  all `scope = SUBGROUP` (3):

  | M | N | K | A | B | C | Result |
  |---|---|---|---|---|---|---|
  | 8 | 16 | 16 | FLOAT16 | FLOAT16 | FLOAT16 | FLOAT16 |
  | 8 | 16 | 16 | FLOAT16 | FLOAT16 | FLOAT32 | FLOAT32 |
  | 8 | 16 | 16 | SFLOAT16 | SFLOAT16 | SFLOAT16 | SFLOAT16 |
  | 8 | 16 | 16 | SFLOAT16 | SFLOAT16 | FLOAT32 | FLOAT32 |
  | 8 | 16 | 32 | SINT8 | SINT8 | SINT32 | SINT32 |
  | 8 | 16 | 32 | UINT8 | UINT8 | UINT32 | UINT32 |

  (enumerated with `vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR`
  resolved through `vkGetInstanceProcAddr`; the loader does not export the
  symbol directly.)

**Level Zero SPIR-V IL module creation works.** A trivial OpenCL-C kernel
compiled with `clang --target=spirv64 -x cl -cl-std=CL3.0 -c` (validated by
`spirv-val`) was accepted by `zeModuleCreate` with
`ZE_MODULE_FORMAT_IL_SPIRV` (`ZE_RESULT_SUCCESS`), and
`zeKernelCreate("k")` succeeded. This is the capability OpenVINO never
exercises (it uses NATIVE binaries, §2.1) — it is available to Briev's own
Intel dispatch.

**OpenCL device facts:** NEO driver 26.31.39395, OpenCL 3.0, max compute
units 96, sub-group sizes **16, 32**, max sub-groups/WG 64.
`cl_khr_il_program`, `cl_intel_device_attribute_query`,
`cl_intel_subgroup_matrix_multiply_accumulate` (+`_tf32`),
`cl_intel_split_work_group_barrier`, `cl_intel_unified_shared_memory` are
advertised; `cl_intel_subgroup_split_matrix_multiply_accumulate` is **not**
in the extension list (OpenVINO emits its define regardless — do not rely on
that define being load-bearing).
**OpenVINO default GPU inference precision is f16:** a 256×256 f32 matmul
showed max abs error `6.28e-3` by default vs `6.87e-5` with
`INFERENCE_PRECISION_HINT=f32`.

### 3.2 Briev consumers

| Fact | Consumer | How |
|---|---|---|
| Xe2/Xe3 subgroup = 16, GRF = 64 B, Xe3 register-file option | `config/targets.toml` Intel entries; `src/analysis/gpu_strategy.rs` | Cost-model inputs per arch; never a source keyword (Rule 2) |
| DPAS reachable only through CL Intel extensions **in OpenVINO**; no SPIR-V matrix types used there | `docs/plans/2026-09-04-beyond-coopmat.md`; `intel-dispatch.md` | **Answered on PTL:** `VK_KHR_cooperative_matrix` rev 2 is present with 6 M8×N16×K16/K32 subgroup configs (§3.1a); Briev's portable SPIR-V tier can use it. OpenVINO's CL choice was not a hardware limit |
| Arch from ngen family, `unknown` clears immad; XeHPC unsupported on OCL/ZE | `intel-dispatch.md`; `config/targets.toml` | Capability probe must be explicit; unknown arch ⇒ conservative portable tier |
| Microkernel support is versioned/compile-probed, can regress per driver | Workstream B results file; `intel-dispatch.md` | Re-probe per driver era; never bake a driver-version assumption into codegen |
| Extension inventory + subgroup block IO | `intel-dispatch.md` | Candidate primitives for Briev's Intel SPIR-V tier; verify each on the actual driver |

---

## 4. Per-arch configs — concrete knobs for the cost model

### 4.1 Facts

**SDPA micro configuration catalogs** (`impls/ocl_v2/sdpa/sdpa_gen_micro.cpp`):
a `sdpa_config_t { unroll_m_kq, unroll_n_kq, unroll_m_vs, unroll_n_vs,
wg_m_kq, wg_n_kq, wg_m_vs, wg_n_vs }` (`288-293`), with tables per arch and
head size: xe_hpg (`300-362`), xe2 (`~364-470`), **xe3 (`470-498`, "Native
Xe3p configurations from oneDNN SDPA's config catalog")**. Example Xe3 rows:
`xe3_h128 = {32,32,32,32,4,4,4,4}`, `xe3_h128_pa = {16,16,16,16,16,4,16,4}`,
`xe3_h512 = {32,16,32,16,16,2,16,2}`. Choosers
`choose_config_xehpg/xe2/xe3` (`~500-1026`); Xe3 falls through to the Xe2
chooser for h256/h512 leftovers. Debug override knob
`micro_sdpa_workgroup_config` (`options.inl:141`).

**Other per-arch tuning:**
- FC `bf_tiled`: "special tuning params for Gen12HP dGPUs"
  (`kernel_selector/kernels/fully_connected/fully_connected_kernel_bf_tiled.cpp:560-577`);
  immad compressed → tile `(1,1,1,4)` (`557-558`).
- Conv: xe_hpg-only LWS branch
  (`convolution_kernel_b_fs_zyx_fsv16.cpp:123, 265`), xe2+ branches
  (`convolution_kernel_b_fs_yx_fsv16.cpp:277`), `os_iyx_osv32` accepts
  `ip_major >= 20` (`convolution_kernel_bfyx_os_iyx_osv32.cpp:248-250`).
- LWS heuristic: optimal/suboptimal tables, Gen12+ cap-to-256 exceptions
  (`graph/common_utils/dispatch_utils.cpp:153-179`; duplicate
  `kernel_selector/kernel_selector_utils.cpp:367`).
- TernOCL int2 tile tables keyed integrated-vs-discrete
  (`impls/ocl_v2/ternocl_int2/fully_connected_ternocl_int2.cpp:37-190`;
  doc `docs/ternocl_int2_architecture.md:238-276`).
- PagedAttention GQA crossover: `tokens² × context <= 64K` measured on
  Xe2/Xe3 (`impls/ocl_v2/sdpa/paged_attention_opt.cpp:171-174`).
- Occupancy inputs: `maxThreadsPerExecutionUnit = num_threads_per_eu`
  (`impls/ocl/kernel_selector_helper.cpp:1335-1336`).

**Auto-tuning is offline and read-only at runtime.**
`ENABLE_OFFLINE_TUNING_CACHE 1` (`kernel_selector/kernel_selector.cpp:30`);
`GetAutoTuneBestKernel` loads `(kernel_name, config_index)` from
`cache.json` next to the plugin, keyed by params hash × compute-units
(`111-147`; `kernel_selector/auto_tuner.cpp:154-243`). On a miss it takes
the first non-empty implementation after priority sort
(`kernel_selector.cpp:76-106`). No online tuner writes results.

### 4.2 Briev consumers

| Fact | Consumer | How |
|---|---|---|
| SDPA tile catalog shape (`unroll_m/n` per GEMM, wg tiles per GEMM) | Briev attention/LLM kernel plans; `gpu_strategy.rs` attention bucket | The tile/stage family the Briev cost model must be able to derive on Intel |
| Xe3 tables exist and differ from Xe2 | `config/targets.toml` (Xe3 entry) | Seed values, not truth: Briev derives from shape+probe, config carries measured baselines |
| Offline tuning keyed by params hash + device | `intel-dispatch.md` | Mirror the *idea* (per-shape cached plan) in our frontend plan cache; never hardcode tables |
| LWS / occupancy / threads-per-EU inputs | `gpu_strategy.rs` | Occupancy targets derived from device info (Rule 19: via universe/config, not type names) |
| GQA crossover `tokens² × context ≤ 64K` | attention kernel plan | A shape fact for kernel selection; validate at PTL geometry |

---

## 5. Op coverage map — where Briev's stdlib should invest on Intel

### 5.1 Facts

- **138** op/topology `.cpp` files in `graph/`; **77** op-kernel directories
  in `kernel_selector/kernels/`.
- Implementation families live in `graph/impls/`:
  - `ocl_v2/` — hand-written OpenCL for modern/LLM ops: SDPA, PagedAttention,
    MoE (router/mask/gather/scatter/3gemm/int3), gather-matmul, LoRA, RoPE,
    selective-SSM, gated-delta-net (+paged), paged-causal-conv1d,
    group-normalization, col2im, ctc-loss, MSDA, segment-max, slice-scatter,
    scatter-nd-update, PA-KV-cache, TernOCL int2.
  - `cm/` — CM/XeTLA: PagedAttention variants, `vl_sdpa_opt`, XAttention,
    XeTLA LSTM, KV-cache ops.
  - `onednn/` — convolution, deconvolution, fully-connected, GEMM,
    grouped-GEMM, MoE-GEMM, pooling, reduce, reorder, concat, GRU/LSTM,
    SDPA (debug).
  - `cpu/` — CPU fallbacks for a long list of ops.
  - `ocl/` — legacy glue (kernels cache, kernel-selector helper).
- Impl selection is `implementation_map::get(preferred_impl_type, shape_type)`
  (`graph/registry/implementation_map.hpp:40-56`; chosen at
  `graph_optimizer/select_preferred_formats.cpp:204-209`,
  `layout_optimizer.cpp:1503-1522`).
- Pattern (fact, not stated policy): classic CV/geometry ops stay in
  `kernel_selector`; transformer/LLM ops and anything needing DPAS
  microkernels moved to `ocl_v2`/`cm`/`onednn`. **No comment documents the
  migration policy** — do not infer intent.
- OneDNN primitive scope: `CONCAT; CONVOLUTION; DECONVOLUTION; MATMUL;
  REORDER; POOLING; REDUCTION; RNN` (+ SDPA debug)
  (`src/plugins/intel_gpu/thirdparty/CMakeLists.txt:29-32`).

### 5.2 Briev consumers

| Fact | Consumer | How |
|---|---|---|
| LLM ops are the newest, hand-written family; CV ops remain in the legacy selector | Workstream D sequencing; `docs/plans/2026-09-20-gpu-dialect-beyond-cuda.md` | Briev's Intel kernel coverage should land attention/GEMM/MoE first (same priority Intel chose), not start with CV |
| OneDNN covers GEMM/conv/pool/reduce | Briev benchmark lane (Workstream B) | Defines which OpenVINO path is a fair competitor per benchmark |
| Impl selection is shape-type + preferred-impl keyed | `gpu_strategy.rs` | Confirms the cost model needs (shape class, dtype, device class) — the same tuple Briev derives |

---

## 6. Modern LLM kernels — derivation references

### 6.1 Facts

Structure: `graph/<op>.cpp` is a **shape-inference/layout shell** (e.g.
`graph/gated_delta_net.cpp:27-51` asserts input count and returns layouts);
the kernels live under `graph/impls/{ocl_v2,cm,onednn}/`.

- **SDPA.** `graph/scaled_dot_product_attention.cpp` handles transposed input
  orders and rank-broadcast shape inference (`28-61`). Impls: micro
  (`ocl_v2/sdpa_micro.cl`), compiled per config from §4;
  KV staged in SLM as f16/bf16 (`#define KV_SLM_T`, `sdpa_micro.cl:23-27`);
  S-tile quantization modes `QUANTIZE_2D`/`QUANTIZE_COMMON` (`31-34`);
  Q tile `D_MAX/2` columns; both GEMMs tiled by subgroup (`39-65`).
  **Decomposition is preferred on XMX GPUs when memory allows** —
  "sdpa_opt has performance issue on GPUs with XMX"
  (`plugin/transformations_pipeline.cpp:1208`).
- **PagedAttention.** Constraints: `heads_num % kv_heads_num == 0`,
  `k/v_head_size % pa_block_size == 0` (`graph/paged_attention.cpp:180-183`);
  `SUBGROUPS_PER_WG = head_size / SUBGROUP_SIZE`; partition =
  `SEQ_LEN_PARTITION_SIZE / PAGED_ATTENTION_BLOCK_SIZE`
  (`ocl_v2/paged_attention_opt.cl:12-16`); query staged to SLM (`19`);
  unroll disabled for K_HEAD=128/SUBGROUP=16 to bound live registers
  (`23-26`); subgroup reductions for boundary search (`42-64`).
- **KV cache update.** Per-block conversion with
  `sub_group_reduce_min/max` for int4/int8 quantization; shuffle-based
  transpose; pitch layout (key = block_size per token, value = token-major)
  (`ocl_v2/pa_kv_cache_update_ref.cl:6-89`).
- **RoPE.** Vectorized unpack/pack per dtype (f16/bf16/f32), optional
  slicing, no layout constraint (`ocl_v2/rope_opt.cl:8-189`).
- **RMS.** `kernel_selector/kernels/rms/rms_kernel_bfyx_opt.cpp`:
  `subgroup_size = 16`, feature-axis normalization, LWS from local-memory
  budget; fused DynamicQuantize output (`graph/rms.cpp:31-33`); ref kernel
  exists.
- **MoE.** `impls/ocl_v2/moe/`: fused router, mask gen, gather, scatter
  reduction (opt/ref/row-LUT), 3gemm with SwiGLU fusion, int3 GEMV, expert
  weight provider with LRU cache.
- **Gated delta net / paged GDN / paged selective SSM / paged causal conv1d.**
  ref + opt kernels + JIT precompute, plus "large state" variants
  (`impls/ocl_v2/{gated_delta_net_ref,paged_gated_delta_net_*,paged_selective_ssm_*,paged_causal_conv1d_ref}.*`).
- **MSDA.** Optimized + reference (`impls/ocl_v2/msda_opt.cl`); output layout
  is the default format for the result rank (`graph/msda.cpp:16-19`).
- **VL SDPA.** CM/XeTLA impl (`impls/cm/vl_sdpa_opt.cpp`); CM GRF width rule
  from §3.

### 6.2 Briev consumers

| Fact | Consumer | How |
|---|---|---|
| Attention tiling = two GEMMs tiled by subgroup + SLM KV staging + quantized S tiles | Attention kernel plan (`docs/plans/2026-09-20-gpu-dialect-beyond-cuda.md`, `2026-09-04-beyond-coopmat.md`) | Derivation reference for `softmax_fused!`-class composites; shapes must drive tiles (Rule 2/23) |
| Paged KV layout: block_size per token, int4/int8 quantization via subgroup reduce | KV-cache plan (attention lane) | A format fact, not an op name: derive block/quant from declared layout+quant contract |
| XMX GPUs may prefer decomposition over fused SDPA | `gpu_strategy.rs` | Cost model must be allowed to choose decomposition — a measured Intel-specific fact |
| MoE routing/scatter/3gemm decomposition | MoE kernel coverage plan | Coverage priorities; each row lands as a stdlib composite |
| RoPE/RMS are elementwise/normalization primitives with fused-quant variants | stdlib numeric module coverage | Prefer stdlib composites + general passes over named kernels (Rule 23) |

---

## 7. Pitfalls ledger

### 7.1 Recognition receipts

1. **GatherElements rank-1 constant (PR #39432, §0.2).** Hand-curated
   per-consumer 1-D layout list; a missing row returned element 0 for every
   rank-1 row (GPU `[7,0,0,0]` vs CPU `[7,42,99,0]`); 12/12 tests failed
   pre-fix. **Doctrine evidence:** recognition tables have missing-row failure
   modes; derivation from layout does not.

### 7.2 Layout traps

1. 1-D constant → FEATURE slot `(1,N,1,1)`; 1-D parameter → BATCH slot
   `(N,1,1,1)` (`constant.cpp:87`; parameters differ). Any FFI round-trip of
   a 1-D buffer must not rely on a slot convention.
2. Reshape "from blocked to simple format is not acceptable"
   (`layout_optimizer.cpp:1587-1595`) and FC/GEMM accept plain formats only
   under new shape infer (`1650-1671`) — blocked layouts inserted earlier
   force reorders back.
3. Rank-3 MVN always requires Reorder+Reshape; layout insertion is vetoed
   when the reorder would be too large (`layout_optimizer.cpp:751-797`).
4. `b_fs_yx_fsv16` rejected above 64 input blocks (register pressure,
   `CL_OUT_OF_RESOURCES` on 128-GRF parts, `layout_optimizer.cpp:717-731`).
5. fp32→u8→fp32 reorder chains are **not** fused
   (`remove_redundant_reorders.cpp:187-193`).
6. `fs_b_yx_fsv32` vs `b_fs_yx_fsv32`: fp16 path uses the former; after a
   permute the plugin switches to the latter to enable the tiled permute
   kernel (`layout_optimizer.cpp:1621-1631`).

### 7.3 Driver / runtime quirks

1. **Counter-based events disabled on Xe3/Xe3P/unknown**
   ("performance issues… GSD-13050", `runtime/ze/ze_stream.cpp:245-250`).
2. **XeHPC rejected** on the OCL/ZE runtime paths
   (`ocl_device.cpp:71-72`, `ze_device.cpp:48`).
3. **No SPIR-V import** on OCL/ZE; SPIR-V only via SYCL + `ocloc -spv_only`.
4. Out-of-order queues silently demoted to in-order when unsupported
   (`ocl_command_queues_builder.cpp:106-119`).
5. Parallel kernel build disabled inside Microsoft Store app packages
   (driver bug), and `malloc_trim` after build (≥500 MB retained)
   (`kernels_cache.cpp:444-492, 655-662`).
6. Serialized model binaries are driver-agnostic only if ELF/SPIR-V magic;
   otherwise driver-version-matched (`kernels_cache.cpp:556-607`).
7. Shared USM allocation disabled by policy; host USM zero-copy only iGPU +
   arch ≥ xe2 (`runtime/engine.cpp:97-110`).

### 7.4 Env / config workarounds

| Knob | Meaning | Citation |
|---|---|---|
| `OV_GPU_RUNTIME` | Force `OCL`/`ZE` | `plugin/plugin.cpp:1175` |
| `OV_GPU_ALLOW_NEW_SHAPE_INFER` | Switch shape-infer flow ("shall be removed soon") | `options.inl:69`; `execution_config.cpp:62-144, 237-240` |
| `OV_GPU_USE_CM` / `OV_GPU_ENABLE_MLIR` / `OV_GPU_MLIR_PATTERNS` | CM/XeTLA + MLIR patterns | `internal_properties.hpp:143-170` |
| `OV_GPU_USE_ONEDNN` | Force OneDNN path | `internal_properties.hpp:143` |
| `OV_GPU_DISABLE_USM` | Disable USM allocations | `runtime/engine.cpp:77-82` |
| `OV_GPU_RECORD_REPLAY` / `_DYNAMIC` | Runtime record/replay | `options.inl:91,143` |
| `OV_GPU_MAX_KERNELS_PER_BATCH` | Kernel build batch size (default 8) | `options.inl:73` |

**Briev consumers:** Workstream B environment/records file (driver + knob
evidence per measurement); `intel-dispatch.md` (what not to depend on);
Workstream C FFI docs (runtime selection). The recognition receipt feeds
`derivation-not-recognition.md`.

---

## 8. Consumer ledger (exit criterion)

Every harvested section names a consumer. A row with no consumer is archived.

| Section | Primary Briev consumer | Status |
|---|---|---|
| §1 Layout system | `docs/architecture/slicing.md`; `src/analysis/gpu_strategy.rs` + `config/targets.toml`; Workstream C gate | named |
| §2 Dispatch | Workstream D `docs/architecture/intel-dispatch.md`; Workstream C `lib/std/ai/openvino.bv`; Workstream B results | named |
| §3 Extensions/IGC | `intel-dispatch.md`; `beyond-coopmat` plan; `config/targets.toml` | named (one open hardware question: SPIR-V matrix ext on PTL) |
| §4 Per-arch configs | `gpu_strategy.rs`; `config/targets.toml`; attention plan | named |
| §5 Op coverage | Workstream D sequencing; `gpu-dialect-beyond-cuda` | named |
| §6 LLM kernels | attention/LLM kernel plans; stdlib numeric coverage | named |
| §7 Pitfalls | Workstream B environment doc; `derivation-not-recognition.md`; `intel-dispatch.md` | named |

Archived (deliberately not harvested): OpenVINO's test suite internals,
frontend converters, NPU plugin, and non-GPU plugin code — no Briev consumer.

---

## 9. Open questions — status (updated 2026-11-06)

1. **PTL/Xe3 SPIR-V matrix support — ANSWERED YES.**
   `VK_KHR_cooperative_matrix` rev 2, `cooperativeMatrix = true`, compute
   stage, six M8×N16×K16/K32 subgroup configs (§3.1a). `SPV_INTEL_joint_matrix`
   was not separately dumped; the portable Vulkan path is sufficient evidence
   for the portable tier.
2. **Level Zero SPIR-V IL module creation — ANSWERED YES.**
   `zeModuleCreate(ZE_MODULE_FORMAT_IL_SPIRV)` and `zeKernelCreate` both
   returned `ZE_RESULT_SUCCESS` on the node (§3.1a).
3. **Installed runtime — ANSWERED.** OpenVINO **2026.3.1** (makepkg;
   `openvino-intel-gpu-plugin 2026.3.1-1.1`), devices `['CPU','GPU','NPU']`;
   GPU = `Intel(R) Arc(TM) B390 GPU (iGPU)`, `arch=v30.0.4`, 96 EUs. Live
   64×64 and 256×256 GPU matmuls executed correctly (§10).
4. **`cl_intel_device_attribute_query` — ANSWERED YES.**
   Advertised by the NEO device; GMDID-based arch detection is available.
5. **PagedAttention block size + dtype support on Xe3 — OPEN.** The node
   has the 2026.3.1 wheel (not 2026.4.1 as the plan assumed); query the
   plugin's supported ops + `pa_block_size` set before the attention lane.
6. **Workstream B harness remote pattern — OPEN.** Decide harness-SSH-in vs
   harness-runs-on-node using existing `benchmarks/` conventions (plan §5.2).

---

## 10. PTL node environment record — `Overlord-x8664` (2026-11-06)

Verified SSH target: `prestopoverlord@100.92.76.67` (password auth; hostname
`Overlord-x8664`). Credentials are not recorded in this repository.

| Item | Value |
|---|---|
| OS / kernel | CachyOS, Linux `7.2.2-1-cachyos` |
| CPU | Intel Core Ultra X7 358H |
| iGPU | Panther Lake **[Arc B390]**, PCI `8086:b082` rev 04 (Xe3) |
| OpenVINO | `2026.3.1-22476-759c5a6ab8c-makepkg`; plugin `openvino-intel-gpu-plugin 2026.3.1-1.1` |
| OV devices | `CPU`, `GPU` (`Arc B390 iGPU`), `NPU` (`AI Boost`) |
| GPU arch / EUs | `arch=v30.0.4`, 96 EUs, integrated |
| OpenCL | NEO `26.31.39395` (`intel-compute-runtime 26.31.39395.13-1.1`), OpenCL 3.0; sub-group sizes 16, 32 |
| Vulkan | Mesa `26.2.1`, API `1.4.354`; `vulkan-intel 26.2.1` |
| Level Zero | loader/headers `1.32.0` |
| SPIR-V tooling | clang `22.1.8` (`spirv64` target), `spirv-val`, `spirv-dis`, `spirv-as`, `ocloc` |
| OpenCL ICDs | Intel NEO (GPU) + Mesa (CPU); `ocl-icd 2.3.5` |

**Functional gates run (all passed):**

1. `python3 -c "import openvino"` → 2026.3.1, `['CPU','GPU','NPU']`.
2. 64×64 f32 `MatMul` compiled to `GPU` (`GPU.0`) and executed; result shape
   `(64,64)`, max abs error `3.03e-3` (default f16 precision).
3. 256×256 f32 `MatMul`: default `6.28e-3`, `INFERENCE_PRECISION_HINT=f32`
   `6.87e-5` — confirms the default f16 inference precision.
4. `clang --target=spirv64 -x cl … -c` → `spirv-val` OK → `zeModuleCreate`
   `ZE_MODULE_FORMAT_IL_SPIRV` OK → `zeKernelCreate("k")` OK.
5. `vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR` → 6 configs (§3.1a).

**Environment deltas vs the plan (§2):** runtime is **2026.3.1**, not the
2026.4.1 wheel the plan assumed; the GPU plugin *does* enumerate the Xe3
iGPU, so the benchmark lane does not need the CPU-plugin degradation. Driver
version for `GPU_DRIVER_VERSION` was not exposed by this Python build
(property lookup error); use `clinfo` (NEO 26.31.39395) as the recorded
driver version.

**Briev consumers:** Workstream B results-file header (geometry, commit,
driver, gate evidence); Workstream D `intel-dispatch.md` (L0 IL + coopmat
viability); Workstream C gate test (runtime version + GPU enumeration).
