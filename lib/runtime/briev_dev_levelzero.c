// Level Zero device driver for briev_accel_rt — SPIR-V IL via L0 compute.
//
// 2026-11-06 (G1 rung, docs/architecture/intel-dispatch.md §5): the Intel
// lane behind the driver table. L0 consumes the SAME SPIR-V blob as the
// Vulkan/OpenCL drivers through `zeModuleCreate(ZE_MODULE_FORMAT_IL_SPIRV)`
// — verified on PTL (harvest §3.1a: module + kernel creation succeed for a
// clang-emitted module; OpenVINO never exercises the IL path, Briev does).
//
// G1 simplifications (recorded; revisit at G2/G4):
//   - staging is HOST USM (`zeMemAllocHost`): on the PTL iGPU it is
//     device-accessible, so the kernel arg points straight at it and the
//     per-launch cost is two memcpys + one dispatch. A device-USM working
//     set (the D4 shape) is the G2/G4 upgrade, not a G1 requirement.
//   - sync is one immediate command list + `zeCommandListHostSynchronize`
//     (D5). NO counter-based events — disabled on Xe3/Xe3P by the vendor
//     stack (harvest §7.3).
//
// ABI notes: every enum/offset below was read from the node's
// /usr/include/level_zero/ze_api.h (L0 1.32, 2026-11-06): stype is a 4-byte
// enum at offset 0 with pNext at 8; DEVICE_PROPERTIES=0x3 carries
// type/vendorId/deviceId at 16/20/24; COMMAND_QUEUE_GROUP_PROPERTIES=0x6;
// CONTEXT_DESC=0xd; COMMAND_QUEUE_DESC=0xe; HOST_MEM_ALLOC_DESC=0x16;
// MODULE_DESC=0x1b (format@16, inputSize@24, pInputModule@32);
// KERNEL_DESC=0x1d (flags@16, pKernelName@24); ZE_DEVICE_TYPE_GPU=1;
// ZE_MODULE_FORMAT_IL_SPIRV=0; queue group COMPUTE flag = bit 0.
// Loaded via dlopen("libze_loader.so.1"); when absent, available() returns 0
// and the chain falls through (Vulkan → LevelZero → OpenCL → CPU).

#include "briev_accel_rt.h"

#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <dlfcn.h>

#define ZE_SUCCESS 0
#define ZE_TIMEOUT_MAX 0xFFFFFFFFFFFFFFFFull
#define ZE_INIT_FLAG_GPU_ONLY 1u
#define ZE_DEVICE_TYPE_GPU 1u
#define ZE_MODULE_FORMAT_IL_SPIRV 0u
#define ZE_QUEUE_MODE_ASYNCHRONOUS 2u
#define ZE_QUEUE_PRIORITY_NORMAL 0u
#define ZE_QUEUE_GROUP_FLAG_COMPUTE 1u
#define ZE_ST_DEVICE_PROPERTIES 0x3u
#define ZE_ST_COMMAND_QUEUE_GROUP_PROPERTIES 0x6u
#define ZE_ST_CONTEXT_DESC 0xdu
#define ZE_ST_COMMAND_QUEUE_DESC 0xeu
#define ZE_ST_HOST_MEM_ALLOC_DESC 0x16u
#define ZE_ST_MODULE_DESC 0x1bu
#define ZE_ST_KERNEL_DESC 0x1du

typedef void* lz_handle;

// Struct layouts mirror ze_api.h exactly (see ABI notes above). C's natural
// padding puts pNext at 8 after the 4-byte stype, and size_t members after
// a 4-byte pad — matching the header.
typedef struct { uint32_t stype; const void* pNext; uint32_t flags; } lz_context_desc;
typedef struct { uint32_t stype; const void* pNext; uint32_t ordinal; uint32_t index;
                 uint32_t flags; uint32_t mode; uint32_t priority; } lz_queue_desc;
typedef struct { uint32_t stype; const void* pNext; uint32_t format; size_t inputSize;
                 const uint8_t* pInputModule; const char* pBuildFlags; const void* pConstants; } lz_module_desc;
typedef struct { uint32_t stype; const void* pNext; uint32_t flags; const char* pKernelName; } lz_kernel_desc;
typedef struct { uint32_t stype; const void* pNext; uint32_t flags; } lz_host_mem_desc;
typedef struct { uint32_t stype; void* pNext; uint32_t flags; size_t maxFill;
                 uint32_t numQueues; } lz_queue_group_props;
typedef struct { uint32_t x; uint32_t y; uint32_t z; } lz_group_count;

// 2026-11-09 (lane ceilings): L0 event timing (device time, like Vulkan).
typedef struct { uint32_t stype; const void* pNext; uint32_t flags; uint32_t count; } lz_event_pool_desc;
typedef struct { uint32_t stype; const void* pNext; uint32_t index; uint32_t signal; uint32_t wait; } lz_event_desc;
typedef struct { uint64_t globalStart; uint64_t globalEnd; uint64_t contextStart; uint64_t contextEnd; } lz_kernel_ts;

static void* lz_lib = NULL;
static int lz_ready = 0;
// 2026-11-09 (lane ceilings): device timer resolution (ticks/sec) and valid
// bits for kernel timestamps (ze_device_properties_t offsets 80/92).
static uint64_t lz_timer_resolution = 0;
static uint32_t lz_timestamp_valid_bits = 0;
static lz_handle lz_driver;
static lz_handle lz_dev;
static lz_handle lz_ctx;
static lz_handle lz_cmd_list;
static uint32_t lz_queue_ordinal = 0;
static char lz_name[64] = "levelzero";

static int (*zeInit)(uint32_t) = NULL;
static int (*zeDriverGet)(uint32_t*, lz_handle*) = NULL;
static int (*zeDeviceGet)(lz_handle, uint32_t*, lz_handle*) = NULL;
static int (*zeDeviceGetProperties)(lz_handle, void*) = NULL;
static int (*zeDeviceGetCommandQueueGroupProperties)(lz_handle, uint32_t*, lz_queue_group_props*) = NULL;
static int (*zeContextCreate)(lz_handle, const lz_context_desc*, lz_handle*) = NULL;
static void (*zeContextDestroy)(lz_handle) = NULL;
static int (*zeCommandListCreateImmediate)(lz_handle, lz_handle, const lz_queue_desc*, lz_handle*) = NULL;
static void (*zeCommandListDestroy)(lz_handle) = NULL;
static int (*zeCommandListAppendLaunchKernel)(lz_handle, lz_handle, const lz_group_count*, void*, uint32_t, void**) = NULL;
static int (*zeCommandListHostSynchronize)(lz_handle, uint64_t) = NULL;
static int (*zeMemAllocHost)(lz_handle, const lz_host_mem_desc*, size_t, size_t, void**) = NULL;
static int (*zeMemFree)(lz_handle, void*) = NULL;
static int (*zeModuleCreate)(lz_handle, lz_handle, const lz_module_desc*, lz_handle*, void*) = NULL;
static void (*zeModuleDestroy)(lz_handle) = NULL;
static int (*zeModuleBuildLogGetString)(void*, size_t*, char*) = NULL;
static void (*zeModuleBuildLogDestroy)(void*) = NULL;
static int (*zeKernelCreate)(lz_handle, const lz_kernel_desc*, lz_handle*) = NULL;
static void (*zeKernelDestroy)(lz_handle) = NULL;
static int (*zeModuleGetKernelNames)(lz_handle, uint32_t*, const char**) = NULL;
static int (*zeKernelSetArgumentValue)(lz_handle, uint32_t, size_t, const void*) = NULL;
static int (*zeKernelSetGroupSize)(lz_handle, uint32_t, uint32_t, uint32_t) = NULL;
static int (*zeEventPoolCreate)(lz_handle, const lz_event_pool_desc*, uint32_t, const lz_handle*, lz_handle*) = NULL;
static int (*zeEventCreate)(lz_handle, const lz_event_desc*, lz_handle*) = NULL;
static int (*zeEventDestroy)(lz_handle) = NULL;
static int (*zeEventPoolDestroy)(lz_handle) = NULL;
static int (*zeEventHostSynchronize)(lz_handle, uint64_t) = NULL;
static int (*zeEventQueryKernelTimestamp)(lz_handle, lz_kernel_ts*) = NULL;

static int load_l0_symbols(void) {
    lz_lib = dlopen("libze_loader.so.1", RTLD_LAZY | RTLD_LOCAL);
    if (!lz_lib) {
        lz_lib = dlopen("libze_loader.so", RTLD_LAZY | RTLD_LOCAL);
    }
    if (!lz_lib) {
        return 0;
    }
#define LOAD(sym) do { p = dlsym(lz_lib, #sym); if (!p) return 0; memcpy(&sym, &p, sizeof(void*)); } while (0)
    void* p;
    LOAD(zeInit);
    LOAD(zeDriverGet);
    LOAD(zeDeviceGet);
    LOAD(zeDeviceGetProperties);
    LOAD(zeDeviceGetCommandQueueGroupProperties);
    LOAD(zeContextCreate);
    LOAD(zeContextDestroy);
    LOAD(zeCommandListCreateImmediate);
    LOAD(zeCommandListDestroy);
    LOAD(zeCommandListAppendLaunchKernel);
    LOAD(zeCommandListHostSynchronize);
    LOAD(zeMemAllocHost);
    LOAD(zeMemFree);
    LOAD(zeModuleCreate);
    LOAD(zeModuleDestroy);
    LOAD(zeModuleBuildLogGetString);
    LOAD(zeModuleBuildLogDestroy);
    LOAD(zeKernelCreate);
    LOAD(zeKernelDestroy);
    LOAD(zeModuleGetKernelNames);
    LOAD(zeKernelSetArgumentValue);
    LOAD(zeKernelSetGroupSize);
    LOAD(zeEventPoolCreate);
    LOAD(zeEventCreate);
    LOAD(zeEventDestroy);
    LOAD(zeEventPoolDestroy);
    LOAD(zeEventHostSynchronize);
    LOAD(zeEventQueryKernelTimestamp);
#undef LOAD
    return 1;
}

static int briev_dev_levelzero_available(void) {
    if (lz_ready) {
        return 1;
    }
    if (!load_l0_symbols()) {
        return 0;
    }
    int res = zeInit(ZE_INIT_FLAG_GPU_ONLY);
    if (res != ZE_SUCCESS) {
        if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] zeInit failed res=%d\n", res);
        return 0;
    }
    uint32_t nd = 0;
    if (zeDriverGet(&nd, NULL) != ZE_SUCCESS || nd == 0) {
        return 0;
    }
    if (nd > 8) { nd = 8; }
    lz_handle drv[8];
    zeDriverGet(&nd, drv);
    for (uint32_t di = 0; di < nd; di++) {
        uint32_t ndev = 0;
        if (zeDeviceGet(drv[di], &ndev, NULL) != ZE_SUCCESS || ndev == 0) {
            continue;
        }
        if (ndev > 8) { ndev = 8; }
        lz_handle devs[8];
        zeDeviceGet(drv[di], &ndev, devs);
        for (uint32_t ei = 0; ei < ndev; ei++) {
            // The REAL ze_device_properties_t is ~600 bytes (name[256] and
            // friends); the driver writes the full struct, so pass a full
            // buffer and read fields at their documented offsets
            // (type@16, vendorId@20, deviceId@24) — a 32-byte head struct
            // gets smashed past its end. Same pattern as the Vulkan
            // driver's 4096-byte props probe.
            unsigned char props[4096];
            memset(props, 0, sizeof(props));
            *(uint32_t*)props = ZE_ST_DEVICE_PROPERTIES;
            if (zeDeviceGetProperties(devs[ei], props) != ZE_SUCCESS) {
                continue;
            }
            uint64_t timer_res = 0;
            uint32_t valid_bits = 0;
            memcpy(&timer_res, props + 80, 8);
            memcpy(&valid_bits, props + 92, 4);
            uint32_t dtype, vendor_id, device_id;
            memcpy(&dtype, props + 16, 4);
            memcpy(&vendor_id, props + 20, 4);
            memcpy(&device_id, props + 24, 4);
            if (dtype != ZE_DEVICE_TYPE_GPU) {
                continue;
            }
            lz_driver = drv[di];
            lz_dev = devs[ei];
            lz_timer_resolution = timer_res;
            lz_timestamp_valid_bits = valid_bits;
            if (g_verbose) {
                fprintf(stderr, "[briev_accel/levelzero] timestamps: res=%llu bits=%u\n",
                        (unsigned long long)timer_res, valid_bits);
            }
            snprintf(lz_name, sizeof(lz_name), "levelzero:%x:%x",
                     vendor_id, device_id);
            // Compute queue group ordinal for the immediate list (fallback 0).
            lz_queue_ordinal = 0;
            uint32_t nq = 0;
            if (zeDeviceGetCommandQueueGroupProperties(lz_dev, &nq, NULL) == ZE_SUCCESS && nq > 0) {
                if (nq > 8) { nq = 8; }
                lz_queue_group_props qg[8];
                memset(qg, 0, sizeof(qg));
                for (uint32_t qi = 0; qi < nq; qi++) { qg[qi].stype = ZE_ST_COMMAND_QUEUE_GROUP_PROPERTIES; }
                if (zeDeviceGetCommandQueueGroupProperties(lz_dev, &nq, qg) == ZE_SUCCESS) {
                    for (uint32_t qi = 0; qi < nq; qi++) {
                        if (qg[qi].flags & ZE_QUEUE_GROUP_FLAG_COMPUTE) {
                            lz_queue_ordinal = qi;
                            break;
                        }
                    }
                }
            }
            lz_ready = 1;
            return 1;
        }
    }
    return 0;
}

static int briev_dev_levelzero_init(void) {
    if (!briev_dev_levelzero_available()) {
        if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] no loader/driver/GPU device\n");
        return 0;
    }
    int res;
    lz_context_desc cd = {0};
    cd.stype = ZE_ST_CONTEXT_DESC;
    cd.flags = 0;
    res = zeContextCreate(lz_driver, &cd, &lz_ctx);
    if (res != ZE_SUCCESS) {
        if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] zeContextCreate failed res=%d\n", res);
        return 0;
    }
    lz_queue_desc qd = {0};
    qd.stype = ZE_ST_COMMAND_QUEUE_DESC;
    qd.ordinal = lz_queue_ordinal;
    qd.index = 0;
    qd.flags = 0;  // NOT in-order: the G1 dispatches are independent; D5
                   // records in-order/immediate-lists as the G2+ decision.
    qd.mode = ZE_QUEUE_MODE_ASYNCHRONOUS;
    qd.priority = ZE_QUEUE_PRIORITY_NORMAL;
    res = zeCommandListCreateImmediate(lz_ctx, lz_dev, &qd, &lz_cmd_list);
    if (res != ZE_SUCCESS) {
        if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] zeCommandListCreateImmediate failed res=%d\n", res);
        return 0;
    }
    if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] %s ready (queue ordinal %u)\n",
                           lz_name, lz_queue_ordinal);
    return 1;
}

typedef struct {
    lz_handle module;
    lz_handle kernel;
    size_t bytes;      // staging capacity
    void* staging;     // host-USM staging (device-accessible on iGPU)
    uint32_t local_x;
} BrievLzKernel;

static int briev_dev_levelzero_create_kernel(const uint8_t* spirv, size_t size, void** out) {
    int res;
    lz_module_desc md = {0};
    md.stype = ZE_ST_MODULE_DESC;
    md.format = ZE_MODULE_FORMAT_IL_SPIRV;
    md.inputSize = size;
    md.pInputModule = spirv;
    md.pBuildFlags = "";
    md.pConstants = NULL;
    void* log = NULL;
    lz_handle module = NULL;
    res = zeModuleCreate(lz_ctx, lz_dev, &md, &module, &log);
    if (res != ZE_SUCCESS) {
        if (g_verbose) {
            if (log) {
                size_t log_size = 0;
                if (zeModuleBuildLogGetString(log, &log_size, NULL) == ZE_SUCCESS && log_size > 1) {
                    char* lb = (char*)calloc(1, log_size);
                    if (lb) {
                        zeModuleBuildLogGetString(log, &log_size, lb);
                        fprintf(stderr, "[briev_accel/levelzero] build log: %s\n", lb);
                        free(lb);
                    }
                }
                zeModuleBuildLogDestroy(log);
            }
            fprintf(stderr, "[briev_accel/levelzero] zeModuleCreate(IL_SPIRV) failed res=%d\n", res);
        }
        return 0;
    }
    if (log) {
        zeModuleBuildLogDestroy(log);
    }
    lz_kernel_desc kd = {0};
    kd.stype = ZE_ST_KERNEL_DESC;
    kd.flags = 0;
    // 2026-11-08 (Intel G1.5 Stage F): the image may be Vulkan-flavored
    // ("main") or the OpenCL projection ("briev_kernel"). Ask the module
    // instead of assuming, preferring the OpenCL entry when present.
    char entry_name[64] = "main";
    {
        uint32_t nn = 0;
        if (zeModuleGetKernelNames(module, &nn, NULL) == ZE_SUCCESS && nn > 0) {
            const char** names = (const char**)calloc(nn, sizeof(char*));
            if (names && zeModuleGetKernelNames(module, &nn, names) == ZE_SUCCESS) {
                const char* pick = NULL;
                for (uint32_t ni = 0; ni < nn; ni++) {
                    if (names[ni] && strcmp(names[ni], "briev_kernel") == 0) { pick = names[ni]; break; }
                }
                for (uint32_t ni = 0; !pick && ni < nn; ni++) {
                    if (names[ni] && strcmp(names[ni], "main") == 0) { pick = names[ni]; break; }
                }
                if (!pick && names[0]) pick = names[0];
                if (pick) {
                    strncpy(entry_name, pick, sizeof(entry_name) - 1);
                    entry_name[sizeof(entry_name) - 1] = '\0';
                }
            }
            free(names);
        }
    }
    kd.pKernelName = entry_name;
    lz_handle kernel = NULL;
    res = zeKernelCreate(module, &kd, &kernel);
    if (res != ZE_SUCCESS) {
        if (g_verbose) {
            // What kernels does the compiled module actually expose?
            uint32_t nn = 0;
            if (zeModuleGetKernelNames(module, &nn, NULL) == ZE_SUCCESS && nn > 0) {
                const char** names = (const char**)calloc(nn, sizeof(char*));
                if (names) {
                    zeModuleGetKernelNames(module, &nn, names);
                    fprintf(stderr, "[briev_accel/levelzero] module kernels (%u):", nn);
                    for (uint32_t ni = 0; ni < nn; ni++) {
                        fprintf(stderr, " %s", names[ni] ? names[ni] : "(null)");
                    }
                    fprintf(stderr, "\n");
                    free(names);
                }
            } else {
                fprintf(stderr, "[briev_accel/levelzero] module exposes no kernels\n");
            }
            fprintf(stderr, "[briev_accel/levelzero] zeKernelCreate(\"%s\") failed res=%d (0x%x)\n", entry_name, res, res);
        }
        zeModuleDestroy(module);
        return 0;
    }
    BrievLzKernel* k = calloc(1, sizeof(BrievLzKernel));
    if (!k) {
        zeKernelDestroy(kernel);
        zeModuleDestroy(module);
        return 0;
    }
    k->module = module;
    k->kernel = kernel;
    k->bytes = 0;
    k->staging = NULL;
    // Parse the module's OpExecutionMode LocalSize (SPIR-V: opcode 16, mode
    // literal 17, followed by W/H/D) — same parse as the OpenCL driver.
    k->local_x = 256;
    {
        const uint32_t* w = (const uint32_t*)spirv;
        size_t nw = size / 4;
        if (nw > 5 && w[0] == 0x07230203u) {
            size_t i = 5;
            while (i < nw) {
                uint32_t wc = w[i] >> 16, op = w[i] & 0xFFFFu;
                if (wc == 0) { break; }
                if (op == 16 && wc >= 4 && i + wc <= nw && w[i + 2] == 17) {
                    k->local_x = w[i + 3];
                    break;
                }
                i += wc;
            }
        }
    }
    *out = k;
    return 1;
}

static int briev_dev_levelzero_launch(void* handle, const void* proj, size_t proj_bytes,
                                      size_t global_n, void* proj_out) {
    BrievLzKernel* k = (BrievLzKernel*)handle;
    // Grow the host-USM staging to the projection extent (first launch —
    // projections are fixed per program once the bound is known).
    if (!k->staging || k->bytes < proj_bytes) {
        if (k->staging) {
            zeMemFree(lz_ctx, k->staging);
            k->staging = NULL;
        }
        lz_host_mem_desc hd = {0};
        hd.stype = ZE_ST_HOST_MEM_ALLOC_DESC;
        hd.flags = 0;
        int res = zeMemAllocHost(lz_ctx, &hd, proj_bytes, 64, &k->staging);
        if (res != ZE_SUCCESS) {
            if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] zeMemAllocHost failed res=%d bytes=%zu\n",
                                   res, proj_bytes);
            k->staging = NULL;
            return 0;
        }
        k->bytes = proj_bytes;
        if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] staging %zu bytes (host USM)\n", proj_bytes);
    }
    memcpy(k->staging, proj, proj_bytes);
    // Kernel signature: main(ptr %state, i64 %n) — arg values are copied by
    // zeKernelSetArgumentValue at set time.
    void* sp = k->staging;
    int res = zeKernelSetArgumentValue(k->kernel, 0, sizeof(void*), &sp);
    if (res == ZE_SUCCESS) {
        uint64_t n = (uint64_t)global_n;
        res = zeKernelSetArgumentValue(k->kernel, 1, sizeof(uint64_t), &n);
    }
    if (res != ZE_SUCCESS) {
        if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] set kernel args failed res=%d\n", res);
        return 0;
    }
    if (global_n == 0) {
        memcpy(proj_out, k->staging, proj_bytes);
        return 1;
    }
    uint32_t local = k->local_x ? k->local_x : 256;
    zeKernelSetGroupSize(k->kernel, local, 1, 1);
    size_t groups = (global_n + local - 1) / local;
    if (groups > 0xFFFFFFFFu) { groups = 0xFFFFFFFFu; }
    lz_group_count gc = { (uint32_t)groups, 1, 1 };
    // 2026-11-09 (lane ceilings): timestamp event for device-time reporting.
    lz_event_pool_desc pd = { 0x10 /* EVENT_POOL_DESC */, NULL, 4 /* KERNEL_TIMESTAMP */, 1 };
    lz_event_desc ed = { 0x11 /* EVENT_DESC */, NULL, 0, 4 /* HOST scope */, 0 };
    lz_handle pool = NULL, event = NULL;
    int ev_res = zeEventPoolCreate(lz_ctx, &pd, 0, NULL, &pool);
    if (ev_res != ZE_SUCCESS) {
        if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] event pool failed res=%d\n", ev_res);
    }
    int have_ev = 0;
    if (ev_res == ZE_SUCCESS) {
        ev_res = zeEventCreate(pool, &ed, &event);
        if (ev_res != ZE_SUCCESS) {
            if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] event create failed res=%d\n", ev_res);
        } else {
            have_ev = 1;
        }
    }
    res = zeCommandListAppendLaunchKernel(lz_cmd_list, k->kernel, &gc,
                                          have_ev ? event : NULL, 0, NULL);
    if (res != ZE_SUCCESS) {
        if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] append launch failed res=%d groups=%zu\n",
                               res, groups);
        return 0;
    }
    res = zeCommandListHostSynchronize(lz_cmd_list, ZE_TIMEOUT_MAX);
    if (res != ZE_SUCCESS) {
        if (g_verbose) fprintf(stderr, "[briev_accel/levelzero] host sync failed res=%d\n", res);
        return 0;
    }
    if (have_ev) {
        zeEventHostSynchronize(event, ZE_TIMEOUT_MAX);
        lz_kernel_ts ts = {0};
        int qres = zeEventQueryKernelTimestamp(event, &ts);
        if (qres != ZE_SUCCESS && g_verbose) {
            fprintf(stderr, "[briev_accel/levelzero] timestamp query failed res=%d\n", qres);
        }
        if (qres == ZE_SUCCESS && g_verbose) {
            uint64_t mask = lz_timestamp_valid_bits >= 64
                ? ~(uint64_t)0
                : ((uint64_t)1 << lz_timestamp_valid_bits) - 1;
            uint64_t ticks = (ts.globalEnd - ts.globalStart) & mask;
            // NEO reports timerResolution as NANOSECONDS PER TICK (52 on
            // PTL), not cycles/sec: ticks * resolution = ns. Keep the
            // spec-shaped cycles/sec branch for drivers that report a large
            // frequency.
            double ms = 0.0;
            if (lz_timer_resolution > 0 && lz_timer_resolution < 1000000) {
                ms = (double)ticks * (double)lz_timer_resolution / 1e6;
            } else if (lz_timer_resolution > 0) {
                ms = (double)ticks * 1000.0 / (double)lz_timer_resolution;
            }
            fprintf(stderr, "# gpu_time: %.3f ms\n", ms);
        }
        zeEventDestroy(event);
        zeEventPoolDestroy(pool);
    }
    memcpy(proj_out, k->staging, proj_bytes);
    return 1;
}

static void briev_dev_levelzero_destroy_kernel(void* handle) {
    BrievLzKernel* k = (BrievLzKernel*)handle;
    if (!k) {
        return;
    }
    if (k->staging) {
        zeMemFree(lz_ctx, k->staging);
    }
    zeKernelDestroy(k->kernel);
    zeModuleDestroy(k->module);
    free(k);
}

static void briev_dev_levelzero_shutdown(void) {
    if (lz_cmd_list) {
        zeCommandListDestroy(lz_cmd_list);
        lz_cmd_list = NULL;
    }
    if (lz_ctx) {
        zeContextDestroy(lz_ctx);
        lz_ctx = NULL;
    }
    if (lz_lib) {
        dlclose(lz_lib);
        lz_lib = NULL;
    }
    lz_ready = 0;
}

BrievDeviceDriver briev_dev_levelzero = {
    "levelzero",
    0,  // capabilities: no zero-copy (staging memcpy) until device USM lands
    briev_dev_levelzero_available,
    briev_dev_levelzero_init,
    briev_dev_levelzero_create_kernel,
    briev_dev_levelzero_launch,
    briev_dev_levelzero_destroy_kernel,
    briev_dev_levelzero_shutdown,
};
