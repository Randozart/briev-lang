// Minimal OpenCL surface for the Intel lane-ceiling harness.
// The PTL node ships libOpenCL.so.1 but no CL headers, so the harness carries
// its own typedefs and loads every entry point with dlopen (the same approach
// as lib/runtime/briev_dev_opencl.c). No link-time OpenCL dependency.

#ifndef BRIEV_CEILING_CL_API_H
#define BRIEV_CEILING_CL_API_H

#include <stddef.h>
#include <stdint.h>
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

#define CL_DEVICE_TYPE_GPU        0x1u
#define CL_MEM_READ_WRITE         0x1u
#define CL_MEM_COPY_HOST_PTR      0x20u
#define CL_QUEUE_PROFILING_ENABLE 0x2u
#define CL_PROFILING_COMMAND_START 0x1282u
#define CL_PROFILING_COMMAND_END   0x1283u

static void* g_cl_lib = NULL;

static cl_platform_id* (*p_clGetPlatformIDs)(uint32_t, cl_platform_id*, uint32_t*);
static int (*p_clGetDeviceIDs)(cl_platform_id, uint64_t, uint32_t, cl_device_id*, uint32_t*);
static cl_context (*p_clCreateContext)(const void*, uint32_t, const cl_device_id*, void*, void*, int*);
static cl_command_queue (*p_clCreateCommandQueue)(cl_context, cl_device_id, uint64_t, int*);
static cl_program (*p_clCreateProgramWithSource)(cl_context, uint32_t, const char**, const size_t*, int*);
static int (*p_clBuildProgram)(cl_program, uint32_t, const cl_device_id*, const char*, void*, void*);
static int (*p_clGetProgramBuildInfo)(cl_program, cl_device_id, uint32_t, size_t, void*, size_t*);
static cl_kernel (*p_clCreateKernel)(cl_program, const char*, int*);
static int (*p_clSetKernelArg)(cl_kernel, uint32_t, size_t, const void*);
static int (*p_clEnqueueNDRangeKernel)(cl_command_queue, cl_kernel, uint32_t,
                                       const size_t*, const size_t*, const size_t*,
                                       uint32_t, const cl_event*, cl_event*);
static int (*p_clEnqueueReadBuffer)(cl_command_queue, cl_mem, int, size_t, size_t,
                                    void*, uint32_t, const cl_event*, cl_event*);
static int (*p_clEnqueueWriteBuffer)(cl_command_queue, cl_mem, int, size_t, size_t,
                                     const void*, uint32_t, const cl_event*, cl_event*);
static int (*p_clFinish)(cl_command_queue);
static int (*p_clGetEventProfilingInfo)(cl_event, uint32_t, size_t, void*, size_t*);
static int (*p_clReleaseEvent)(cl_event);
static int (*p_clReleaseKernel)(cl_kernel);
static int (*p_clReleaseProgram)(cl_program);
static int (*p_clReleaseCommandQueue)(cl_command_queue);
static int (*p_clReleaseContext)(cl_context);
static int (*p_clReleaseMemObject)(cl_mem);
static cl_mem (*p_clCreateBuffer)(cl_context, uint64_t, size_t, void*, int*);

static int cl_load(void) {
    g_cl_lib = dlopen("libOpenCL.so.1", RTLD_LAZY | RTLD_LOCAL);
    if (!g_cl_lib) {
        g_cl_lib = dlopen("libOpenCL.so", RTLD_LAZY | RTLD_LOCAL);
    }
    if (!g_cl_lib) {
        return 0;
    }
#define CL_LOAD(sym) do { p_##sym = (void*)dlsym(g_cl_lib, #sym); if (!p_##sym) return 0; } while (0)
    CL_LOAD(clGetPlatformIDs);
    CL_LOAD(clGetDeviceIDs);
    CL_LOAD(clCreateContext);
    CL_LOAD(clCreateCommandQueue);
    CL_LOAD(clCreateProgramWithSource);
    CL_LOAD(clBuildProgram);
    CL_LOAD(clGetProgramBuildInfo);
    CL_LOAD(clCreateKernel);
    CL_LOAD(clSetKernelArg);
    CL_LOAD(clEnqueueNDRangeKernel);
    CL_LOAD(clEnqueueReadBuffer);
    CL_LOAD(clEnqueueWriteBuffer);
    CL_LOAD(clFinish);
    CL_LOAD(clGetEventProfilingInfo);
    CL_LOAD(clReleaseEvent);
    CL_LOAD(clReleaseKernel);
    CL_LOAD(clReleaseProgram);
    CL_LOAD(clReleaseCommandQueue);
    CL_LOAD(clReleaseContext);
    CL_LOAD(clReleaseMemObject);
    CL_LOAD(clCreateBuffer);
#undef CL_LOAD
    return 1;
}

#endif
