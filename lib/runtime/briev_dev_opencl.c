// OpenCL device driver for briev_accel_rt — SPIR-V via OpenCL 3.0 IL
// (clCreateProgramWithIL). Ported from the legacy briev_gpu_rt.c OpenCL
// fallback and restructured to the single-flat-buffer model: one storage
// buffer holds the kernel's packed `%State` projection; the kernel signature
// is `kernel main(ptr %state, i64 %n)`.
//
// Loaded dynamically via dlopen("libOpenCL.so.1"); when absent, available()
// returns 0 and the fallback chain moves on.

#include "briev_accel_rt.h"

#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <dlfcn.h>
#include <stdio.h>

typedef void* cl_platform_id;
typedef void* cl_device_id;
typedef void* cl_context;
typedef void* cl_command_queue;
typedef void* cl_program;
typedef void* cl_kernel;
typedef void* cl_mem;
typedef void* cl_event;

static void* cl_lib = NULL;
static int cl_ready = 0;
static cl_platform_id cl_platform;
static cl_device_id cl_device;
static cl_context cl_ctx;
static cl_command_queue cl_queue;

static cl_platform_id* (*p_clGetPlatformIDs)(uint32_t, cl_platform_id*, uint32_t*) = NULL;
static int (*p_clGetDeviceIDs)(cl_platform_id, uint64_t, uint32_t, cl_device_id*, uint32_t*) = NULL;
static cl_context (*p_clCreateContext)(void*, uint32_t, cl_device_id*, void*, void*, int*) = NULL;
static cl_command_queue (*p_clCreateCommandQueue)(cl_context, cl_device_id, uint64_t, int*) = NULL;
static cl_program (*p_clCreateProgramWithIL)(cl_context, const void*, size_t, int*) = NULL;
static int (*p_clBuildProgram)(cl_program, uint32_t, const cl_device_id*, const char*, void*, void*) = NULL;
static int (*p_clGetProgramBuildInfo)(cl_program, cl_device_id, uint32_t, size_t, void*, size_t*) = NULL;
static int (*p_clGetEventProfilingInfo)(cl_event, uint32_t, size_t, void*, size_t*) = NULL;
static int (*p_clReleaseEvent)(cl_event) = NULL;
static int (*p_clGetProgramInfo)(cl_program, uint32_t, size_t, void*, size_t*) = NULL;
static cl_kernel (*p_clCreateKernel)(cl_program, const char*, int*) = NULL;
static int (*p_clSetKernelArg)(cl_kernel, uint32_t, size_t, const void*) = NULL;
static int (*p_clEnqueueNDRangeKernel)(cl_command_queue, cl_kernel, uint32_t, const size_t*, const size_t*, const size_t*, uint32_t, const cl_event*, cl_event*) = NULL;
static int (*p_clEnqueueReadBuffer)(cl_command_queue, cl_mem, int, size_t, size_t, void*, uint32_t, const cl_event*, cl_event*) = NULL;
static int (*p_clEnqueueWriteBuffer)(cl_command_queue, cl_mem, int, size_t, size_t, const void*, uint32_t, const cl_event*, cl_event*) = NULL;
static int (*p_clFinish)(cl_command_queue) = NULL;
static int (*p_clReleaseKernel)(cl_kernel) = NULL;
static int (*p_clReleaseProgram)(cl_program) = NULL;
static int (*p_clReleaseCommandQueue)(cl_command_queue) = NULL;
static int (*p_clReleaseContext)(cl_context) = NULL;
static int (*p_clReleaseMemObject)(cl_mem) = NULL;
static cl_mem (*p_clCreateBuffer)(cl_context, uint64_t, size_t, void*, int*) = NULL;

static int load_opencl_symbols(void) {
    cl_lib = dlopen("libOpenCL.so.1", RTLD_LAZY | RTLD_LOCAL);
    if (!cl_lib) {
        cl_lib = dlopen("libOpenCL.so", RTLD_LAZY | RTLD_LOCAL);
    }
    if (!cl_lib) {
        return 0;
    }
#define LOAD(sym) do { p_##sym = (void*)dlsym(cl_lib, #sym); if (!p_##sym) return 0; } while (0)
    LOAD(clGetPlatformIDs);
    LOAD(clGetDeviceIDs);
    LOAD(clCreateContext);
    LOAD(clCreateCommandQueue);
    LOAD(clCreateProgramWithIL);
    LOAD(clBuildProgram);
    LOAD(clGetProgramBuildInfo);
    LOAD(clGetProgramInfo);
    LOAD(clCreateKernel);
    LOAD(clSetKernelArg);
    LOAD(clEnqueueNDRangeKernel);
    LOAD(clEnqueueReadBuffer);
    LOAD(clEnqueueWriteBuffer);
    LOAD(clFinish);
    LOAD(clReleaseKernel);
    LOAD(clReleaseProgram);
    LOAD(clReleaseCommandQueue);
    LOAD(clReleaseContext);
    LOAD(clReleaseMemObject);
    LOAD(clCreateBuffer);
    LOAD(clGetEventProfilingInfo);
    LOAD(clReleaseEvent);
#undef LOAD
    return 1;
}

static int briev_dev_opencl_available(void) {
    if (cl_ready) {
        return 1;
    }
    if (!load_opencl_symbols()) {
        return 0;
    }
    uint32_t np = 0;
    if (p_clGetPlatformIDs(0, NULL, &np) != 0 || np == 0) {
        return 0;
    }
    p_clGetPlatformIDs(1, &cl_platform, &np);
    // 2026-11-06 (Intel G0): the old code COUNTED GPU devices but only
    // fetched a handle in the fallback branch. When the platform had a GPU
    // (NEO does), cl_device stayed NULL and clCreateContext failed with
    // CL_INVALID_DEVICE (-33). Always fetch the handle for the chosen type.
    uint32_t nd = 0;
    int dev_err = p_clGetDeviceIDs(cl_platform, 0x1 /* CL_DEVICE_TYPE_GPU */, 0, NULL, &nd);
    if (dev_err != 0 || nd == 0) {
        dev_err = p_clGetDeviceIDs(cl_platform, 0xFFFFFFFF /* all */, 1, &cl_device, &nd);
    } else {
        dev_err = p_clGetDeviceIDs(cl_platform, 0x1 /* CL_DEVICE_TYPE_GPU */, 1, &cl_device, &nd);
    }
    if (dev_err != 0 || nd == 0 || !cl_device) {
        return 0;
    }
    cl_ready = 1;
    return 1;
}

static int briev_dev_opencl_init(void) {
    if (!briev_dev_opencl_available()) {
        if (g_verbose) fprintf(stderr, "[briev_accel/opencl] no platform/device\n");
        return 0;
    }
    int err = 0;
    cl_ctx = p_clCreateContext(NULL, 1, &cl_device, NULL, NULL, &err);
    if (!cl_ctx || err != 0) {
        if (g_verbose) fprintf(stderr, "[briev_accel/opencl] clCreateContext failed err=%d\n", err);
        return 0;
    }
    // 2026-11-09 (lane ceilings): profiling queue for device-time reporting.
    cl_queue = p_clCreateCommandQueue(cl_ctx, cl_device, 0x2 /* CL_QUEUE_PROFILING_ENABLE */, &err);
    if (!cl_queue || err != 0) {
        if (g_verbose) fprintf(stderr, "[briev_accel/opencl] clCreateCommandQueue failed err=%d\n", err);
        return 0;
    }
    return 1;
}

typedef struct {
    cl_program program;
    cl_kernel kernel;
    // 2026-09-01: the module's OpExecutionMode LocalSize X (256 flat, 32
    // cooperative row kernels). The NDRange local size must match it.
    size_t local_x;
    // 2026-11-08 (Stage F): the host staging window the last launch read the
    // result into — `mapped` returns it so the runtime's field download path
    // works (OpenCL has no persistent host-visible projection).
    void* staging;
    // 2026-11-17 (S2 residency): the device buffer and its size, kept across
    // dispatches. Before this the copy path created, wrote, ran, read and
    // RELEASED a cl_mem on every launch — three driver round-trips of pure
    // setup per dispatch, which is most of the wall-vs-device gap.
    cl_mem dev_buf;
    size_t dev_buf_bytes;
    // The projection length the runtime hands the copy path (the only place
    // the size is known), and whether `dev_buf` already holds valid contents.
    size_t proj_bytes;
    int dev_seeded;
} BrievOpenClKernel;

static int briev_dev_opencl_create_kernel(const uint8_t* spirv, size_t size, void** out) {
    int err = 0;
    cl_program program = p_clCreateProgramWithIL(cl_ctx, spirv, size, &err);
    if (!program || err != 0) {
        if (g_verbose) fprintf(stderr, "[briev_accel/opencl] clCreateProgramWithIL failed err=%d\n", err);
        return 0;
    }
    err = p_clBuildProgram(program, 0, NULL, "", NULL, NULL);
    if (err != 0) {
        if (g_verbose) {
            size_t log_size = 0;
            if (p_clGetProgramBuildInfo &&
                p_clGetProgramBuildInfo(program, cl_device, 0x1183 /* CL_PROGRAM_BUILD_LOG */,
                                        sizeof(log_size), &log_size, NULL) == 0 &&
                log_size > 1) {
                char* log = (char*)calloc(1, log_size);
                if (log) {
                    p_clGetProgramBuildInfo(program, cl_device, 0x1183,
                                            log_size, log, NULL);
                    fprintf(stderr, "[briev_accel/opencl] build log: %s\n", log);
                    free(log);
                }
            }
            fprintf(stderr, "[briev_accel/opencl] clBuildProgram failed err=%d\n", err);
        }
        p_clReleaseProgram(program);
        return 0;
    }
    // 2026-11-08 (Intel G1.5 Stage F): the image may be Vulkan-flavored
    // ("main") or the OpenCL projection ("briev_kernel"). Ask the program
    // for its kernel names and prefer the OpenCL entry when present.
    char entry_name[64] = "main";
    if (p_clGetProgramInfo) {
        size_t names_size = 0;
        if (p_clGetProgramInfo(program, 0x1167 /* CL_PROGRAM_KERNEL_NAMES */,
                               sizeof(names_size), &names_size, NULL) == 0 &&
            names_size > 1) {
            char* names = (char*)calloc(1, names_size);
            if (names && p_clGetProgramInfo(program, 0x1167, names_size, names, NULL) == 0) {
                const char* pick = strstr(names, "briev_kernel");
                if (!pick) pick = strstr(names, "main");
                if (!pick) pick = names;
                size_t n = 0;
                while (pick[n] && pick[n] != ' ' && n < sizeof(entry_name) - 1) n++;
                memcpy(entry_name, pick, n);
                entry_name[n] = '\0';
            }
            free(names);
        }
    }
    // NEO's loader may not expose clGetProgramInfo; probe the two known
    // entry names directly (OpenCL projection first, Vulkan second).
    cl_kernel kernel = p_clCreateKernel(program, "briev_kernel", &err);
    if (!kernel || err != 0) {
        err = 0;
        kernel = p_clCreateKernel(program, "main", &err);
    }
    if (!kernel || err != 0) {
        err = 0;
        kernel = p_clCreateKernel(program, entry_name, &err);
    }
    if (!kernel || err != 0) {
        if (g_verbose) {
            size_t names_size = 0;
            if (p_clGetProgramInfo &&
                p_clGetProgramInfo(program, 0x1167 /* CL_PROGRAM_KERNEL_NAMES */,
                                   sizeof(names_size), &names_size, NULL) == 0 &&
                names_size > 1) {
                char* names = (char*)calloc(1, names_size);
                if (names) {
                    p_clGetProgramInfo(program, 0x1167, names_size, names, NULL);
                    fprintf(stderr, "[briev_accel/opencl] kernel names: %s\n", names);
                    free(names);
                }
            }
            fprintf(stderr, "[briev_accel/opencl] clCreateKernel(\"%s\") failed err=%d\n", entry_name, err);
        }
        p_clReleaseProgram(program);
        return 0;
    }
    BrievOpenClKernel* k = calloc(1, sizeof(BrievOpenClKernel));
    if (!k) {
        p_clReleaseKernel(kernel);
        p_clReleaseProgram(program);
        return 0;
    }
    k->program = program;
    k->kernel = kernel;
    k->staging = NULL;
    // Parse the module's OpExecutionMode LocalSize (SPIR-V: opcode 16, mode
    // literal 17, followed by W/H/D) — same parse as the Vulkan driver.
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

static int briev_dev_opencl_launch(void* handle, const void* proj, size_t proj_bytes,
                                  size_t global_n, void* proj_out) {
    BrievOpenClKernel* k = (BrievOpenClKernel*)handle;
    int err = 0;
    // Single READ_WRITE storage buffer holds the packed %State projection.
    if (!p_clCreateBuffer) {
        if (g_verbose) fprintf(stderr, "[briev_accel/opencl] clCreateBuffer not loaded\n");
        return 0;
    }
    cl_mem buf = p_clCreateBuffer(cl_ctx, 0x1 /* CL_MEM_READ_WRITE (1<<0) */, proj_bytes, NULL, &err);
    if (!buf || err != 0) {
        if (g_verbose) fprintf(stderr, "[briev_accel/opencl] create buffer failed err=%d size=%zu\n", err, proj_bytes);
        return 0;
    }
    err = p_clEnqueueWriteBuffer(cl_queue, buf, 0, 0, proj_bytes, proj, 0, NULL, NULL);
    if (err != 0 && g_verbose) {
        fprintf(stderr, "[briev_accel/opencl] write buffer failed err=%d\n", err);
    }
    // Kernel signature: kernel main(ptr %state, i64 %n).
    err = p_clSetKernelArg(k->kernel, 0, sizeof(cl_mem), &buf);
    if (err != 0 && g_verbose) {
        fprintf(stderr, "[briev_accel/opencl] set arg 0 failed err=%d\n", err);
    }
    int64_t n = (int64_t)global_n;
    err = p_clSetKernelArg(k->kernel, 1, sizeof(int64_t), &n);
    if (err != 0 && g_verbose) {
        fprintf(stderr, "[briev_accel/opencl] set arg 1 failed err=%d\n", err);
    }
    size_t global_size = global_n;
    size_t local_size = k->local_x ? k->local_x : 64;
    if (g_verbose) { fprintf(stderr, "[briev_accel/opencl] enqueue global=%zu local=%zu n=%llu\n", global_size, local_size, (unsigned long long)global_n); fflush(stderr); }
    cl_event ev = NULL;
    err = p_clEnqueueNDRangeKernel(cl_queue, k->kernel, 1, NULL, &global_size, &local_size, 0, NULL, &ev);
    if (g_verbose) { fprintf(stderr, "[briev_accel/opencl] enqueue res=%d\n", err); fflush(stderr); }
    if (err == 0 && ev && p_clGetEventProfilingInfo) {
        uint64_t t0 = 0, t1 = 0;
        p_clFinish(cl_queue);
        p_clGetEventProfilingInfo(ev, 0x1282 /* START */, sizeof(t0), &t0, NULL);
        p_clGetEventProfilingInfo(ev, 0x1283 /* END */, sizeof(t1), &t1, NULL);
        if (g_verbose) fprintf(stderr, "# gpu_time: %.3f ms\n", (double)(t1 - t0) / 1e6);
        if (p_clReleaseEvent) p_clReleaseEvent(ev);
        if (err != 0) { p_clReleaseMemObject(buf); return 0; }
        p_clEnqueueReadBuffer(cl_queue, buf, 0, 0, proj_bytes, proj_out, 0, NULL, NULL);
        p_clFinish(cl_queue);
        p_clReleaseMemObject(buf);
        k->staging = proj_out;
    k->proj_bytes = proj_bytes;
        return 1;
    }
    if (err != 0) {
        if (g_verbose) {
            fprintf(stderr, "[briev_accel/opencl] NDRange failed err=%d (global=%zu local=%zu)\n",
                    err, global_size, local_size);
        }
        p_clReleaseMemObject(buf);
        return 0;
    }
    if (g_verbose) { fprintf(stderr, "[briev_accel/opencl] finish...\n"); fflush(stderr); }
    p_clFinish(cl_queue);
    if (g_verbose) { fprintf(stderr, "[briev_accel/opencl] finish done\n"); fflush(stderr); }
    p_clEnqueueReadBuffer(cl_queue, buf, 0, 0, proj_bytes, proj_out, 0, NULL, NULL);
    p_clFinish(cl_queue);
    p_clReleaseMemObject(buf);
    // The staging window now holds the result; `mapped` exposes it.
    k->staging = proj_out;
    k->proj_bytes = proj_bytes;
    return 1;
}

static void* briev_dev_opencl_mapped(void* handle) {
    BrievOpenClKernel* k = (BrievOpenClKernel*)handle;
    return k ? k->staging : NULL;
}

static void briev_dev_opencl_destroy_kernel(void* handle) {
    BrievOpenClKernel* k = (BrievOpenClKernel*)handle;
    if (!k) {
        return;
    }
    if (k->dev_buf && p_clReleaseMemObject) {
        p_clReleaseMemObject(k->dev_buf);
        k->dev_buf = NULL;
    }
    p_clReleaseKernel(k->kernel);
    p_clReleaseProgram(k->program);
    free(k);
}

static void briev_dev_opencl_shutdown(void) {
    if (cl_queue) {
        p_clReleaseCommandQueue(cl_queue);
        cl_queue = NULL;
    }
    if (cl_ctx) {
        p_clReleaseContext(cl_ctx);
        cl_ctx = NULL;
    }
    if (cl_lib) {
        dlclose(cl_lib);
        cl_lib = NULL;
    }
    cl_ready = 0;
}

// 2026-11-17 (S2 residency) — NOT YET ON THE DISPATCH PATH.
// Measured on PTL: the runner's resident entry still takes the copy path, and
// the duplicate `briev_dev_opencl` driver table in libbriev_gpu_rt.a /
// libbriev_accel_rt.a is the likely cause (see
// benchmarks/results/2026-11-17-neo-residency-partial.md). The code below is
// correct against the ABI and is kept so the next session can finish the
// routing instead of rewriting it; it is inert until then.
//
// The resident path — no per-dispatch buffer churn. `mapped` exposes the host staging window, the runtime seeds the
// dirty scalars there, and the launch uploads ONLY full projections or the
// dirty (offset,length) pairs it is given, then reads the whole projection
// back for the runtime's field download.
//
// Still a copy: OpenCL has no device-mapped host pointer, so the honest
// ceiling here is "two copies per dispatch, zero allocations". Fine-grained
// SVM (`clSVMAlloc` + `clEnqueueSVMMemcpy`) is the follow-up that removes the
// copies too; it needs a device that reports CL_DEVICE_SVM_FINE_GRAIN_BUFFER,
// so it must be probed, not assumed.
static int briev_dev_opencl_resident_ready(BrievOpenClKernel* k, size_t proj_bytes) {
    if (!k || !p_clCreateBuffer) {
        return 0;
    }
    if (k->dev_buf && k->dev_buf_bytes == proj_bytes) {
        return 1;
    }
    if (k->dev_buf && p_clReleaseMemObject) {
        p_clReleaseMemObject(k->dev_buf);
        k->dev_buf = NULL;
        k->dev_buf_bytes = 0;
    }
    int err = 0;
    k->dev_buf = p_clCreateBuffer(cl_ctx, 0x1 /* CL_MEM_READ_WRITE */, proj_bytes, NULL, &err);
    if (!k->dev_buf || err != 0) {
        k->dev_buf = NULL;
        return 0;
    }
    k->dev_buf_bytes = proj_bytes;
    // First use of a new buffer: the device copy is undefined, so the next
    // launch must upload the whole projection regardless of the dirty list.
    k->dev_seeded = 0;
    return 1;
}

static int briev_dev_opencl_launch_dev2d(void* handle, size_t nx, size_t ny,
                                         int full_sync, const size_t* dirty,
                                         uint32_t n_dirty) {
    BrievOpenClKernel* k = (BrievOpenClKernel*)handle;
    if (!k || !k->staging) {
        return 0;
    }
    size_t proj_bytes = k->proj_bytes;
    if (proj_bytes == 0 || !briev_dev_opencl_resident_ready(k, proj_bytes)) {
        return 0;
    }
    int err = 0;
    if (full_sync || !k->dev_seeded) {
        k->dev_seeded = 1;
        err = p_clEnqueueWriteBuffer(cl_queue, k->dev_buf, 0, 0, proj_bytes, k->staging, 0, NULL, NULL);
        if (err != 0) {
            return 0;
        }
    } else {
        for (uint32_t i = 0; i < n_dirty; i++) {
            size_t off = dirty[2 * i];
            size_t len = dirty[2 * i + 1];
            if (off >= proj_bytes) {
                continue;
            }
            if (off + len > proj_bytes) {
                len = proj_bytes - off;
            }
            err = p_clEnqueueWriteBuffer(cl_queue, k->dev_buf, 0, off, len,
                                         (const char*)k->staging + off, 0, NULL, NULL);
            if (err != 0) {
                return 0;
            }
        }
    }
    err = p_clSetKernelArg(k->kernel, 0, sizeof(cl_mem), &k->dev_buf);
    if (err != 0) {
        return 0;
    }
    int64_t n = (int64_t)(nx * ny);
    err = p_clSetKernelArg(k->kernel, 1, sizeof(int64_t), &n);
    if (err != 0) {
        return 0;
    }
    size_t global_size = nx * ny;
    size_t local_size = k->local_x ? k->local_x : 64;
    if (g_verbose) {
        fprintf(stderr, "[briev_accel/opencl] resident dispatch global=%zu local=%zu dirty=%u full=%d\n",
                global_size, local_size, n_dirty, full_sync);
        fflush(stderr);
    }
    if (p_clEnqueueNDRangeKernel(cl_queue, k->kernel, 1, NULL, &global_size, &local_size, 0, NULL, NULL) != 0) {
        return 0;
    }
    if (p_clFinish(cl_queue) != 0) {
        return 0;
    }
    // The runtime's field download reads `mapped`, so land the results there.
    if (p_clEnqueueReadBuffer(cl_queue, k->dev_buf, 0, 0, proj_bytes, k->staging, 0, NULL, NULL) != 0) {
        return 0;
    }
    return p_clFinish(cl_queue) == 0;
}

static int briev_dev_opencl_launch_dev(void* handle, size_t global_n) {
    return briev_dev_opencl_launch_dev2d(handle, global_n, 1, 1, NULL, 0);
}

BrievDeviceDriver briev_dev_opencl = {
    // 2026-11-08 (Stage F): designated init so `mapped` (9th field) is set
    // while the optional residency tail stays zero.
    .name = "opencl",
    .capabilities = 0,  // no zero-copy (regular buffers)
    .available = briev_dev_opencl_available,
    .init = briev_dev_opencl_init,
    .create_kernel = briev_dev_opencl_create_kernel,
    .launch = briev_dev_opencl_launch,
    .destroy_kernel = briev_dev_opencl_destroy_kernel,
    .shutdown = briev_dev_opencl_shutdown,
    .mapped = briev_dev_opencl_mapped,
    // 2026-11-18 (R1) — DO NOT ADVERTISE `launch_dev`/`launch_dev2d` YET.
    // Measured on PTL with unconditional probes: once these entries are present the
    // OpenCL lane DOES enter `briev_accel_launch_resident_2d`, gets past the prime
    // (`mapped()` non-null) and then **dumps core** between `seed_or_sync_scalars`
    // and the driver dispatch — the `[R1] C launch_dev2d called` print never fires.
    // Until that is fixed the lane must stay on the copy path, which is correct:
    //   .launch_dev = briev_dev_opencl_launch_dev,
    //   .launch_dev2d = briev_dev_opencl_launch_dev2d,
    // (Earlier runs "proving" the copy path was taken were linking a stale archive
    // whose driver tail was zero — the entries were never actually reached.)
};
