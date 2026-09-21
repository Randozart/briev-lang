// Briev Accel Runtime — ABI header.
//
// The compiler never names a device. It emits SPIR-V kernel blobs + per-kernel
// layout descriptors + calls the stable briev_accel_* ABI below. This runtime
// dispatches to a pluggable device-driver table (BrievDeviceDriver). Kernel
// EMISSION is per device-FAMILY (CUDA needs PTX; Vulkan/OpenCL/LevelZero all
// consume the same SPIR-V), so the glue is shared. See
// docs/plans/2026-08-06-accel-gpu-offload.md §7.
//
// 2026-09-21 (Family K, plan 2026-09-21-family-k-accel-host-consolidation):
// the ABI TYPES + public signatures moved here so the orchestration can move
// to the Rust host while every consumer keeps the same include/link surface.
// The orchestration implementation is migrating to src/accel_rt (Rust);
// the device drivers (briev_dev_*.c) stay C bindings behind the ops table.
//
// Device model: each kernel sees ONE flat projection buffer — the host %State
// sliced to the kernel's buffers in kernel `%State` field order (arrays then
// scalars, each sorted). The kernel GEPs into that struct; the device buffer
// holds exactly that packed struct. The generic pack/unpack is
// device-independent; each driver only uploads, launches, downloads.
//
// Selection: BRIEV_ACCEL_DEVICE env (vulkan|opencl|...) overrides the default;
// otherwise the first available driver wins (Vulkan → OpenCL → CPU).

#ifndef BRIEV_ACCEL_RT_H
#define BRIEV_ACCEL_RT_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// ────────────────────────────────────────────────────────────────────────────
// Layout + descriptor types (emitted by the compiler as const tables)
// ────────────────────────────────────────────────────────────────────────────

typedef enum {
    BRIEV_FIELD_ARRAY = 1,
    BRIEV_FIELD_SCALAR = 2,
} BrievFieldKind;

/// One state field the kernel touches. Fields are listed in KERNEL `%State`
/// order (arrays first, then scalars — the order the kernel GEPs by).
typedef struct {
    const char* name;      // Briev field name (diagnostics)
    uint32_t kind;         // BRIEV_FIELD_ARRAY | BRIEV_FIELD_SCALAR
    uint64_t host_offset;  // byte offset of the field in the HOST %State
    uint64_t elem_bytes;   // array: element size; scalar: value size
    uint64_t count;        // array: element count; scalar: 1
    uint32_t is_write;     // array written by the kernel (readback after)
    // 2026-09-01 (plan vec4-projection-layout): byte offset of the field in
    // the DEVICE projection — declared by the compiler's descriptor
    // emitters (vec4-eligible arrays are 16B-aligned there; host layouts
    // stay packed). The old packed-sum recomputation is gone: one rule,
    // compiler-owned, consumers derive.
    uint64_t proj_offset;
} BrievField;

// 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): a texel-
// formatted state array realized as a device storage image. NOT a
// BrievField — images are not SSBO members (the blob's SSBO excludes
// them); the host %State still holds the flat array, so the download
// copies image -> state+host_offset.
#define BRIEV_IMAGE_FORMAT_R32F 1u
typedef struct {
    const char* name;      // diagnostics
    uint64_t host_offset;  // byte offset of the flat array in HOST %State
    uint32_t width;
    uint32_t height;
    uint32_t format;       // BRIEV_IMAGE_FORMAT_R32F (the only one so far)
} BrievImageDesc;

/// One compiled SPIR-V kernel + its layout.
typedef struct {
    const char* txn_name;  // diagnostics
    const uint8_t* spirv;  // SPIR-V blob
    uint32_t spirv_size;
    uint32_t n_fields;
    const BrievField* fields;
    // 2026-09-02: image-resident arrays. Grows the struct at the END —
    // positional C initializers zero-fill the tail, so every existing
    // descriptor construction compiles unchanged (n_images = 0).
    uint32_t n_images;
    const BrievImageDesc* images;
    // 2026-09-09 (S3b+ perf rungs): CUDA block thread count for this
    // kernel (0 = driver default 64). Tail of the struct — positional
    // C initializers zero-fill it, so existing descriptor constructions
    // compile unchanged.
    uint32_t block_threads;
    // 2026-09-10 (cp.async stages): dynamic shared-memory bytes for this
    // kernel (0 = none). Tail of the struct, same zero-fill contract.
    uint32_t shared_bytes;
    // 2026-09-15 (gpu_schedule Phase 3): the PROGRAM projection extent —
    // max(proj_offset + elem*count) over ALL buffer fields. Per-kernel field
    // tables list only the kernel's touched fields, but the kernel's SSBO
    // struct still spans every member at global offsets; allocations use
    // this so the struct always fits. Tail of the struct, zero-fill contract.
    uint64_t program_bytes;
    // 2026-09-15 (gpu_schedule Phase 3): the program SEED table — input
    // arrays only (first-use txn reads them). The one-time resident seed
    // uploads these; kernel-written arrays are device-produced. Shared by
    // every desc (all point at the same table). Tail, zero-fill contract.
    const BrievField* seed_fields;
    uint32_t n_seed_fields;
    // 2026-09-17 (CyberLlama plan M2.0): optional PTX TEXT blob for the
    // CUDA driver — cuModuleLoadData JITs it; entry sentinel "main" (one
    // entry per module; blob-ABI detail, Briev node names stay arbitrary).
    // NULL/0 = no CUDA-side image (the SPIR-V blob is not valid PTX and
    // the CUDA lane rejects the kernel, clean CPU/Vulkan fallback).
    // Tail of the struct, zero-fill contract.
    const uint8_t* ptx;
    uint32_t ptx_size;
    // 2026-09-18 (P1 lane-coverage fix): when nonzero, the dispatch
    // multiplies the launch count by block_threads so the CUDA driver's
    // gx = ceil(n*bx/bx) = n blocks — the kernel's lane-mapped reduction
    // treats each block as one work item (w = ctaid.x) with all64 threads
    // computing redundantly for full 32-lane d-coverage.  0 = the flat
    // gid model (every thread is a work item).  Tail, zero-fill contract.
    uint32_t block_per_workitem;
} BrievKernelDesc;

// ────────────────────────────────────────────────────────────────────────────
// Device-driver ABI
// ────────────────────────────────────────────────────────────────────────────

#define BRIEV_DEV_CAP_ZERO_COPY 0x1u  // can skip the upload/download copy
// 2026-09-14 (gpu_schedule Phase 0): the driver keeps ONE program-level
// device buffer (the full projection) instead of per-kernel buffers —
// producer array writes are visible to consumers (cross-kernel D2D). The
// runtime's residency seed becomes program-level (full_sync once), not
// per-kernel.
#define BRIEV_DEV_CAP_SHARED_STATE 0x2u

typedef struct {
    /// Raw device transfer: upload `proj` (the flat kernel projection),
    /// dispatch `global_n` work-items, download the result into `proj_out`.
    /// The driver owns its device memory; `proj`/`proj_out` are host buffers.
    int (*launch)(void* kernel, const void* proj, size_t proj_bytes,
                  size_t global_n, void* proj_out);
} BrievDriverOps;

/// 2026-09-18 (M2 strided append): driver-level pitched-copy descriptor —
/// absolute host source, projection-relative device offset. The runtime
/// translates BrievPushDesc (offsets) into these; drivers implement them
/// (CUDA: one cuMemcpy2D height-1 per descriptor).
typedef struct BrievStridedCopy {
    const void* src;      // absolute host memory (the staging buffer)
    size_t proj_off;      // projection byte offset, element 0
    size_t src_pitch;     // host stride between elements (bytes)
    size_t dst_pitch;     // projection stride between elements (bytes)
    size_t width_bytes;   // count * elem_bytes (one row of the gather)
    size_t count;         // element count
} BrievStridedCopy;

typedef struct BrievDeviceDriver {
    const char* name;            // "vulkan" | "opencl" | ...
    uint32_t capabilities;
    int (*available)(void);      // dlopen + device present
    int (*init)(void);
    int (*create_kernel)(const uint8_t* spirv, size_t size, void** kernel_out);
    int (*launch)(void* kernel, const void* proj, size_t proj_bytes,
                  size_t global_n, void* proj_out);
    void (*destroy_kernel)(void* kernel);
    void (*shutdown)(void);
    // 2026-08-31 (plan item 3, device residency): optional. `mapped` returns
    // the kernel's persistent host-visible projection pointer (NULL if the
    // driver cannot keep it mapped); `launch_dev` records dispatch + submit
    // + fence-wait with NO host copies — the runtime drives the mapped
    // projection itself. NULL → the runtime falls back to `launch`.
    void* (*mapped)(void* kernel);
    int (*launch_dev)(void* kernel, size_t global_n);
    // 2026-08-31 (plan 2026-08-31-gpu-next §2b): 2D dispatch — `nx` work
    // items per row, `ny` rows. `full_sync` pushes the ENTIRE staging
    // projection to the device working set first (the seed); otherwise only
    // the `dirty` (offset, bytes) pairs cross (the scalar counters). NULL →
    // the runtime falls back to the full-copy launch.
    int (*launch_dev2d)(void* kernel, size_t nx, size_t ny,
                        int full_sync, const size_t* dirty, uint32_t n_dirty);
    // Pull the device working set into the staging window (device residency
    // download). NULL → the staging window is the source of truth already.
    // 2026-09-02: optional human-readable DEVICE name (vkGetPhysicalDevice-
    // Properties.deviceName) for run diagnostics — NULL → the static
    // driver `name` ("vulkan").
    int (*download_dev)(void* kernel);
    // 2026-09-01 (plan smallm-splitk): `times` identical dispatches in ONE
    // submission (one record, one submit, one fence wait) — the per-launch
    // fence wake (~33us measured) amortizes once per batch. Requires
    // launch-invariant host scalar state across the batch (the caller's
    // contract; scalars sync once, before the dispatches). NULL → the
    // runtime falls back to `times` sequential launch_dev2d calls.
    int (*launch_dev2d_batch)(void* kernel, size_t nx, size_t ny, uint32_t times,
                              int full_sync, const size_t* dirty, uint32_t n_dirty);
    // 2026-09-02: optional real device name (vkGetPhysicalDeviceProperties.
    // deviceName). NULL → the static driver `name` is the best answer.
    const char* (*device_name)(void);
    // 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): optional
    // image-support hooks — TAIL of the struct (positional initializers in
    // the drivers only ever append). `set_images` allocates the device
    // storage images + binds them (set 0, binding 1+) after create_kernel;
    // NULL → the runtime refuses image kernels loudly (the driver cannot
    // serve them). `download_images` pulls each image into its flat host
    // array (state + host_offset); NULL → image kernels read back nothing.
    int (*set_images)(void* kernel, const BrievImageDesc* imgs, uint32_t n);
    int (*download_images)(void* kernel, const BrievImageDesc* imgs,
                           uint32_t n, void* state);
    // 2026-09-09 (S3b+ perf rungs): optional per-kernel block-thread-size
    // override (CUDA tier). NULL → the driver's fixed default (64).
    int (*set_block_threads)(void* kernel, uint32_t n);
    // 2026-09-10 (cp.async stages): optional per-kernel dynamic shared-
    // memory size (CUDA tier). NULL → 0 (no dynamic shared memory).
    int (*set_shared_bytes)(void* kernel, uint32_t n);
    // 2026-09-18 (M4 decode-append, plan 2026-09-18-m3-matrix-and-m4-fattn-shim):
    // optional array-range upload — push (offset, bytes) pairs from the
    // mapped host mirror into the device working set WITHOUT a dispatch.
    // Decode attention appends one KV row per step between kernel chains;
    // the scalar dirty path covers counters only. Uses the same synchronous
    // copy contract as launch_dev2d's dirty loop. NULL → the decode-append
    // path is unavailable (briev_accel_push_ranges returns 0).
    int (*upload_ranges)(void* kernel, const size_t* dirty, uint32_t n_dirty);
    // 2026-09-18 (M2 strided append, plan coalesced-kv-memory-path):
    // pitched host→device copies without a dispatch — the strided
    // generalization of upload_ranges. `copies` are driver-level
    // (absolute host source pointers, projection-relative device
    // offsets). CUDA: one cuMemcpy2D (height 1) per descriptor. NULL →
    // briev_accel_push_strided returns 0.
    int (*push_strided)(void* kernel, const void* copies, uint32_t n);
    // 2026-09-18 (P1 lane-coverage fix): optional per-kernel flag — when
    // nonzero the CUDA driver multiplies the launch nx by block_threads
    // so the grid has `n` blocks (lane-mapped reduction: w = ctaid.x).
    // NULL → flat gid model.  Tail, zero-fill.
    int (*set_block_per_workitem)(void* kernel, uint32_t flag);
} BrievDeviceDriver;

extern BrievDeviceDriver briev_dev_cuda;
extern BrievDeviceDriver briev_dev_vulkan;
extern BrievDeviceDriver briev_dev_opencl;

// Shared runtime diagnostics state — defined by the orchestration host
// (src/accel_rt.rs since Family K), read by the drivers.
extern int g_verbose;        // BRIEV_ACCEL_VERBOSE
extern int g_async_launch;   // BRIEV_ACCEL_ASYNC

// ────────────────────────────────────────────────────────────────────────────
// Public orchestration ABI (implemented by the Rust host, src/accel_rt)
// ────────────────────────────────────────────────────────────────────────────

/// Public strided-push descriptor. A d-major K append writes ONE new token
/// row as `count` elements spaced `dst_pitch` bytes apart in the state
/// (the source row arrives packed — ggml's cache row or the adapter's
/// staging buffer). One descriptor per (buffer, kv head); a whole decode
/// step is ~2*HKV+1 descriptors instead of hundreds of 4-byte ranges.
typedef struct BrievPushDesc {
    const void* src;   // ABSOLUTE host source (a staging row, or state+off)
    size_t proj_off;   // projection (device/mirror) byte offset, element 0
    size_t count;      // element count
    size_t elem_bytes; // 2 (f16) or 4 (f32)
    size_t src_pitch;  // host stride between elements; elem_bytes = packed
    size_t dst_pitch;  // projection stride between elements
} BrievPushDesc;

int briev_accel_init(const BrievKernelDesc* descs, uint32_t n);
int briev_accel_available(void);
const char* briev_accel_device_name(void);
int briev_accel_download_written(uint32_t idx, void* state);
void briev_accel_invalidate_resident(void);
int briev_accel_launch(uint32_t idx, void* state, uint64_t work_n);
int briev_accel_launch_resident(uint32_t idx, void* state, uint64_t work_n);
int briev_accel_launch_resident_2d(uint32_t idx, void* state,
                                   uint64_t nx, uint64_t ny);
int briev_accel_launch_resident_batch(uint32_t idx, void* state,
                                      uint64_t nx, uint64_t ny, uint32_t times);
int briev_accel_download(uint32_t idx, void* state);
int briev_accel_push_ranges(void* state, const size_t* ranges, uint32_t n);
int briev_accel_push_strided(const BrievPushDesc* descs, uint32_t n);
void briev_accel_shutdown(void);
/// Auto-tune probe (plan smallm-splitk): run `cpu_fn` and `gpu_fn` on `ctx`,
/// confirm the outputs match within `tolerance`. Returns 1 = GPU, 0 = CPU.
/// `state_size` is the host %State byte count (the compiler emits it).
int briev_accel_probe(void (*cpu_fn)(void*), void (*gpu_fn)(void*), void* ctx,
                      uint64_t state_size, int64_t probe_k, double tolerance,
                      double margin,
                      int (*gpu_ok)(const void*, const void*, double, void*));

#ifdef __cplusplus
}
#endif

#endif  // BRIEV_ACCEL_RT_H
