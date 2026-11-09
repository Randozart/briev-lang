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
typedef struct { void* data; } cl_event;

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
    cl_queue = p_clCreateCommandQueue(cl_ctx, cl_device, 0, &err);
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
    if (g_verbose) { fprintf(stderr, "[briev_accel/opencl] enqueue global=%zu local=%zu\n", global_size, local_size); fflush(stderr); }
    err = p_clEnqueueNDRangeKernel(cl_queue, k->kernel, 1, NULL, &global_size, &local_size, 0, NULL, NULL);
    if (g_verbose) { fprintf(stderr, "[briev_accel/opencl] enqueue res=%d\n", err); fflush(stderr); }
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
};
