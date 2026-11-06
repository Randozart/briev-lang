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
| No SPIR-V import in OCL/ZE; SPIR-V only via SYCL+ocloc | `intel-dispatch.md`; `config/targets.toml` | Briev's own tier would call `zeModuleCreate(ZE_MODULE_FORMAT_IL, spirv)` — a capability OpenVINO does not exercise; must be verified on PTL in Workstream B |
| Kernel cache key includes driver version + device + runtime tag | Workstream B benchmark doc; `intel-dispatch.md` | Mirror the keying so Briev kernel caches are not stale across driver upgrades |
| Counter-based events broken on Xe3/Xe3P (GSD-13050) | §7 pitfalls; `intel-dispatch.md` | Do not depend on counter-based events on Panther Lake |
| Device info: EUs, threads/EU, num_ccs, arch enum | `src/analysis/gpu_strategy.rs`; `config/targets.toml` | Inputs to the shape-driven cost model; no type-name matching (Rule 19) |

---

*Sections 3–8 (extension/IGC matrix, per-arch configs, op coverage, LLM
kernels, pitfalls ledger, consumer ledger) follow in the next commit.*
