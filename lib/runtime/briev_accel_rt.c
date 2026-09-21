// Briev Accel Runtime — device-agnostic GPU dispatch glue.
//
// The compiler never names a device. It emits SPIR-V kernel blobs + per-kernel
// layout descriptors + calls the stable briev_accel_* ABI below. This runtime
// dispatches to a pluggable device-driver table (BrievDeviceDriver). Kernel
// EMISSION is per device-FAMILY (CUDA needs PTX; Vulkan/OpenCL/LevelZero all
// consume the same SPIR-V), so the glue is shared. See
// docs/plans/2026-08-06-accel-gpu-offload.md §7.
//
// Device model: each kernel sees ONE flat projection buffer — the host %State
// sliced to the kernel's buffers in kernel `%State` field order (arrays then
// scalars, each sorted). The kernel GEPs into that struct; the device buffer
// holds exactly that packed struct. The generic pack/unpack here is
// device-independent; each driver only uploads, launches, downloads.
//
// Selection: BRIEV_ACCEL_DEVICE env (vulkan|opencl|...) overrides the default;
// otherwise the first available driver wins (Vulkan → OpenCL → CPU).

#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <dlfcn.h>

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

int briev_accel_launch_resident_2d(uint32_t idx, void* state,
                                   uint64_t nx, uint64_t ny);

// ────────────────────────────────────────────────────────────────────────────
// Device selection (config default + BRIEV_ACCEL_DEVICE env + fallback chain)
// ────────────────────────────────────────────────────────────────────────────

static const BrievDeviceDriver* g_driver = NULL;
static int g_init_done = 0;
static uint8_t* g_resident_seeded = NULL;   // per-kernel first-launch flag (residency)
// 2026-09-14 (gpu_schedule Phase 0): program-level residency seed for
// drivers with BRIEV_DEV_CAP_SHARED_STATE — the shared buffer is seeded
// once, then arrays persist on-device (scalars only upload dirty).
static int g_program_seeded = 0;
// 2026-08-31: shared with the #included device drivers (single TU) — they
// read it for BRIEV_ACCEL_VERBOSE diagnostics.
static int g_verbose = 0;
// 2026-09-14 (gpu_schedule Phase 2, sync elimination): kernels on ONE stream
// execute in submission order, so the per-launch cuStreamSynchronize is pure
// overhead — the node DAG's edges are already satisfied by stream order.
// When set (BRIEV_ACCEL_ASYNC=1), resident launches submit WITHOUT syncing;
// the sync happens at the download / explicit briev_accel_sync.
static int g_async_launch = 0;
static void** g_kernels = NULL;       // per-desc kernel handles
static uint32_t g_n_kernels = 0;
static const BrievKernelDesc* g_descs = NULL;

static const BrievDeviceDriver* select_driver(void) {
    const char* env = getenv("BRIEV_ACCEL_DEVICE");
    // 2026-09-08 (plan 2026-09-08-ptx-tier-execution S1): CUDA first — the
    // perf tier (PTX tensor cores). Falls to Vulkan when libcuda is absent.
    const BrievDeviceDriver* chain[] = { &briev_dev_cuda, &briev_dev_vulkan, &briev_dev_opencl, NULL };
    if (env != NULL && env[0] != '\0') {
        for (int i = 0; chain[i] != NULL; i++) {
            if (strcmp(chain[i]->name, env) == 0) {
                if (chain[i]->available()) {
                    return chain[i];
                }
            }
        }
        // env names a driver that is unavailable → fall through to the chain.
    }
    for (int i = 0; chain[i] != NULL; i++) {
        if (chain[i]->available()) {
            return chain[i];
        }
    }
    return NULL;
}

/// Register the kernel set. Call once from the emitted program's init.
/// Returns 1 when a device is active and all kernels compiled, 0 for CPU.
/// 2026-08-31 (plan abv-gpu-by-default): a failed device init or kernel
/// compile now marks the chain DEAD (available()==0 → clean CPU fallback).
/// Previously init()'s return was ignored and a failed create_kernel left
/// available()==1 — the GPU lane was "chosen" and launches no-op'd, and
/// there was no way to see why. BRIEV_ACCEL_VERBOSE=1 prints the reason.
int briev_accel_init(const BrievKernelDesc* descs, uint32_t n) {
    g_verbose = getenv("BRIEV_ACCEL_VERBOSE") != NULL;
    g_async_launch = getenv("BRIEV_ACCEL_ASYNC") != NULL;
    int verbose = g_verbose;
    if (!g_init_done) {
        g_driver = select_driver();
        if (g_driver != NULL) {
            if (!g_driver->init()) {
                if (verbose) {
                    fprintf(stderr, "[briev_accel] driver '%s' init failed — CPU fallback\n",
                            g_driver->name);
                }
                g_driver = NULL;
            }
        } else if (verbose) {
            fprintf(stderr, "[briev_accel] no device driver available — CPU fallback\n");
        }
        g_init_done = 1;
    }
    g_descs = descs;
    g_n_kernels = n;
    if (g_driver == NULL) {
        return 0;
    }
    if (g_kernels != NULL) {
        free(g_kernels);
    }
    g_kernels = calloc(n, sizeof(void*));
    if (g_kernels == NULL) {
        return 0;
    }
    free(g_resident_seeded);
    g_resident_seeded = calloc(n, 1);
    for (uint32_t i = 0; i < n; i++) {
        // 2026-09-17 (CyberLlama plan M2.0): per-driver blob selection —
        // the CUDA lane consumes PTX TEXT (desc.ptx, cuModuleLoadData
        // JITs it), Vulkan/OpenCL consume the SPIR-V blob. A kernel with
        // no image for the chosen driver is a per-kernel CPU fallback
        // slot, same contract as the old empty-blob skip (2026-08-31).
        const uint8_t* blob = descs[i].spirv;
        uint32_t blob_size = descs[i].spirv_size;
        if (strcmp(g_driver->name, "cuda") == 0) {
            blob = descs[i].ptx;
            blob_size = descs[i].ptx_size;
        }
        // An EMPTY blob is a per-kernel CPU fallback slot (the
        // compiler keeps descriptor indices stable) — skip, don't fail all.
        if (blob_size == 0) {
            if (verbose) {
                fprintf(stderr, "[briev_accel] kernel '%s' has no %s image — CPU lane\n",
                        descs[i].txn_name, g_driver->name);
            }
            g_kernels[i] = NULL;
            continue;
        }
        if (!g_driver->create_kernel(blob, blob_size, &g_kernels[i])) {
            if (verbose) {
                fprintf(stderr, "[briev_accel] kernel '%s' rejected by driver '%s' — CPU fallback\n",
                        descs[i].txn_name, g_driver->name);
            }
            g_driver = NULL;
            return 0;
        } else if (verbose) {
            fprintf(stderr, "[briev_accel] kernel '%s' compiled on '%s'\n",
                    descs[i].txn_name, g_driver->name);
        }
        // 2026-09-09 (S3b+ perf rungs): per-kernel block-size override.
        if (descs[i].block_threads > 0 && g_driver->set_block_threads != NULL) {
            g_driver->set_block_threads(g_kernels[i], descs[i].block_threads);
        }
        if (descs[i].shared_bytes > 0 && g_driver->set_shared_bytes != NULL) {
            g_driver->set_shared_bytes(g_kernels[i], descs[i].shared_bytes);
        }
        // 2026-09-18 (P1 lane-coverage fix): per-kernel block-per-workitem
        // dispatch flag — the CUDA driver multiplies nx by block_threads
        // when this is set (lane-mapped reduction: w = ctaid.x).
        if (descs[i].block_per_workitem && g_driver->set_block_per_workitem != NULL) {
            g_driver->set_block_per_workitem(g_kernels[i], descs[i].block_per_workitem);
        }
        // 2026-09-02: image-resident arrays need the driver's image path.
        // Absent = loud refusal (a silent skip would leave the image
        // descriptor unwritten and the kernel reading garbage).
        if (descs[i].n_images > 0 && g_driver->set_images == NULL) {
            fprintf(stderr,
                    "[briev_accel] kernel '%s' has %u image array(s) but "
                    "driver '%s' has no image support\n",
                    descs[i].txn_name, descs[i].n_images, g_driver->name);
            g_driver = NULL;
            return 0;
        }
        if (descs[i].n_images > 0
            && !g_driver->set_images(g_kernels[i], descs[i].images,
                                     descs[i].n_images)) {
            fprintf(stderr, "[briev_accel] image allocation failed for '%s'\n",
                    descs[i].txn_name);
            g_driver = NULL;
            return 0;
        }
    }
    return 1;
}

/// 1 when a device is active (after init), 0 → CPU path.
int briev_accel_available(void) {
    return (g_init_done && g_driver != NULL) ? 1 : 0;
}

/// The active DEVICE name (e.g. "NVIDIA GeForce RTX 3060"), or the driver
/// name, or "cpu". 2026-09-02: the device op when the driver provides it —
/// run diagnostics should name the GPU, not the API.
const char* briev_accel_device_name(void) {
    if (!g_init_done || g_driver == NULL) {
        return "cpu";
    }
    if (g_driver->device_name != NULL) {
        const char* n = g_driver->device_name();
        if (n != NULL) {
            return n;
        }
    }
    return g_driver->name;
}

// ────────────────────────────────────────────────────────────────────────────
// Generic pack/unpack + dispatch
// ────────────────────────────────────────────────────────────────────────────

/// Total device projection size. 2026-09-15 (Phase 3 enablement): the
/// compiler supplies `program_bytes` (the union extent over ALL buffer
/// fields) because per-kernel field tables list only the kernel's touched
/// fields — a kernel's SSBO struct still spans every member, so the buffer
/// must cover the union, not the touched subset. Falls back to the
/// field-derived end for older descriptors (program_bytes == 0).
static uint64_t proj_size(const BrievKernelDesc* k) {
    if (k->program_bytes != 0) {
        return k->program_bytes;
    }
    uint64_t end = 0;
    for (uint32_t j = 0; j < k->n_fields; j++) {
        uint64_t f_end = k->fields[j].proj_offset
                       + k->fields[j].count * k->fields[j].elem_bytes;
        if (f_end > end) { end = f_end; }
    }
    return end;
}

/// 2026-09-15 (Phase 3 enablement): seed the program's INPUT arrays (the
/// compiler's seed table) into the device mirror. Inputs are read-first;
/// write-first reuse targets are disjoint from inputs, so no input shares a
/// slot — the seed never clobbers a live value.
static void seed_program_fields(const BrievKernelDesc* k, const void* state,
                                void* mapped) {
    for (uint32_t i = 0; i < k->n_seed_fields; i++) {
        const BrievField* f = &k->seed_fields[i];
        memcpy((uint8_t*)mapped + f->proj_offset,
               (const uint8_t*)state + f->host_offset,
               (size_t)(f->count * f->elem_bytes));
    }
}

/// Pack the kernel's fields from the host %State into a flat projection
/// (kernel field order), launch, then unpack written fields back.
/// Pull ONLY the kernel-written fields VRAM→staging→host (plan
/// gpu-backend-hardening Track B): the resident launch's outputs become
/// visible in host state without re-copying read-only inputs. Fields with
/// is_write=0 are skipped (their staging is stale by design and unused).
int briev_accel_download_written(uint32_t idx, void* state) {
    int shared = g_driver && (g_driver->capabilities & BRIEV_DEV_CAP_SHARED_STATE) != 0;
    int seeded = shared ? g_program_seeded : (idx < 32 && g_resident_seeded[idx]);
    if (!briev_accel_available() || idx >= g_n_kernels || g_kernels == NULL
        || g_kernels[idx] == NULL || !seeded) {
        return 0;
    }
    void* mapped = g_driver->mapped(g_kernels[idx]);
    if (mapped == NULL) {
        return 0;
    }
    if (g_driver->download_dev != NULL) {
        g_driver->download_dev(g_kernels[idx]);
    }
    const BrievKernelDesc* k = &g_descs[idx];
    // 2026-09-02: image-resident arrays pull via the driver's image path —
    // they are not in fields[] (not SSBO members).
    if (k->n_images > 0) {
        if (g_driver->download_images == NULL) {
            fprintf(stderr,
                    "[briev_accel] kernel has %u image array(s) but the device "
                    "driver cannot download images — no readback\n",
                    k->n_images);
        } else if (!g_driver->download_images(g_kernels[idx], k->images,
                                              k->n_images, state)) {
            fprintf(stderr, "[briev_accel] image download failed\n");
        }
    }
    for (uint32_t i = 0; i < k->n_fields; i++) {
        const BrievField* f = &k->fields[i];
        if (!f->is_write) {
            continue;
        }
        size_t n = (size_t)(f->count * f->elem_bytes);
        memcpy((uint8_t*)state + f->host_offset, mapped + f->proj_offset, n);
    }
    return 1;
}

/// CPU-fallback coherence (Track B): after a CPU accel body writes HOST
/// state, the VRAM working set is stale — clear the seed so the next
/// resident launch re-pushes ALL fields from the host. The .bv wrapper's
/// accel_cpu block calls this.
void briev_accel_invalidate_resident(void) {
    for (uint32_t i = 0; i < g_n_kernels && i < 32; i++) {
        g_resident_seeded[i] = 0;
    }
    g_program_seeded = 0;
}

int briev_accel_launch(uint32_t idx, void* state, uint64_t work_n) {
    if (!briev_accel_available() || idx >= g_n_kernels || g_kernels == NULL
        || g_kernels[idx] == NULL) {
        return 0;
    }
    const BrievKernelDesc* k = &g_descs[idx];
    uint64_t bytes = proj_size(k);
    uint8_t* proj = malloc(bytes == 0 ? 1 : bytes);
    uint8_t* proj_out = malloc(bytes == 0 ? 1 : bytes);
    if (proj == NULL || proj_out == NULL) {
        free(proj);
        free(proj_out);
        return 0;
    }
    // Pack: copy each field from its host offset into projection order.
    for (uint32_t i = 0; i < k->n_fields; i++) {
        const BrievField* f = &k->fields[i];
        size_t n = (size_t)(f->count * f->elem_bytes);
        memcpy(proj + f->proj_offset, (const uint8_t*)state + f->host_offset, n);
    }
    int ok = g_driver->launch(g_kernels[idx], proj, bytes, work_n, proj_out);
    // Unpack: copy written fields back to the host state.
    for (uint32_t i = 0; i < k->n_fields; i++) {
        const BrievField* f = &k->fields[i];
        if (!f->is_write) {
            continue;
        }
        size_t n = (size_t)(f->count * f->elem_bytes);
        memcpy((uint8_t*)state + f->host_offset, proj_out + f->proj_offset, n);
    }
    free(proj);
    free(proj_out);
    return ok;
}

// ────────────────────────────────────────────────────────────────────────────
// Device residency (2026-08-31, plan abv-gpu-by-default item 3): iterative
// kernels keep their array state ON the device across launches. Only scalar
// fields (counters, phase gates) cross PCIe each step. `launch_resident`
// seeds all fields on the first call, then syncs scalars both ways;
// `download` pulls the full projection back at the end. Requires the driver
// to expose its persistent mapped projection (Vulkan does; otherwise falls
// back to the full-copy launch).
// ────────────────────────────────────────────────────────────────────────────

int briev_accel_launch_resident(uint32_t idx, void* state, uint64_t work_n) {
    return briev_accel_launch_resident_2d(idx, state, work_n, 1);
}

// 2D resident launch (plan 2026-08-31-gpu-next §2b): ny == 1 is the flat
// form. Scalars sync host→device as dirty byte ranges; the first launch
// seeds the full projection. Everything else stays in VRAM.
int briev_accel_launch_resident_2d(uint32_t idx, void* state,
                                   uint64_t nx, uint64_t ny) {
    if (!briev_accel_available() || idx >= g_n_kernels || g_kernels == NULL
        || g_kernels[idx] == NULL) {
        return 0;
    }
    if (g_driver->launch_dev2d == NULL || g_driver->mapped == NULL
        || g_driver->launch_dev == NULL) {
        fprintf(stderr, "[briev_accel] DBG fallback: 2d=%p mapped=%p dev=%p\n",
                (void*)(size_t)!!g_driver->launch_dev2d,
                (void*)(size_t)!!g_driver->mapped,
                (void*)(size_t)!!g_driver->launch_dev);
        return briev_accel_launch(idx, state, nx * ny);  // driver can't
    }
    void* mapped = g_driver->mapped(g_kernels[idx]);
    const BrievKernelDesc* k = &g_descs[idx];
    if (mapped == NULL) {
        // The vulkan driver creates its buffer + map LAZILY on the first
        // full-copy launch — prime it once, then take the resident path.
        // A second NULL means the driver genuinely cannot go resident.
        //
        // 2026-09-21 (BUGS.md 2026-09-20, root cause found): the prime is
        // a FULL launch — seed + dispatch + download — and its download
        // replaces host state with run-1 OUTPUTS. The resident seed below
        // then uploads those clobbered bytes as the "zero-on-entry"
        // inputs, and dispatch #2 re-accumulates: any kernel that
        // read-modify-writes a seeded scratch (composite deferred softmax:
        // acc += p*v) lands at exactly 2x. Snapshot the seed spans before
        // the prime, restore the authored bytes after it — the resident
        // seed then uploads what the program author wrote. (The prime's
        // device-side work is discarded: full_sync re-seeds every seed
        // field, including the scratch.) One-time cost per program.
        size_t snap_bytes = 0;
        for (uint32_t i = 0; i < k->n_seed_fields; i++) {
            const BrievField* f = &k->seed_fields[i];
            snap_bytes += (size_t)(f->count * f->elem_bytes);
        }
        uint8_t* snap = (uint8_t*)malloc(snap_bytes ? snap_bytes : 1);
        if (snap == NULL) {
            return 0;  // fail closed: a silent 2x beats a failed launch
        }
        size_t snap_off = 0;
        for (uint32_t i = 0; i < k->n_seed_fields; i++) {
            const BrievField* f = &k->seed_fields[i];
            size_t n = (size_t)(f->count * f->elem_bytes);
            memcpy(snap + snap_off, (const uint8_t*)state + f->host_offset, n);
            snap_off += n;
        }
        int primed = briev_accel_launch(idx, state, nx * ny);
        snap_off = 0;
        for (uint32_t i = 0; i < k->n_seed_fields; i++) {
            const BrievField* f = &k->seed_fields[i];
            size_t n = (size_t)(f->count * f->elem_bytes);
            memcpy((uint8_t*)state + f->host_offset, snap + snap_off, n);
            snap_off += n;
        }
        free(snap);
        if (!primed) {
            return 0;
        }
        mapped = g_driver->mapped(g_kernels[idx]);
        if (mapped == NULL) {
            return 0;
        }
    }
    int full_sync = 0;
    size_t dirty[2 * 16];
    uint32_t n_dirty = 0;
    // 2026-09-14 (gpu_schedule Phase 0): with a shared device buffer the
    // FULL projection is seeded once (program-level); every later launch
    // uploads scalars only, so producer array writes persist on-device for
    // consumers. Per-kernel drivers keep the historical per-kernel seed.
    int shared = (g_driver->capabilities & BRIEV_DEV_CAP_SHARED_STATE) != 0;
    int seeded = shared ? g_program_seeded : (idx < 32 && g_resident_seeded[idx]);
    if (!seeded) {
        full_sync = 1;
        // 2026-09-15 (Phase 3 enablement): seed the program's INPUT arrays
        // (the compiler's seed table), not every touched field. Inputs are
        // read-first; reuse targets are write-first and disjoint, so no
        // input shares a slot — the seed never clobbers a live value.
        seed_program_fields(k, state, mapped);
        if (shared) {
            g_program_seeded = 1;
        } else if (idx < 32) {
            g_resident_seeded[idx] = 1;
        }
    } else {
        // Scalars only: the host's counters/phase gates are authoritative
        // between launches (the phase machine runs on the host).
        for (uint32_t i = 0; i < k->n_fields && n_dirty < 16; i++) {
            const BrievField* f = &k->fields[i];
            if (f->kind != BRIEV_FIELD_SCALAR) {
                continue;
            }
            uint64_t off = f->proj_offset;
            dirty[2 * n_dirty] = off;
            dirty[2 * n_dirty + 1] = f->elem_bytes;
            n_dirty++;
            memcpy(mapped + off,
                   (const uint8_t*)state + f->host_offset,
                   (size_t)f->elem_bytes);
        }
    }
    // No device→host scalar sync: the host owns the scalars (they were just
    // uploaded); arrays stay device-resident until briev_accel_download.
    int ok = g_driver->launch_dev2d(g_kernels[idx], nx, ny, full_sync,
                                     dirty, n_dirty);
    // 2026-09-18 (multi-kernel composition): after the FIRST resident
    // dispatch (full_sync=1), the output arrays live on-device but NOT in
    // host state. The next kernel's seed_program_fields reads state — stale
    // zeros would be uploaded. Pull written fields back after each full_sync
    // dispatch so subsequent kernels see correct inputs.
    if (ok && full_sync) {
        briev_accel_download_written(idx, state);
    }
    return ok;
}

/// Batched resident launch (plan 2026-09-01-smallm-splitk): `times`
/// IDENTICAL dispatches in one submission. The fence wake (~33us) and the
/// submit cost (~7us) amortize once per batch instead of once per launch —
/// loop deployments (inference steps, benchmark iterations) see per-call
/// cost ≈ kernel time. CONTRACT: the host's scalar state must be
/// launch-invariant across the batch (e.g. the cooperative kernel's i=0
/// reset); scalars sync ONCE before the dispatches. Returns 1 on success.
int briev_accel_launch_resident_batch(uint32_t idx, void* state,
                                      uint64_t nx, uint64_t ny, uint32_t times) {
    if (!briev_accel_available() || idx >= g_n_kernels || g_kernels == NULL
        || g_kernels[idx] == NULL || times == 0) {
        return 0;
    }
    if (g_driver->launch_dev2d_batch == NULL || g_driver->mapped == NULL
        || g_driver->launch_dev == NULL) {
        // Driver can't batch: sequential fallback (identical semantics).
        for (uint32_t t = 0; t < times; t++) {
            if (!briev_accel_launch_resident_2d(idx, state, nx, ny)) {
                return 0;
            }
        }
        return 1;
    }
    void* mapped = g_driver->mapped(g_kernels[idx]);
    if (mapped == NULL) {
        for (uint32_t t = 0; t < times; t++) {
            if (!briev_accel_launch_resident_2d(idx, state, nx, ny)) {
                return 0;
            }
        }
        return 1;
    }
    const BrievKernelDesc* k = &g_descs[idx];
    int full_sync = 0;
    size_t dirty[2 * 16];
    uint32_t n_dirty = 0;
    int shared = (g_driver->capabilities & BRIEV_DEV_CAP_SHARED_STATE) != 0;
    int seeded = shared ? g_program_seeded : (idx < 32 && g_resident_seeded[idx]);
    if (!seeded) {
        full_sync = 1;
        // 2026-09-15 (Phase 3 enablement): seed the program's INPUT arrays.
        seed_program_fields(k, state, mapped);
        if (shared) {
            g_program_seeded = 1;
        } else if (idx < 32) {
            g_resident_seeded[idx] = 1;
        }
    } else {
        // Scalars sync ONCE — the batch contract is launch-invariant scalars.
        for (uint32_t i = 0; i < k->n_fields && n_dirty < 16; i++) {
            const BrievField* f = &k->fields[i];
            if (f->kind != BRIEV_FIELD_SCALAR) {
                continue;
            }
            uint64_t off = f->proj_offset;
            dirty[2 * n_dirty] = off;
            dirty[2 * n_dirty + 1] = f->elem_bytes;
            n_dirty++;
            memcpy(mapped + off,
                   (const uint8_t*)state + f->host_offset,
                   (size_t)f->elem_bytes);
        }
    }
    return g_driver->launch_dev2d_batch(g_kernels[idx], nx, ny, times, full_sync,
                                        dirty, n_dirty);
}

/// Pull the FULL projection back to the host state (end of a resident run —
/// observables read host state). Returns 0 when residency isn't active for
/// `idx` (the caller's state is then already current from full-copy launches).
int briev_accel_download(uint32_t idx, void* state) {
    int shared = g_driver && (g_driver->capabilities & BRIEV_DEV_CAP_SHARED_STATE) != 0;
    int seeded = shared ? g_program_seeded : (idx < 32 && g_resident_seeded[idx]);
    if (!briev_accel_available() || idx >= g_n_kernels || g_kernels == NULL
        || g_kernels[idx] == NULL || !seeded) {
        return 0;
    }
    void* mapped = g_driver->mapped(g_kernels[idx]);
    if (mapped == NULL) {
        return 0;
    }
    // 2026-09-01: with the device-local working set, the resident launch's
    // results land in the VRAM buffer — pull them into the staging window
    // before the host-side copy (the mapped read is stale otherwise). A 0
    // return means the all-host fallback, where staging is already current.
    if (g_driver->download_dev != NULL) {
        g_driver->download_dev(g_kernels[idx]);
    }
    const BrievKernelDesc* k = &g_descs[idx];
    // 2026-09-02: image-resident arrays pull via the driver's image path —
    // they are not SSBO fields (their bytes never touch the staging window).
    if (k->n_images > 0) {
        if (g_driver->download_images == NULL) {
            fprintf(stderr,
                    "[briev_accel] kernel '%s' has %u image array(s) but the "
                    "driver cannot download images — no readback\n",
                    k->txn_name, k->n_images);
        } else if (!g_driver->download_images(g_kernels[idx], k->images,
                                              k->n_images, state)) {
            fprintf(stderr, "[briev_accel] image download failed for '%s'\n",
                    k->txn_name);
        }
    }
    for (uint32_t i = 0; i < k->n_fields; i++) {
        const BrievField* f = &k->fields[i];
        memcpy((uint8_t*)state + f->host_offset,
               mapped + f->proj_offset,
               (size_t)(f->count * f->elem_bytes));
    }
    return 1;
}

/// 2026-09-18 (M4 decode-append, plan 2026-09-18-m3-matrix-and-m4-fattn-shim):
/// push array byte ranges from the host staging `state` into the device
/// working set WITHOUT a dispatch. Decode attention appends one KV row per
/// step between kernel chains; the launch-time dirty path covers scalar
/// counters only. `ranges` holds TRIPLES: projection offset, host offset,
/// bytes — the two layouts differ (alignment padding). Contract: the
/// program must already be seeded through the shared-state resident path
/// and previous launches synchronized (the default sync-launch contract).
/// Requires the driver's upload_ranges hook; returns 0 when unavailable.
int briev_accel_push_ranges(void* state, const size_t* ranges, uint32_t n) {
    if (!briev_accel_available() || g_driver == NULL || g_kernels == NULL
        || g_n_kernels == 0 || g_kernels[0] == NULL || !g_program_seeded
        || (g_driver->capabilities & BRIEV_DEV_CAP_SHARED_STATE) == 0) {
        return 0;
    }
    if (g_driver->upload_ranges == NULL || g_driver->mapped == NULL) {
        return 0;
    }
    void* mapped = g_driver->mapped(g_kernels[0]);
    if (mapped == NULL) {
        return 0;
    }
    size_t* dev_ranges = (size_t*)malloc(2 * sizeof(size_t) * n);
    if (dev_ranges == NULL) {
        return 0;
    }
    for (uint32_t r = 0; r < n; r++) {
        size_t proj_off = ranges[3 * r];
        size_t len = ranges[3 * r + 2];
        memcpy(mapped + proj_off,
               (const uint8_t*)state + ranges[3 * r + 1], len);
        dev_ranges[2 * r] = proj_off;
        dev_ranges[2 * r + 1] = len;
    }
    int ok = g_driver->upload_ranges(g_kernels[0], dev_ranges, n);
    free(dev_ranges);
    return ok;
}

/// 2026-09-18 (M2 strided append, plan coalesced-kv-memory-path): the
/// public strided-push descriptor. A d-major K append writes ONE new token
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

/// 2026-09-18 (M2 strided append): push strided element ranges from the
/// host staging `state` into the device working set WITHOUT a dispatch —
/// the pitched-copy generalization of briev_accel_push_ranges. The host
/// mirror is gathered first (coherence with download/mirror readers),
/// then the driver performs the pitched host→device copies in one call
/// per descriptor (CUDA: cuMemcpy2D, height 1). Same contract as
/// push_ranges: program seeded through the shared-state resident path,
/// previous launches synchronized. Returns 0 when the driver lacks the
/// hook (a failed push leaves the mirror partially gathered; treat 0 as
/// fatal, the caller aborts).
int briev_accel_push_strided(const BrievPushDesc* descs, uint32_t n) {
    if (!briev_accel_available() || g_driver == NULL || g_kernels == NULL
        || g_n_kernels == 0 || g_kernels[0] == NULL || !g_program_seeded
        || (g_driver->capabilities & BRIEV_DEV_CAP_SHARED_STATE) == 0) {
        return 0;
    }
    if (g_driver->push_strided == NULL || g_driver->mapped == NULL) {
        return 0;
    }
    void* mapped = g_driver->mapped(g_kernels[0]);
    if (mapped == NULL) {
        return 0;
    }
    // Host mirror gather (element-wise; descriptors are few and rows are
    // 512 B—2 KB, CPU cost negligible next to the HtoD) + driver-level
    // copies: both read the descriptor's ABSOLUTE source pointer.
    BrievStridedCopy* copies =
        (BrievStridedCopy*)malloc(sizeof(BrievStridedCopy) * n);
    if (copies == NULL) {
        return 0;
    }
    for (uint32_t r = 0; r < n; r++) {
        const BrievPushDesc* dsc = &descs[r];
        uint8_t* dst = (uint8_t*)mapped + dsc->proj_off;
        for (size_t i = 0; i < dsc->count; i++) {
            memcpy(dst + i * dsc->dst_pitch,
                   (const uint8_t*)dsc->src + i * dsc->src_pitch,
                   dsc->elem_bytes);
        }
        copies[r].src = dsc->src;
        copies[r].proj_off = dsc->proj_off;
        copies[r].src_pitch = dsc->src_pitch;
        copies[r].dst_pitch = dsc->dst_pitch;
        copies[r].width_bytes = dsc->count * dsc->elem_bytes;
        copies[r].count = dsc->count;
    }
    int ok = g_driver->push_strided(g_kernels[0], copies, n);
    free(copies);
    return ok;
}

/// Free kernel handles + driver shutdown. Called by program exit.
void briev_accel_shutdown(void) {    if (g_driver != NULL && g_kernels != NULL) {
        for (uint32_t i = 0; i < g_n_kernels; i++) {
            if (g_kernels[i] != NULL) {
                g_driver->destroy_kernel(g_kernels[i]);
            }
        }
        g_driver->shutdown();
    }
    free(g_resident_seeded);
    g_resident_seeded = NULL;
    free(g_kernels);
    g_kernels = NULL;
    g_driver = NULL;
    g_init_done = 0;
}

// ────────────────────────────────────────────────────────────────────────────
// Auto-tuning probe (D7). Runs both lanes on a slice; verifies output
// equality within tolerance and commits to the faster path. Returns 1 = GPU.
// ────────────────────────────────────────────────────────────────────────────

#include <time.h>

static double now_seconds(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

/// Auto-tuning probe (D7): runs the CPU and GPU lanes on SEPARATE state
/// copies, times each over `probe_k` full-map runs, and commits to the GPU
/// path only when its wall time beats CPU by the margin AND `gpu_ok`
/// confirms the outputs match within `tolerance`. Returns 1 = GPU, 0 = CPU.
/// `state_size` is the host %State byte count (the compiler emits it).
int briev_accel_probe(void (*cpu_fn)(void*), void (*gpu_fn)(void*), void* ctx,
                     uint64_t state_size, int64_t probe_k, double tolerance,
                     double margin,
                     int (*gpu_ok)(const void*, const void*, double, void*)) {
    if (!briev_accel_available() || probe_k <= 0) {
        return 0;
    }
    uint8_t* cpu_state = malloc(state_size == 0 ? 1 : state_size);
    uint8_t* gpu_state = malloc(state_size == 0 ? 1 : state_size);
    if (cpu_state == NULL || gpu_state == NULL) {
        free(cpu_state);
        free(gpu_state);
        return 0;
    }
    memcpy(cpu_state, ctx, state_size);
    memcpy(gpu_state, ctx, state_size);

    // Warm-up: one dummy full-map run each (first device dispatch is slow).
    cpu_fn(cpu_state);
    gpu_fn(gpu_state);

    double t0 = now_seconds();
    for (int64_t i = 0; i < probe_k; i++) {
        cpu_fn(cpu_state);
    }
    double cpu_t = now_seconds() - t0;

    t0 = now_seconds();
    for (int64_t i = 0; i < probe_k; i++) {
        gpu_fn(gpu_state);
    }
    double gpu_t = now_seconds() - t0;

    // Correctness gate: the GPU lane's result must match the CPU lane's within
    // tolerance — the probe doubles as the safety net against GPU codegen bugs.
    if (gpu_ok != NULL && !gpu_ok(cpu_state, gpu_state, tolerance, ctx)) {
        free(cpu_state);
        free(gpu_state);
        return 0;
    }
    free(cpu_state);
    free(gpu_state);
    return (gpu_t * (1.0 + margin) < cpu_t) ? 1 : 0;
}

// ────────────────────────────────────────────────────────────────────────────
// Drivers — Vulkan and OpenCL, ported from the legacy briev_gpu_rt.c dual-API
// (dlopen'd, both SPIR-V consumers), restructured to the single-flat-buffer
// model. The generic pack/selection/probe above is complete; these drivers
// carry over the original mechanism and its known simplifications (see the
// per-driver header comments) until hardened against real hardware.
// ────────────────────────────────────────────────────────────────────────────

#include "briev_dev_cuda.c"
#include "briev_dev_vulkan.c"
#include "briev_dev_opencl.c"

// ────────────────────────────────────────────────────────────────────────────
// Self-test (BRIEV_ACCEL_SELF_TEST) — exercises selection, pack math, and the
// probe gate on synthetic data with NO device required. Built standalone:
//   cc -DBRIEV_ACCEL_SELF_TEST briev_accel_rt.c -ldl -o /tmp/briev_accel_selftest
// ────────────────────────────────────────────────────────────────────────────

#ifdef BRIEV_ACCEL_SELF_TEST

static int self_failures = 0;
static int reject_hits = 0;

static void probe_cpu_fn(void* ctx);
static void probe_gpu_fn(void* ctx);
static int probe_reject_ok(const void* a, const void* b, double tol, void* ctx);

static void expect(int cond, const char* what) {
    if (!cond) {
        fprintf(stderr, "SELF-TEST FAIL: %s\n", what);
        self_failures++;
    }
}

static int fake_launch(void* k, const void* proj, size_t bytes,
                       size_t n, void* proj_out) {
    (void)k;
    // Copy proj to proj_out (identity) so write fields round-trip.
    memcpy(proj_out, proj, bytes);
    (void)n;
    return 1;
}

int main(void) {
    // Field projection order + offsets (declared): array a (Float[4], 4B)
    // is vec4-eligible and 16B-aligned at proj 0 (16B); scalar s packed at
    // proj 16 (8B). Host struct has count first (8B) so a's host_offset is
    // 8, s's is 24 — host layouts stay packed; only the projection aligns.
    BrievField fields[2] = {
        { "a", BRIEV_FIELD_ARRAY, 8, 4, 4, 1, 0 },
        { "s", BRIEV_FIELD_SCALAR, 24, 8, 1, 0, 16 },
    };
    BrievKernelDesc desc = { "force", NULL, 0, 2, fields };

    // ── Pack math ──
    expect(proj_size(&desc) == 24, "proj_size = 16 + 8");

    // ── Launch with the fake driver ──
    static BrievDeviceDriver fake_driver = {
        "fake", 0, NULL, NULL, NULL, fake_launch, NULL, NULL,
    };
    g_driver = &fake_driver;
    g_init_done = 1;
    g_descs = &desc;
    g_n_kernels = 1;
    g_kernels = malloc(sizeof(void*));
    g_kernels[0] = (void*)0x1;

    uint64_t host[4] = { 0 };
    float* a = (float*)((uint8_t*)host + 8);
    a[0] = 1.0f; a[1] = 2.0f; a[2] = 3.0f; a[3] = 4.0f;
    ((int64_t*)((uint8_t*)host + 24))[0] = 42;
    int ok = briev_accel_launch(0, host, 4);
    expect(ok == 1, "fake launch returns ok");
    expect(a[0] == 1.0f && a[3] == 4.0f, "write field round-trips");
    expect(((int64_t*)((uint8_t*)host + 24))[0] == 42, "scalar preserved");

    // ── Probe gate ──
    // The probe returns GPU(1) only when gpu_ok confirms output equality; a
    // rejecting gate forces CPU regardless of timing. Runs a real clocked
    // loop with the fake driver installed above (gpu_ok always rejects).
    {
        static struct ProbeCtx { int n; } pctx = { 4 };
        int verdict = briev_accel_probe(probe_cpu_fn, probe_gpu_fn, &pctx,
                                       sizeof(pctx), 2, 0.001, 0.05,
                                       probe_reject_ok);
        expect(verdict == 0, "rejecting gate forces CPU");
    }
    (void)reject_hits;

    if (self_failures == 0) {
        printf("briev_accel_rt self-test: all passed\n");
        return 0;
    }
    return 1;
}

static void probe_cpu_fn(void* ctx) { (void)ctx; }
static void probe_gpu_fn(void* ctx) { reject_hits++; (void)ctx; }
static int probe_reject_ok(const void* a, const void* b, double tol, void* ctx) {
    (void)a; (void)b; (void)tol; (void)ctx;
    return 0;
}

#endif
