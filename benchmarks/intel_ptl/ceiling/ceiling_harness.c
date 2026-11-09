// Intel lane ceiling harness — OpenCL C references (f32 + f16 storage).
//
// Measures DEVICE time (profiling events) for hand-written references, so the
// matrix compares the LANE's achievable ceiling, not Briev-generated kernels.
// The same harness structure is mirrored by vk_harness.c for Vulkan.
//
// Usage: ceiling_harness <copy|elem|reduce|gemm> <f32|f16> <M> <N> <K> <iters>
//   copy   : y[i] = x[i]                     (bandwidth probe, 2*bytes)
//   elem   : y[i] = a[i] + b[i]              (bandwidth probe, 3*bytes)
//   reduce : out[r] = sum_k x[r*K + k]       (rows M, K=K; workgroup per row)
//   gemm   : C[MxN] = A[MxK] * B[KxN]        (64x64 block, 4x4 micro-tile)
//
// Values are drawn from {0.25, 0.5, 1.0, 2.0} so f16 storage is exact and the
// double-precision CPU reference validates the device result directly.

#include "cl_api.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define LOCAL 256
#define WARMUP 3

static const char* KERNEL_SRC =
"#ifdef USE_HALF\n"
"#pragma OPENCL EXTENSION cl_khr_fp16 : enable\n"
"typedef half real_t;\n"
"#define TO_REAL(x) ((half)(x))\n"
"#else\n"
"typedef float real_t;\n"
"#define TO_REAL(x) (x)\n"
"#endif\n"
"__kernel void copy_k(__global const real_t* x, __global real_t* y, long n) {\n"
"  long i = get_global_id(0);\n"
"  if (i < n) y[i] = x[i];\n"
"}\n"
"__kernel void elem_k(__global const real_t* a, __global const real_t* b,\n"
"                     __global real_t* y, long n) {\n"
"  long i = get_global_id(0);\n"
"  if (i < n) y[i] = a[i] + b[i];\n"
"}\n"
"__kernel void reduce_k(__global const real_t* x, __global float* out, long K) {\n"
"  __local float scratch[256];\n"
"  long row = get_group_id(0);\n"
"  long lid = get_local_id(0);\n"
"  float acc = 0.0f;\n"
"  for (long k = lid; k < K; k += 256) acc += (float)x[row * K + k];\n"
"  scratch[lid] = acc;\n"
"  barrier(CLK_LOCAL_MEM_FENCE);\n"
"  for (int s = 128; s > 0; s >>= 1) {\n"
"    if (lid < s) scratch[lid] += scratch[lid + s];\n"
"    barrier(CLK_LOCAL_MEM_FENCE);\n"
"  }\n"
"  if (lid == 0) out[row] = scratch[0];\n"
"}\n"
"#define TS 64\n"
"__kernel void gemm_k(__global const real_t* A, __global const real_t* B,\n"
"                     __global real_t* C, long M, long N, long K) {\n"
"  __local float As[TS][TS + 1];\n"
"  __local float Bs[TS][TS + 1];\n"
"  long gx = get_group_id(0) * TS;\n"
"  long gy = get_group_id(1) * TS;\n"
"  long tx = get_local_id(0);\n"
"  long ty = get_local_id(1);\n"
"  float acc[4][4] = {{0.0f,0,0,0},{0.0f,0,0,0},{0.0f,0,0,0},{0.0f,0,0,0}};\n"
"  for (long k0 = 0; k0 < K; k0 += TS) {\n"
"    for (int i = 0; i < 4; i++) {\n"
"      for (int j = 0; j < 4; j++) {\n"
"        long ar = gy + ty * 4 + i, ac = k0 + tx * 4 + j;\n"
"        long br = k0 + ty * 4 + i, bc = gx + tx * 4 + j;\n"
"        As[ty * 4 + i][tx * 4 + j] =\n"
"            (ar < M && ac < K) ? (float)A[ar * K + ac] : 0.0f;\n"
"        Bs[ty * 4 + i][tx * 4 + j] =\n"
"            (br < K && bc < N) ? (float)B[br * N + bc] : 0.0f;\n"
"      }\n"
"    }\n"
"    barrier(CLK_LOCAL_MEM_FENCE);\n"
"    for (int kk = 0; kk < TS; kk++) {\n"
"      float ar[4], br[4];\n"
"      for (int i = 0; i < 4; i++) ar[i] = As[ty * 4 + i][kk];\n"
"      for (int j = 0; j < 4; j++) br[j] = Bs[kk][tx * 4 + j];\n"
"      for (int i = 0; i < 4; i++)\n"
"        for (int j = 0; j < 4; j++) acc[i][j] += ar[i] * br[j];\n"
"    }\n"
"    barrier(CLK_LOCAL_MEM_FENCE);\n"
"  }\n"
"  for (int i = 0; i < 4; i++) {\n"
"    long r = gy + ty * 4 + i;\n"
"    if (r >= M) continue;\n"
"    for (int j = 0; j < 4; j++) {\n"
"      long c = gx + tx * 4 + j;\n"
"      if (c < N) C[r * N + c] = TO_REAL(acc[i][j]);\n"
"    }\n"
"  }\n"
"}\n";

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e3 + (double)ts.tv_nsec / 1e6;
}

static int cmp_double(const void* a, const void* b) {
    double x = *(const double*)a, y = *(const double*)b;
    return (x > y) - (x < y);
}

// Exact-representable half bits for a small value set (no host f16 support
// assumed): 0.25=0x3400, 0.5=0x3800, 1.0=0x3C00, 2.0=0x4000, 3.0=0x4200.
static uint16_t half_of_idx(long i) {
    static const uint16_t v[] = {0x3400, 0x3800, 0x3C00, 0x4000, 0x4200};
    return v[i % 5];
}
static float half_to_float(uint16_t h) {
    uint32_t sign = (uint32_t)(h >> 15) & 1u;
    uint32_t exp = (uint32_t)(h >> 10) & 0x1fu;
    uint32_t man = (uint32_t)h & 0x3ffu;
    uint32_t bits;
    if (exp == 0) {
        if (man == 0) {
            bits = sign << 31;
        } else {
            int e = -1;
            do { e++; man <<= 1; } while ((man & 0x400u) == 0);
            man &= 0x3ffu;
            bits = (sign << 31) | ((uint32_t)(127 - 15 - e) << 23) | (man << 13);
        }
    } else if (exp == 0x1fu) {
        bits = (sign << 31) | 0x7f800000u | (man << 13);
    } else {
        bits = (sign << 31) | ((exp + 112u) << 23) | (man << 13);
    }
    float f;
    memcpy(&f, &bits, sizeof(f));
    return f;
}

static double val_of_idx(long i) {
    static const double v[] = {0.25, 0.5, 1.0, 2.0, 3.0};
    return v[i % 5];
}

typedef struct {
    cl_platform_id platform;
    cl_device_id device;
    cl_context ctx;
    cl_command_queue queue;
} ClEnv;

static int cl_env_init(ClEnv* e) {
    uint32_t np = 0, nd = 0;
    int err = 0;
    if (!cl_load()) { fprintf(stderr, "no libOpenCL\n"); return 0; }
    if (p_clGetPlatformIDs(1, &e->platform, &np) != 0 || np == 0) {
        fprintf(stderr, "no platform\n"); return 0;
    }
    if (p_clGetDeviceIDs(e->platform, CL_DEVICE_TYPE_GPU, 1, &e->device, &nd) != 0
        || nd == 0) {
        fprintf(stderr, "no GPU device\n"); return 0;
    }
    e->ctx = p_clCreateContext(NULL, 1, &e->device, NULL, NULL, &err);
    if (!e->ctx || err != 0) { fprintf(stderr, "ctx err=%d\n", err); return 0; }
    e->queue = p_clCreateCommandQueue(e->ctx, e->device,
                                      CL_QUEUE_PROFILING_ENABLE, &err);
    if (!e->queue || err != 0) { fprintf(stderr, "queue err=%d\n", err); return 0; }
    return 1;
}

static cl_program build_program(ClEnv* e, int half) {
    int err = 0;
    const char* opts = half ? "-DUSE_HALF -cl-fast-relaxed-math" : "-cl-fast-relaxed-math";
    cl_program p = p_clCreateProgramWithSource(e->ctx, 1, &KERNEL_SRC, NULL, &err);
    if (!p || err != 0) { fprintf(stderr, "program err=%d\n", err); return NULL; }
    err = p_clBuildProgram(p, 1, &e->device, opts, NULL, NULL);
    if (err != 0) {
        size_t n = 0;
        p_clGetProgramBuildInfo(p, e->device, 0x1183 /* CL_PROGRAM_BUILD_LOG */,
                                sizeof(n), NULL, &n);
        char* log = calloc(1, n + 1);
        if (log) {
            p_clGetProgramBuildInfo(p, e->device, 0x1183, n, log, NULL);
            fprintf(stderr, "build failed: %s\n", log);
            free(log);
        }
        return NULL;
    }
    return p;
}

typedef struct {
    cl_kernel k;
    cl_mem buf[4];
    int nbuf;
} Run;

static cl_mem make_buf(ClEnv* e, size_t bytes, void* host, int copy) {
    int err = 0;
    cl_mem m = p_clCreateBuffer(e->ctx, CL_MEM_READ_WRITE | (copy ? CL_MEM_COPY_HOST_PTR : 0),
                                bytes, copy ? host : NULL, &err);
    if (!m || err != 0) { fprintf(stderr, "buffer err=%d bytes=%zu\n", err, bytes); return NULL; }
    return m;
}

// One timed dispatch: returns DEVICE ms from profiling events, or -1.
static double time_kernel(ClEnv* e, cl_kernel k, size_t gx, size_t gy,
                          size_t lx, size_t ly) {
    cl_event ev = NULL;
    size_t g[2] = {gx, gy};
    size_t l[2] = {lx, ly};
    int err = p_clEnqueueNDRangeKernel(e->queue, k, gy > 1 ? 2 : 1, NULL,
                                       g, l, 0, NULL, &ev);
    if (err != 0) { fprintf(stderr, "ndrange err=%d\n", err); return -1; }
    p_clFinish(e->queue);
    uint64_t t0 = 0, t1 = 0;
    p_clGetEventProfilingInfo(ev, CL_PROFILING_COMMAND_START, sizeof(t0), &t0, NULL);
    p_clGetEventProfilingInfo(ev, CL_PROFILING_COMMAND_END, sizeof(t1), &t1, NULL);
    p_clReleaseEvent(ev);
    return (double)(t1 - t0) / 1e6;
}

static int report(const char* mode, const char* dtype, double ms, double gbs,
                  double gflops, int ok) {
    printf("%s %s ms=%.4f GB/s=%.1f GFLOP/s=%.1f %s\n",
           mode, dtype, ms, gbs, gflops, ok ? "PASS" : "FAIL");
    return ok ? 0 : 1;
}

int main(int argc, char** argv) {
    if (argc < 6) {
        fprintf(stderr, "usage: %s <copy|elem|reduce|gemm> <f32|f16> <M> <N> <K> <iters>\n", argv[0]);
        return 2;
    }
    const char* mode = argv[1];
    int half = strcmp(argv[2], "f16") == 0;
    long M = atol(argv[3]), N = atol(argv[4]), K = atol(argv[5]);
    int iters = argc > 6 ? atoi(argv[6]) : 10;
    const size_t esz = half ? 2 : 4;
    const char* dt = half ? "f16" : "f32";

    ClEnv e = {0};
    if (!cl_env_init(&e)) return 2;
    cl_program prog = build_program(&e, half);
    if (!prog) return 2;

    cl_kernel k = NULL;
    cl_mem b0 = NULL, b1 = NULL, b2 = NULL;
    double ms = -1, gbs = 0, gflops = 0;
    int ok = 0;
    double* times = calloc(iters, sizeof(double));

    if (strcmp(mode, "copy") == 0 || strcmp(mode, "elem") == 0) {
        const long n = M;
        k = p_clCreateKernel(prog, strcmp(mode, "copy") == 0 ? "copy_k" : "elem_k", NULL);
        if (!k) { fprintf(stderr, "no kernel\n"); return 2; }
        uint16_t* h0 = malloc((size_t)n * esz), *h1 = malloc((size_t)n * esz),
                *h2 = malloc((size_t)n * esz);
        for (long i = 0; i < n; i++) {
            if (half) { h0[i] = half_of_idx(i); h1[i] = half_of_idx(i + 2); }
            else {
                ((float*)h0)[i] = (float)val_of_idx(i);
                ((float*)h1)[i] = (float)val_of_idx(i + 2);
            }
        }
        b0 = make_buf(&e, (size_t)n * esz, h0, 1);
        b1 = make_buf(&e, (size_t)n * esz, h1, 1);
        b2 = make_buf(&e, (size_t)n * esz, NULL, 0);
        long nn = n;
        p_clSetKernelArg(k, 0, sizeof(cl_mem), &b0);
        if (strcmp(mode, "elem") == 0) p_clSetKernelArg(k, 1, sizeof(cl_mem), &b1);
        p_clSetKernelArg(k, strcmp(mode, "elem") == 0 ? 2 : 1, sizeof(cl_mem), &b2);
        p_clSetKernelArg(k, strcmp(mode, "elem") == 0 ? 3 : 2, sizeof(long), &nn);
        size_t g = ((size_t)n + LOCAL - 1) / LOCAL * LOCAL;
        for (int i = 0; i < WARMUP; i++) time_kernel(&e, k, g, 1, LOCAL, 1);
        for (int i = 0; i < iters; i++) {
            times[i] = time_kernel(&e, k, g, 1, LOCAL, 1);
        }
        // verify
        uint16_t* out = malloc((size_t)n * esz);
        p_clEnqueueReadBuffer(e.queue, b2, 1, 0, (size_t)n * esz, out, 0, NULL, NULL);
        p_clFinish(e.queue);
        ok = 1;
        for (long i = 0; i < n && ok; i++) {
            double got = half ? (double)half_to_float(out[i]) : (double)((float*)out)[i];
            double want = (double)val_of_idx(i) + (strcmp(mode, "elem") == 0 ? (double)val_of_idx(i + 2) : 0.0);
            if (fabs(got - want) > 1e-4 * fabs(want) + 1e-4) ok = 0;
        }
        free(out); free(h0); free(h1); free(h2);
        size_t bytes = (size_t)n * esz * (strcmp(mode, "elem") == 0 ? 3 : 2);
        gbs = 0; // filled after median
        qsort(times, iters, sizeof(double), cmp_double);
        ms = times[iters / 2];
        gbs = (double)bytes / (ms * 1e6);
    } else if (strcmp(mode, "reduce") == 0) {
        long rows = M, kk = N;
        k = p_clCreateKernel(prog, "reduce_k", NULL);
        if (!k) return 2;
        void* h0 = malloc((size_t)rows * kk * esz);
        float* out = calloc((size_t)rows, sizeof(float));
        for (long i = 0; i < rows * kk; i++) {
            if (half) ((uint16_t*)h0)[i] = half_of_idx(i);
            else ((float*)h0)[i] = (float)val_of_idx(i);
        }
        b0 = make_buf(&e, (size_t)rows * kk * esz, h0, 1);
        b1 = make_buf(&e, (size_t)rows * sizeof(float), NULL, 0);
        long karg = kk;
        p_clSetKernelArg(k, 0, sizeof(cl_mem), &b0);
        p_clSetKernelArg(k, 1, sizeof(cl_mem), &b1);
        p_clSetKernelArg(k, 2, sizeof(long), &karg);
        for (int i = 0; i < WARMUP; i++) time_kernel(&e, k, (size_t)rows * LOCAL, 1, LOCAL, 1);
        for (int i = 0; i < iters; i++) {
            times[i] = time_kernel(&e, k, (size_t)rows * LOCAL, 1, LOCAL, 1);
        }
        p_clEnqueueReadBuffer(e.queue, b1, 1, 0, (size_t)rows * sizeof(float), out, 0, NULL, NULL);
        p_clFinish(e.queue);
        ok = 1;
        for (long r = 0; r < rows && ok; r += 97) { // sampled rows
            double want = 0;
            for (long c = 0; c < kk; c++) {
                want += half ? val_of_idx(r * kk + c) : (double)((float*)h0)[r * kk + c];
            }
            if (fabs((double)out[r] - want) > 1e-3 * fabs(want) + 1e-3) ok = 0;
        }
        free(h0); free(out);
        qsort(times, iters, sizeof(double), cmp_double);
        ms = times[iters / 2];
        gbs = (double)((size_t)rows * kk * esz) / (ms * 1e6);
    } else if (strcmp(mode, "gemm") == 0) {
        k = p_clCreateKernel(prog, "gemm_k", NULL);
        if (!k) return 2;
        void* ha = malloc((size_t)M * K * esz);
        void* hb = malloc((size_t)K * N * esz);
        for (long i = 0; i < M * K; i++) {
            if (half) ((uint16_t*)ha)[i] = half_of_idx(i);
            else ((float*)ha)[i] = (float)val_of_idx(i);
        }
        for (long i = 0; i < K * N; i++) {
            if (half) ((uint16_t*)hb)[i] = half_of_idx(i + 1);
            else ((float*)hb)[i] = (float)val_of_idx(i + 1);
        }
        b0 = make_buf(&e, (size_t)M * K * esz, ha, 1);
        b1 = make_buf(&e, (size_t)K * N * esz, hb, 1);
        b2 = make_buf(&e, (size_t)M * N * esz, NULL, 0);
        long am = M, an = N, ak = K;
        p_clSetKernelArg(k, 0, sizeof(cl_mem), &b0);
        p_clSetKernelArg(k, 1, sizeof(cl_mem), &b1);
        p_clSetKernelArg(k, 2, sizeof(cl_mem), &b2);
        p_clSetKernelArg(k, 3, sizeof(long), &am);
        p_clSetKernelArg(k, 4, sizeof(long), &an);
        p_clSetKernelArg(k, 5, sizeof(long), &ak);
        size_t gx = (size_t)((N + 63) / 64) * 16;
        size_t gy = (size_t)((M + 63) / 64) * 16;
        for (int i = 0; i < WARMUP; i++) time_kernel(&e, k, gx, gy, 16, 16);
        for (int i = 0; i < iters; i++) {
            times[i] = time_kernel(&e, k, gx, gy, 16, 16);
        }
        void* hc = malloc((size_t)M * N * esz);
        p_clEnqueueReadBuffer(e.queue, b2, 1, 0, (size_t)M * N * esz, hc, 0, NULL, NULL);
        p_clFinish(e.queue);
        ok = 1;
        for (long r = 0; r < M && ok; r += (M / 16 > 0 ? M / 16 : 1)) {
            for (long c = 0; c < N && ok; c += (N / 16 > 0 ? N / 16 : 1)) {
                double want = 0;
                for (long t = 0; t < K; t++) {
                    double av = half ? val_of_idx(r * K + t) : (double)((float*)ha)[r * K + t];
                    double bv = half ? val_of_idx(t * N + c + 1) : (double)((float*)hb)[t * N + c];
                    want += av * bv;
                }
                double got = half ? (double)half_to_float(((uint16_t*)hc)[r * N + c])
                                  : (double)((float*)hc)[r * N + c];
                double tol = half ? 1e-2 : 1e-3;
                if (fabs(got - want) > tol * fabs(want) + tol) ok = 0;
            }
        }
        free(ha); free(hb); free(hc);
        qsort(times, iters, sizeof(double), cmp_double);
        ms = times[iters / 2];
        gflops = (2.0 * (double)M * (double)N * (double)K) / (ms * 1e6);
    } else {
        fprintf(stderr, "unknown mode %s\n", mode);
        return 2;
    }

    return report(mode, dt, ms, gbs, gflops, ok);
}
