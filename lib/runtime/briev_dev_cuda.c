// CUDA device driver for briev_accel_rt — PTX via the CUDA driver API.
//
// 2026-09-08 (plan 2026-09-08-ptx-tier-execution, S1): mirrors the Vulkan
// driver's shape exactly — dlopen("libcuda.so.1"), a dlsym table, and raw
// numeric enums (verified against /opt/cuda/.../include/cuda.h) so the
// driver compiles with NO CUDA headers. The kernel blob it consumes is PTX
// TEXT bytes (the compiler emits them — S2); cuModuleLoadData JITs via the
// driver's internal ptxas. A single source PTX runs on every CUDA device
// family (3060 sm_86 + 1070 Ti sm_61 from the same blob).
//
// Projection semantics are identical to the Vulkan driver: each kernel sees
// ONE flat projection buffer (the packed %State slice); this driver uploads,
// launches, downloads. Residency uses a page-locked host mirror
// (cuMemAllocHost) + a persistent device working set; dirty ranges cross as
// (offset, bytes) pairs, mirroring launch_dev2d's contract.
//
// Selection: BRIEV_ACCEL_DEVICE=cuda, or the default chain (CUDA first — the
// perf tier). Absent libcuda → available() returns 0 and the chain falls to
// Vulkan then OpenCL then CPU. The SPIR-V path stays byte-identical when the
// probe selects Vulkan.

#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <dlfcn.h>

// ── CUDA driver API surface (raw numeric values from cuda.h) ──────────────

typedef int CUdevice;
typedef struct CUctx_st* CUcontext;
typedef struct CUmod_st* CUmodule;
typedef struct CUfunc_st* CUfunction;
typedef struct CUstream_st* CUstream;
typedef unsigned long long CUdeviceptr;

#define CUDA_SUCCESS 0
// CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR / MINOR (used to pick a
// compute-capable device; the 1070 Ti is sm_61, the 3060 sm_86).
#define CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR 75
#define CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR 76
// CU_FUNC_ATTRIBUTE_MAX_DYNAMIC_SHARED_SIZE_BYTES — the opt-in for >48KB
// dynamic shared memory (the tensor tier's smem tiles).
#define CU_FUNC_ATTRIBUTE_MAX_DYNAMIC_SHARED_SIZE_BYTES 8
// CU_CTX_SCHED_AUTO.
#define CU_CTX_SCHED_AUTO 0x00u

static void* cu_lib = NULL;
static int cu_ready = 0;
static CUdevice cu_device;
static CUcontext cu_ctx;
static CUstream cu_stream;
static char cu_device_name[256] = "cuda";
static uint32_t cu_local_x = 64;  // keep in step with VK_LOCAL_SIZE_X

typedef int (*cu_init_fn)(unsigned);
typedef int (*cu_ret1)(int*);
typedef int (*cu_ret_dev)(CUdevice*, int);
typedef int (*cu_name)(char*, int, CUdevice);
typedef int (*cu_attr)(int*, int, CUdevice);
typedef int (*cu_ctx_create)(CUcontext*, unsigned, CUdevice);
// 2026-09-18 (M2): CUDA 13's cuMemcpy2D does not see contexts created by
// the legacy cuCtxCreate entry (it no-ops returning success) — the v4
// entry is the compatible one. Temp probe evidence: probe_repl.c.
typedef int (*cu_ctx_create_v4)(CUcontext*, void*, unsigned, CUdevice);
typedef int (*cu_module_load)(CUmodule*, const void*);
typedef int (*cu_module_load_ex)(CUmodule*, const void*, unsigned int,
                                 unsigned int*, void**);
typedef int (*cu_func_get_attribute)(int*, int, CUfunction);
typedef int (*cu_module_func)(CUfunction*, CUmodule, const char*);
typedef int (*cu_mem_alloc)(CUdeviceptr*, size_t);
typedef int (*cu_mem_free)(CUdeviceptr);
typedef int (*cu_memcpy_htod)(CUdeviceptr, const void*, size_t);
typedef int (*cu_memcpy_dtoh)(void*, CUdeviceptr, size_t);
// CUDA_MEMCPY2D shim (driver ABI, 64-bit Linux: 16 eight-byte fields,
// no padding surprises). Only the host→device pitched form is used.
#define CU_MEMORYTYPE_HOST 1
#define CU_MEMORYTYPE_DEVICE 2
typedef struct CuMemcpy2D {
    size_t srcXInBytes;
    size_t srcY;
    // CUmemorytype is a 4-byte ENUM in cuda.h — using size_t here shifted
    // every following field and the copies silently landed nowhere (the
    // M2 verify caught it; /opt/cuda/include/cuda.h is the reference).
    unsigned int srcMemoryType;
    const void* srcHost;
    CUdeviceptr srcDevice;
    void* srcReserved;
    size_t srcPitch;
    size_t dstXInBytes;
    size_t dstY;
    unsigned int dstMemoryType;
    void* dstHost;
    CUdeviceptr dstDevice;
    void* dstReserved;
    size_t dstPitch;
    size_t WidthInBytes;
    size_t Height;
} CuMemcpy2D;
typedef int (*cu_memcpy_2d)(const CuMemcpy2D*);
typedef int (*cu_launch)(CUfunction, unsigned, unsigned, unsigned,
                         unsigned, unsigned, unsigned, unsigned, CUstream,
                         void**, void**);
typedef int (*cu_func_attr)(CUfunction, int, int);
typedef int (*cu_stream_sync)(CUstream);
typedef int (*cu_get_err)(int, const char**);
typedef int (*cu_mem_host_alloc)(void**, size_t, unsigned);
typedef int (*cu_mem_host_free)(void*);
typedef int (*cu_module_unload)(CUmodule);
typedef int (*cu_ctx_destroy)(CUcontext);
typedef int (*cu_stream_create)(CUstream*, unsigned);

static cu_init_fn p_cuInit = NULL;
static cu_ret1 p_cuDeviceGetCount = NULL;
static cu_ret_dev p_cuDeviceGet = NULL;
static cu_name p_cuDeviceGetName = NULL;
static cu_attr p_cuDeviceGetAttribute = NULL;
static cu_ctx_create p_cuCtxCreate = NULL;
static cu_ctx_create_v4 p_cuCtxCreate_v4 = NULL;
static cu_module_load p_cuModuleLoadData = NULL;
static cu_module_load_ex p_cuModuleLoadDataEx = NULL;
static cu_func_get_attribute p_cuFuncGetAttribute = NULL;
static cu_module_func p_cuModuleGetFunction = NULL;
static cu_mem_alloc p_cuMemAlloc = NULL;
static cu_mem_free p_cuMemFree = NULL;
static cu_memcpy_htod p_cuMemcpyHtoD = NULL;
static cu_memcpy_2d p_cuMemcpy2D = NULL;
static cu_memcpy_dtoh p_cuMemcpyDtoH = NULL;
static cu_launch p_cuLaunchKernel = NULL;
static cu_func_attr p_cuFuncSetAttribute = NULL;
static cu_stream_sync p_cuStreamSynchronize = NULL;
static cu_get_err p_cuGetErrorString = NULL;
static cu_mem_host_alloc p_cuMemAllocHost = NULL;
static cu_mem_host_free p_cuMemFreeHost = NULL;
static cu_module_unload p_cuModuleUnload = NULL;
static cu_ctx_destroy p_cuCtxDestroy = NULL;
static cu_stream_create p_cuStreamCreate = NULL;

/// One compiled PTX kernel + its device resources.
typedef struct {
    CUmodule module;
    CUfunction func;
    // Device residency: a page-locked host mirror (`mapped_host`) + the
    // persistent device working set (`dev`). `bytes` is the projection size.
    // 2026-09-14 (gpu_schedule Phase 0): ALL kernels share ONE program-level
    // buffer + mirror (see g_shared_*) so a producer kernel's array writes
    // are visible to consumers — cross-kernel D2D within a single allocation.
    CUdeviceptr dev;
    void* mapped_host;
    size_t bytes;
    char name[128];
    // 2026-09-09 (S3b+ perf rungs): block thread count for THIS kernel
    // (default 64 — `cu_local_x`). The multi-warp tensor kernels launch
    // mw*nw*32-thread blocks.
    uint32_t block_threads;
    // 2026-09-10 (cp.async stages): dynamic shared-memory bytes for THIS
    // kernel (0 = none). The staged mw kernel's stage arrays live here.
    uint32_t shared_bytes;
    // 2026-09-18 (P1 lane-coverage fix): when set, the launch multiplies
    // nx by block_threads so the driver's gx = ceil(n*bx/bx) = n blocks.
    // The PTX kernel's lane-mapped reduction treats each block as one
    // work item (w = ctaid.x) with all64 threads computing redundantly.
    uint32_t block_per_workitem;
} BrievCudaKernel;

// 2026-09-14 (gpu_schedule Phase 0): the program-level shared device state.
// The full projection lives in ONE device allocation + ONE page-locked host
// mirror; every kernel's BrievCudaKernel.dev/mapped_host alias it. First
// prime allocates; the runtime seeds once (program-level residency).
static CUdeviceptr g_shared_dev = 0;
static void* g_shared_host = NULL;
static size_t g_shared_bytes = 0;

static int cu_resolve(void) {
    if (cu_ready) {
        return 1;
    }
    if (cu_lib != NULL) {
        return 0;  // dlopen failed before — don't retry
    }
    cu_lib = dlopen("libcuda.so.1", RTLD_NOW | RTLD_LOCAL);
    if (cu_lib == NULL) {
        return 0;
    }
    #define CU_SYM(n) p_##n = (void*)dlsym(cu_lib, #n)
    CU_SYM(cuInit);
    CU_SYM(cuDeviceGetCount);
    CU_SYM(cuDeviceGet);
    CU_SYM(cuDeviceGetName);
    CU_SYM(cuDeviceGetAttribute);
    CU_SYM(cuCtxCreate);
    CU_SYM(cuCtxCreate_v4);
    CU_SYM(cuModuleLoadData);
    CU_SYM(cuModuleLoadDataEx);
    CU_SYM(cuFuncGetAttribute);
    CU_SYM(cuModuleGetFunction);
    CU_SYM(cuMemAlloc);
    CU_SYM(cuMemFree);
    CU_SYM(cuMemcpyHtoD);
    CU_SYM(cuMemcpyDtoH);
    CU_SYM(cuMemcpy2D);
    CU_SYM(cuLaunchKernel);
    CU_SYM(cuFuncSetAttribute);
    CU_SYM(cuStreamSynchronize);
    CU_SYM(cuGetErrorString);
    CU_SYM(cuMemAllocHost);
    CU_SYM(cuMemFreeHost);
    CU_SYM(cuModuleUnload);
    CU_SYM(cuCtxDestroy);
    CU_SYM(cuStreamCreate);
    #undef CU_SYM
    if (p_cuInit == NULL || p_cuDeviceGetCount == NULL || p_cuDeviceGet == NULL ||
        p_cuCtxCreate == NULL || p_cuModuleLoadData == NULL ||
        p_cuModuleGetFunction == NULL || p_cuMemAlloc == NULL ||
        p_cuMemcpyHtoD == NULL || p_cuMemcpyDtoH == NULL ||
        p_cuLaunchKernel == NULL || p_cuStreamSynchronize == NULL) {
        dlclose(cu_lib);
        cu_lib = NULL;
        return 0;
    }
    cu_ready = 1;
    return 1;
}

static int briev_dev_cuda_available(void) {
    int verbose = g_verbose;
    if (!cu_resolve()) {
        if (verbose) fprintf(stderr, "[briev_accel/cuda] libcuda.so.1 unavailable\n");
        return 0;
    }
    if (p_cuInit(0) != CUDA_SUCCESS) {
        if (verbose) fprintf(stderr, "[briev_accel/cuda] cuInit failed\n");
        return 0;
    }
    int n = 0;
    if (p_cuDeviceGetCount(&n) != CUDA_SUCCESS || n <= 0) {
        if (verbose) fprintf(stderr, "[briev_accel/cuda] no CUDA device\n");
        return 0;
    }
    // Prefer a compute-capable (sm ≥ 6) device — all supported ones qualify,
    // but the check keeps the driver from picking a non-compute device.
    for (int i = 0; i < n; i++) {
        CUdevice d;
        if (p_cuDeviceGet(&d, i) != CUDA_SUCCESS) {
            continue;
        }
        int major = 0;
        if (p_cuDeviceGetAttribute(&major, CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR, d)
                == CUDA_SUCCESS && major >= 6) {
            return 1;
        }
    }
    return 0;
}

static int briev_dev_cuda_init(void) {
    int verbose = g_verbose;
    if (!cu_resolve()) {
        return 0;
    }
    int n = 0;
    if (p_cuDeviceGetCount(&n) != CUDA_SUCCESS) {
        return 0;
    }
    int chosen = -1;
    for (int i = 0; i < n; i++) {
        CUdevice d;
        if (p_cuDeviceGet(&d, i) != CUDA_SUCCESS) {
            continue;
        }
        int major = 0;
        if (p_cuDeviceGetAttribute(&major, CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR, d)
                == CUDA_SUCCESS && major >= 6) {
            chosen = i;
            break;
        }
    }
    if (chosen < 0 || p_cuDeviceGet(&cu_device, chosen) != CUDA_SUCCESS) {
        return 0;
    }
    if (p_cuCtxCreate_v4 == NULL && cu_lib != NULL) {
        p_cuCtxCreate_v4 = (cu_ctx_create_v4)dlsym(cu_lib, "cuCtxCreate_v4");
    }
    if (p_cuCtxCreate(&cu_ctx, CU_CTX_SCHED_AUTO, cu_device) != CUDA_SUCCESS) {
        if (verbose) fprintf(stderr, "[briev_accel/cuda] cuCtxCreate failed\n");
        return 0;
    }
    if (p_cuStreamCreate(&cu_stream, 0x01u /* CU_STREAM_NON_BLOCKING */) != CUDA_SUCCESS) {
        p_cuStreamCreate(&cu_stream, 0u);
    }
    if (p_cuDeviceGetName(cu_device_name, (int)sizeof(cu_device_name), cu_device) != CUDA_SUCCESS) {
        snprintf(cu_device_name, sizeof(cu_device_name), "cuda:%d", chosen);
    }
    return 1;
}

/// create_kernel receives the PTX blob (bytes). cuModuleLoadData JITs it for
/// the active device (driver-internal ptxas). The PTX entry point is named
/// `main` (the S2 emitter's convention); a single source runs on any sm.
static int briev_dev_cuda_create_kernel(const uint8_t* blob, size_t size,
                                        void** kernel_out) {
    int verbose = g_verbose;
    if (!cu_resolve() || blob == NULL || size == 0 || kernel_out == NULL) {
        return 0;
    }
    BrievCudaKernel* k = (BrievCudaKernel*)calloc(1, sizeof(BrievCudaKernel));
    if (k == NULL) {
        return 0;
    }
    // cuModuleLoadData wants a NUL-terminated string for PTX text. The
    // compiler emits the blob without a trailing NUL — copy it in.
    char* ptx = (char*)malloc(size + 1);
    if (ptx == NULL) {
        free(k);
        return 0;
    }
    memcpy(ptx, blob, size);
    ptx[size] = '\0';
    // 2026-09-10 (.maxnreg): the staged mw kernel emits `.maxnreg 128` —
    // at 138 natural regs only 1 CTA/SM fits; the JIT register cap
    // restores 2 (same-window A/B 2048^3: 15.7 vs 12.3 TFLOP/s, 0 spills,
    // correctness unchanged — re-tested clean on the fixed kernel; the
    // historical IMA verdict was contaminated by that era's OOB bugs).
    // cuModuleLoadData has no options parameter, so when the PTX carries
    // the directive route through cuModuleLoadDataEx with
    // CU_JIT_MAX_REGISTERS (=0).
    unsigned int max_reg = 0;
    const char* mreg = strstr(ptx, ".maxnreg ");
    if (verbose) fprintf(stderr, "[briev_accel/cuda] .maxnreg scan: %s\n", mreg ? "found" : "absent");
    if (mreg != NULL) {
        max_reg = (unsigned int)strtoul(mreg + 9, NULL, 10);
        // The driver-internal JIT rejects the directive itself — strip it
        // and carry the value via CU_JIT_MAX_REGISTERS instead.
        char* line_end = strchr(mreg, ';');
        if (line_end != NULL) {
            memset((char*)mreg, ' ', (size_t)(line_end - mreg) + 1);
        }
    }
    int rc;
    if (max_reg > 0 && p_cuModuleLoadDataEx != NULL) {
        unsigned int jit_opt = 0; /* CU_JIT_MAX_REGISTERS */
        void* jit_vals = &max_reg;
        rc = p_cuModuleLoadDataEx(&k->module, ptx, 1, &jit_opt, &jit_vals);
    } else {
        rc = p_cuModuleLoadData(&k->module, ptx);
    }
    free(ptx);
    if (rc != CUDA_SUCCESS) {
        if (verbose) {
            const char* estr = "?";
            if (p_cuGetErrorString) p_cuGetErrorString(rc, &estr);
            fprintf(stderr, "[briev_accel/cuda] cuModuleLoadData failed: %s (rc %d) — CPU fallback\n", estr, rc);
        }
        free(k);
        return 0;
    }
    if (p_cuModuleGetFunction(&k->func, k->module, "main") != CUDA_SUCCESS) {
        if (verbose) fprintf(stderr, "[briev_accel/cuda] PTX has no 'main' entry\n");
        p_cuModuleUnload(k->module);
        free(k);
        return 0;
    }
    if (verbose && p_cuFuncGetAttribute) {
        int regs = 0;
        if (p_cuFuncGetAttribute(&regs, 4 /* CU_FUNC_ATTRIBUTE_NUM_REGS */,
                                 k->func) == CUDA_SUCCESS) {
            fprintf(stderr, "[briev_accel/cuda] JIT regs=%d\n", regs);
        }
    }
    k->dev = 0;
    k->mapped_host = NULL;
    k->bytes = 0;
    k->block_threads = 64;
    k->shared_bytes = 0;
    *kernel_out = k;
    return 1;
}

// 2026-09-09 (S3b+ perf rungs): per-kernel block size override (default 64).
static int briev_dev_cuda_set_block_threads(void* handle, uint32_t n) {
    BrievCudaKernel* k = (BrievCudaKernel*)handle;
    if (k == NULL || n == 0 || n > 1024) return 0;
    k->block_threads = n;
    return 1;
}

// 2026-09-18 (P1 lane-coverage fix): per-kernel block-per-workitem flag.
// When set, launch_dev2d multiplies nx by block_threads so the driver
// launches `n` blocks of `block_threads` threads (lane-mapped reduction:
// w = ctaid.x, all threads compute redundantly for full d-coverage).
static int briev_dev_cuda_set_block_per_workitem(void* handle, uint32_t flag) {
    BrievCudaKernel* k = (BrievCudaKernel*)handle;
    if (k == NULL) return 0;
    k->block_per_workitem = flag;
    return 1;
}

// 2026-09-10 (cp.async stages): per-kernel dynamic shared-memory size.
static int briev_dev_cuda_set_shared_bytes(void* handle, uint32_t n) {
    BrievCudaKernel* k = (BrievCudaKernel*)handle;
    if (k == NULL || n > 99 * 1024u) return 0;
    k->shared_bytes = n;
    return 1;
}

// ── Launch core ────────────────────────────────────────────────────────────

// Block/grid geometry mirrors the Vulkan driver: block 64×1×1, grid
// (ceil(nx/64), ny, 1) — the kernel reconstructs i = y*nx + x from
// %ctaid.y/%ctaid.x/%tid.x, the same convention as the SPIR-V 2D dispatch.
// One param: the projection pointer (kernel `.param .b64 param0`).
static int cuda_launch_grid(BrievCudaKernel* k, size_t nx, size_t ny,
                            size_t shared_bytes, int sync) {
    unsigned bx = k->block_threads > 0 ? k->block_threads : cu_local_x;
    unsigned gx = (unsigned)((nx + bx - 1) / bx);
    unsigned gy = (unsigned)ny;
    if (gx == 0) gx = 1;
    if (gy == 0) gy = 1;
    if (shared_bytes > 48 * 1024u && p_cuFuncSetAttribute) {
        p_cuFuncSetAttribute(k->func, CU_FUNC_ATTRIBUTE_MAX_DYNAMIC_SHARED_SIZE_BYTES,
                             (int)shared_bytes);
    }
    CUdeviceptr dptr = k->dev;
    void* params[] = { &dptr };
    int rc = p_cuLaunchKernel(k->func, gx, gy, 1, bx, 1, 1,
                              (unsigned)shared_bytes, cu_stream, params, NULL);
    if (rc != CUDA_SUCCESS) {
        if (g_verbose) {
            const char* estr = "?";
            if (p_cuGetErrorString) p_cuGetErrorString(rc, &estr);
            fprintf(stderr, "[briev_accel/cuda] cuLaunchKernel failed: %s (rc %d)\n", estr, rc);
        }
        return 0;
    }
    rc = p_cuStreamSynchronize(cu_stream);
    if (!sync) {
        // 2026-09-14 (gpu_schedule Phase 2): async submit — the stream
        // orders the kernels; no host wait. The sync happens at download.
        return 1;
    }
    if (rc != CUDA_SUCCESS && g_verbose) {
        const char* estr = "?";
        if (p_cuGetErrorString) p_cuGetErrorString(rc, &estr);
        fprintf(stderr, "[briev_accel/cuda] cuStreamSynchronize failed: %s (rc %d)\n", estr, rc);
    }
    return rc == CUDA_SUCCESS;
}

// Full-copy launch: seeds the persistent page-locked host mirror + device
// working set on the first call (that is when the projection size becomes
// known — the same lazy-prime pattern as the Vulkan driver's `launch`),
// then upload, dispatch, download into `proj_out`. Afterwards `mapped()`
// returns the mirror and the resident path (launch_dev2d) takes over.
static int briev_dev_cuda_launch(void* handle, const void* proj, size_t proj_bytes,
                                 size_t global_n, void* proj_out) {
    int verbose = g_verbose;
    BrievCudaKernel* k = (BrievCudaKernel*)handle;
    if (!k || !k->func) {
        return 0;
    }
    if (k->mapped_host == NULL || k->bytes < proj_bytes) {
        if (g_verbose) fprintf(stderr, "[cuda] launch prime bytes=%zu (shared)\n", proj_bytes);
        // 2026-09-14 (gpu_schedule Phase 0): the program-level shared buffer
        // is allocated ONCE; every kernel aliases it. Re-prime (bigger
        // projection) is impossible — the layout is fixed per program.
        if (g_shared_host == NULL) {
            if (p_cuMemAllocHost(&g_shared_host, proj_bytes, 0) != CUDA_SUCCESS) {
                if (verbose) fprintf(stderr, "[briev_accel/cuda] host mirror alloc failed\n");
                return 0;
            }
            if (p_cuMemAlloc(&g_shared_dev, proj_bytes) != CUDA_SUCCESS) {
                if (verbose) fprintf(stderr, "[briev_accel/cuda] device alloc failed\n");
                p_cuMemFreeHost(g_shared_host);
                g_shared_host = NULL;
                return 0;
            }
            g_shared_bytes = proj_bytes;
        }
        k->mapped_host = g_shared_host;
        k->dev = g_shared_dev;
        k->bytes = g_shared_bytes;
    }
    memcpy(k->mapped_host, proj, proj_bytes);
    if (p_cuMemcpyHtoD(k->dev, k->mapped_host, proj_bytes) != CUDA_SUCCESS) {
        if (verbose) fprintf(stderr, "[briev_accel/cuda] HtoD failed\n");
        return 0;
    }
    int ok = cuda_launch_grid(k, global_n, 1, k->shared_bytes, 1);
    if (ok && proj_out) {
        ok = p_cuMemcpyDtoH(proj_out, k->dev, proj_bytes) == CUDA_SUCCESS;
    }
    return ok;
}

/// Mapped projection pointer — the page-locked host mirror of the device
/// working set (created at first launch_dev2d). NULL until then.
static void* briev_dev_cuda_mapped(void* handle) {
    BrievCudaKernel* k = (BrievCudaKernel*)handle;
    if (k && k->mapped_host) {
        return k->mapped_host;
    }
    // 2026-09-15 (gpu_schedule pipeline fix): with the program-level shared
    // working set allocated, an UNPRIMED kernel's handle maps onto it — so
    // the second kernel of a pipeline does NOT re-prime (re-priming packs its
    // touched fields from host state and full-HtoD's them, WIPING the first
    // kernel's device output before the consumer runs).
    return g_shared_host;
}

/// Flat 1D dispatch: nx work items, one row (the common launch_dev form).
static int briev_dev_cuda_launch_dev(void* handle, size_t global_n);

/// Device residency launch: the runtime seeds/syncs the mapped host mirror
/// itself (full_sync → full upload; else the dirty (offset, bytes) pairs),
/// then we dispatch. No host copies beyond the dirty sync. The device
/// working set is allocated once at the first launch_dev2d.
static int briev_dev_cuda_launch_dev2d(void* handle, size_t nx, size_t ny,
                                       int full_sync, const size_t* dirty,
                                       uint32_t n_dirty) {
    BrievCudaKernel* k = (BrievCudaKernel*)handle;
    if (!k || !k->func) {
        return 0;
    }
    // 2026-09-15 (gpu_schedule pipeline fix): an unprimed kernel in a
    // shared-buffer program attaches to the existing working set here —
    // its `mapped()` already returned g_shared_host, so the resident path
    // skipped the re-prime; `k->dev` must follow.
    if (k->mapped_host == NULL) {
        if (g_shared_host != NULL) {
            k->mapped_host = g_shared_host;
            k->dev = g_shared_dev;
            k->bytes = g_shared_bytes;
        } else {
            if (g_verbose) {
                fprintf(stderr, "[briev_accel/cuda] residency before size known — full-copy launch\n");
            }
            return 1;
        }
    }
    size_t bytes = k->bytes;
    if (g_verbose) fprintf(stderr, "[cuda] launch_dev2d bytes=%zu dev=%llx host=%p nx=%zu ny=%zu full=%d ndirty=%u\n", bytes, (unsigned long long)k->dev, (void*)k->mapped_host, nx, ny, full_sync, n_dirty);
    if (k->dev == 0 || k->mapped_host == NULL || bytes == 0) {
        return 0;
    }
    // 2026-09-19 (flash-decode gate forensics #4): n_dirty == 0 with
    // full_sync == 0 means NOTHING changed on the host — skip the copy
    // entirely. The old `full_sync || n_dirty == 0` read "no dirty
    // scalars" as "re-upload the whole projection", costing a full
    // PCIe round trip (21 MB ≈ 3.8 ms) on EVERY launch of every
    // scalar-free kernel. full_sync alone is the seed/everything signal.
    if (full_sync) {
        if (p_cuMemcpyHtoD(k->dev, k->mapped_host, bytes) != CUDA_SUCCESS) {
            return 0;
        }
    } else if (n_dirty > 0) {
        for (uint32_t r = 0; r < n_dirty; r++) {
            size_t off = dirty[2 * r];
            size_t sz = dirty[2 * r + 1];
            if (off + sz > bytes) {
                return 0;
            }
            if (p_cuMemcpyHtoD(k->dev + off, (char*)k->mapped_host + off, sz) != CUDA_SUCCESS) {
                return 0;
            }
        }
    }
    // 2026-09-18 (P1 lane-coverage fix): lane-mapped reduction kernels
    // treat each block as one work item (w = ctaid.x).  The dispatch
    // sends the WORK-ITEM count (nx = count); multiply by block_threads
    // so the driver's gx = ceil(nx*bx/bx) = nx blocks.
    if (k->block_per_workitem) {
        nx *= k->block_threads;
    }
    return cuda_launch_grid(k, nx, ny, k->shared_bytes, !g_async_launch);
}

/// Flat 1D dispatch: nx work items, one row (the common launch_dev form).
static int briev_dev_cuda_launch_dev(void* handle, size_t global_n) {
    return briev_dev_cuda_launch_dev2d(handle, global_n, 1, 0, NULL, 0);
}

static int briev_dev_cuda_launch_dev2d_batch(void* handle, size_t nx, size_t ny,
                                             uint32_t times, int full_sync,
                                             const size_t* dirty, uint32_t n_dirty) {
    BrievCudaKernel* k = (BrievCudaKernel*)handle;
    if (!k || !k->func) {
        return 0;
    }
    int ok = briev_dev_cuda_launch_dev2d(handle, nx, ny, full_sync, dirty, n_dirty);
    for (uint32_t t = 1; ok && t < times; t++) {
        // Subsequent dispatches reuse the working set — no sync.
        ok = cuda_launch_grid(k, nx, ny, k->shared_bytes, !g_async_launch);
    }
    return ok;
}

/// 2026-09-18 (M2 strided append, plan coalesced-kv-memory-path): pitched
/// host→device copies without a dispatch — one cuMemcpy2D (height 1) per
/// descriptor. The runtime already gathered the host mirror; these copies
/// read the CALLER'S staging buffer directly (absolute pointers), so the
/// mirror gather and the device copy see the same values. Same synchronous
/// contract as upload_ranges. Returns 0 if cuMemcpy2D is unavailable or
/// any copy would leave the working set.
static int briev_dev_cuda_push_strided(void* handle, const void* copies_opaque,
                                       uint32_t n) {
    (void)handle; (void)copies_opaque; (void)n;
    // 2026-09-18 (M2, BUGS.md "cuMemcpy2D silent no-op"): DISABLED on
    // CUDA 13.4 / driver 615.71.09 — cuMemcpy2D returns CUDA_SUCCESS but
    // writes nothing when the context was created by the legacy
    // cuCtxCreate entry (reproduced standalone, probe_repl.c); the v4
    // context creation fixes 2D copies but breaks cuLaunchKernel. The
    // strided-append use it would serve (d-major K) is not the M4
    // integration shape — the fused node's j-major K appends are
    // contiguous and ride push_ranges. Re-enable when the driver quirk
    // is understood or the context story settles.
    return 0;
}

/// 2026-09-18 (M4 decode-append): range upload without dispatch — the
/// launch_dev2d dirty loop minus the grid launch. Same synchronous-copy
/// contract (previous launch already drained; see cuda_launch_grid's sync
/// default). Returns 0 on any range that would leave the working set.
static int briev_dev_cuda_upload_ranges(void* handle, const size_t* dirty,
                                        uint32_t n_dirty) {
    BrievCudaKernel* k = (BrievCudaKernel*)handle;
    if (!k || k->dev == 0 || k->mapped_host == NULL || k->bytes == 0) {
        return 0;
    }
    if (p_cuStreamSynchronize && cu_stream) {
        p_cuStreamSynchronize(cu_stream);
    }
    for (uint32_t r = 0; r < n_dirty; r++) {
        size_t off = dirty[2 * r];
        size_t sz = dirty[2 * r + 1];
        if (off + sz > k->bytes) {
            return 0;
        }
        if (p_cuMemcpyHtoD(k->dev + off, (char*)k->mapped_host + off, sz)
            != CUDA_SUCCESS) {
            return 0;
        }
    }
    return 1;
}

/// Pull the device working set into the page-locked host mirror
/// (briev_accel_download's tail).
static int briev_dev_cuda_download_dev(void* handle) {
    BrievCudaKernel* k = (BrievCudaKernel*)handle;
    if (!k || k->mapped_host == NULL || k->dev == 0 || k->bytes == 0) {
        return 0;
    }
    // 2026-09-14 (gpu_schedule Phase 2): async launches submit without a
    // host wait — drain the stream before reading the results back.
    if (p_cuStreamSynchronize && cu_stream) {
        p_cuStreamSynchronize(cu_stream);
    }
    return p_cuMemcpyDtoH(k->mapped_host, k->dev, k->bytes) == CUDA_SUCCESS;
}

static void briev_dev_cuda_destroy_kernel(void* handle) {
    BrievCudaKernel* k = (BrievCudaKernel*)handle;
    if (!k) {
        return;
    }
    // 2026-09-14 (Phase 0): the buffer + mirror are program-shared — freed
    // once at shutdown, never per-kernel.
    if (k->module && p_cuModuleUnload) {
        p_cuModuleUnload(k->module);
    }
    free(k);
}

static void briev_dev_cuda_shutdown(void) {
    if (cu_stream && p_cuStreamSynchronize) {
        p_cuStreamSynchronize(cu_stream);
    }
    if (g_shared_host && p_cuMemFreeHost) {
        p_cuMemFreeHost(g_shared_host);
        g_shared_host = NULL;
    }
    if (g_shared_dev && p_cuMemFree) {
        p_cuMemFree(g_shared_dev);
        g_shared_dev = 0;
    }
    g_shared_bytes = 0;
    if (cu_ctx && p_cuCtxDestroy) {
        p_cuCtxDestroy(cu_ctx);
    }
    if (cu_lib) {
        dlclose(cu_lib);
        cu_lib = NULL;
    }
    cu_ready = 0;
    cu_ctx = NULL;
    cu_stream = NULL;
}

static const char* briev_dev_cuda_device_name(void) {
    return cu_device_name;
}

BrievDeviceDriver briev_dev_cuda = {
    "cuda",
    // 2026-09-14 (gpu_schedule Phase 0): BRIEV_DEV_CAP_SHARED_STATE — the
    // program-level shared device buffer enables cross-kernel array flow.
    BRIEV_DEV_CAP_SHARED_STATE,
    briev_dev_cuda_available,
    briev_dev_cuda_init,
    briev_dev_cuda_create_kernel,
    briev_dev_cuda_launch,
    briev_dev_cuda_destroy_kernel,
    briev_dev_cuda_shutdown,
    briev_dev_cuda_mapped,
    briev_dev_cuda_launch_dev,
    briev_dev_cuda_launch_dev2d,
    briev_dev_cuda_download_dev,
    briev_dev_cuda_launch_dev2d_batch,
    briev_dev_cuda_device_name,
    // Images: not in the CUDA tier's S1 scope — NULL refuses image kernels.
    NULL,
    NULL,
    // 2026-09-09 (S3b+ perf rungs): per-kernel block-size override.
    briev_dev_cuda_set_block_threads,
    // 2026-09-10 (cp.async stages): per-kernel dynamic shared-memory size.
    briev_dev_cuda_set_shared_bytes,
    // 2026-09-18 (M4 decode-append): array-range upload without dispatch.
    briev_dev_cuda_upload_ranges,
    // 2026-09-18 (M2 strided append): pitched copies without dispatch.
    briev_dev_cuda_push_strided,
    // 2026-09-18 (P1 lane-coverage fix): block-per-workitem dispatch flag.
    briev_dev_cuda_set_block_per_workitem,
};