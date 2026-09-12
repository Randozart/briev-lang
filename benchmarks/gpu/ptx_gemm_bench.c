// ptx_gemm_bench.c — standalone driver-API bench + correctness for the
// dump-test PTX tensor kernels (checked in 2026-09-11: the /tmp-wipe of the
// ad-hoc bench2/tharness cost a full rebuild; the tool lives here now).
//
// State layout matches the dump tests in src/backend/ptx/tensor.rs:
//   y @ 0 (M*N*2), a @ a_off (M*K*2), i (8B), b @ b_off (K*N*2).
// The kernel bakes its offsets at emission — pass the SAME values here.
//
// usage: ptx_gemm_bench <kernel.cubin|kernel.ptx> M N K a_off b_off y_off
//                       threads grid_ctas [check]
//   offsets  = the values baked into the kernel at emission (the dump tests
//              use a_off=0, b_off=M*K*2, y_off=b_off+K*N*2+8)
//   threads  = block threads the kernel was emitted for (e.g. 512)
//   grid_ctas = total CTAs = (M/(32*mw)) * (N/(64*nw)) (or the mhr geometry)
//   check    = optional: run the CPU reference and print max_rel_err
// env: BRIEV_GEMM_F16ACC=1 → 1e-2 gate (else 5e-3); BRIEV_ITERS (default 20);
//      BRIEV_WARMUP (default 5); BRIEV_BATCH=1 → batched submission timing.
#include <cuda.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <stdint.h>

static uint16_t f32_to_f16(float f) {
    uint32_t x;
    memcpy(&x, &f, 4);
    uint16_t h = (uint16_t)((x >> 16) & 0x8000);
    int32_t e = (int32_t)((x >> 23) & 0xff) - 127 + 15;
    uint32_t m = x & 0x7fffff;
    if (e <= 0) return h;
    if (e >= 31) return (uint16_t)(h | 0x7c00);
    h |= (uint16_t)(e << 10) | (uint16_t)(m >> 13);
    return h;
}

static float f16_to_f32(uint16_t h) {
    uint32_t sign = (h & 0x8000) << 16;
    uint32_t exp = (h >> 10) & 0x1f;
    uint32_t man = h & 0x3ff;
    uint32_t bits;
    if (exp == 0) {
        if (man == 0) { bits = sign; }
        else { // subnormal
            int e = -1;
            uint32_t m = man;
            do { m <<= 1; e++; } while (!(m & 0x400));
            m &= 0x3ff;
            bits = sign | ((127 - 15 - e + 1) << 23) | (m << 13);
        }
    } else if (exp == 31) {
        bits = sign | 0x7f800000 | (man << 13);
    } else {
        bits = sign | ((exp - 15 + 127) << 23) | (man << 13);
    }
    float f;
    memcpy(&f, &bits, 4);
    return f;
}

#define CU_CHECK(call, what) do { \
    CUresult _r = (call); \
    if (_r != CUDA_SUCCESS) { \
        const char* _s = "?"; cuGetErrorString(_r, &_s); \
        fprintf(stderr, "%s failed: %s (%d)\n", what, _s, (int)_r); return 1; \
    } \
} while (0)

int main(int argc, char** argv) {
    if (argc < 9) {
        fprintf(stderr, "usage: %s <cubin> M N K a_off b_off y_off threads grid_ctas [check]\n", argv[0]);
        return 2;
    }
    uint64_t M = strtoull(argv[2], NULL, 10);
    uint64_t N = strtoull(argv[3], NULL, 10);
    uint64_t K = strtoull(argv[4], NULL, 10);
    uint64_t a_off = strtoull(argv[5], NULL, 10);
    uint64_t b_off = strtoull(argv[6], NULL, 10);
    uint64_t y_off = strtoull(argv[7], NULL, 10);
    uint32_t threads = (uint32_t)atoi(argv[8]);
    uint32_t ctas = (uint32_t)atoi(argv[9]);
    int check = argc > 10;

    FILE* f = fopen(argv[1], "rb");
    if (!f) { fprintf(stderr, "cannot open '%s'\n", argv[1]); return 1; }
    fseek(f, 0, SEEK_END);
    long bytes = ftell(f);
    fseek(f, 0, SEEK_SET);
    char* img = malloc((size_t)bytes + 1);
    if (fread(img, 1, (size_t)bytes, f) != (size_t)bytes) { fprintf(stderr, "short read\n"); return 1; }
    img[bytes] = 0;
    fclose(f);

    CU_CHECK(cuInit(0), "cuInit");
    CUdevice dev;
    CU_CHECK(cuDeviceGet(&dev, 0), "cuDeviceGet");
    CUcontext ctx;
    // CUDA 13: cuCtxCreate maps to v4 (pctx, ctxCreateParams, flags, dev).
    CUresult rc2 = cuCtxCreate_v4(&ctx, NULL, 0, dev);
    if (rc2 != CUDA_SUCCESS) { fprintf(stderr, "cuCtxCreate failed (%d)\n", (int)rc2); return 1; }

    CUmodule mod;
    CUresult rc = cuModuleLoadData(&mod, img);
    if (rc != CUDA_SUCCESS) {
        const char* s = "?"; cuGetErrorString(rc, &s);
        fprintf(stderr, "cuModuleLoadData: %s (%d)\n", s, (int)rc);
        return 1;
    }
    CUfunction fn;
    CU_CHECK(cuModuleGetFunction(&fn, mod, "main"), "cuModuleGetFunction");

    // state: a @ a_off, b @ b_off, i @ (y_off - 8), y @ y_off.
    // BRIEV_Y_ELEM: y element size — 2 (f16, default) or 4 (f32 tier). The
    // f32 kernels store an M*N*4 y tile; a 2-byte state buffer faults them
    // at check time (found 2026-09-12 driving the f32 warp_mh A/B).
    const char* yelem_env = getenv("BRIEV_Y_ELEM");
    uint64_t y_elem_sz = yelem_env ? strtoull(yelem_env, NULL, 10) : 2;
    uint64_t state_bytes = y_off + M * N * y_elem_sz + 64;
    unsigned char* state;
    CU_CHECK(cuMemAllocHost((void**)&state, state_bytes), "cuMemAllocHost");
    memset(state, 0, state_bytes);
    uint16_t* a = (uint16_t*)(state + a_off);
    uint16_t* b = (uint16_t*)(state + b_off);
    for (uint64_t j = 0; j < M * K; j++) a[j] = f32_to_f16((float)((int)(j % 7)) * 0.25f);
    for (uint64_t j = 0; j < K * N; j++) b[j] = f32_to_f16((float)((int)(j % 5)) * 0.5f);

    CUdeviceptr dev_state;
    CU_CHECK(cuMemAlloc(&dev_state, state_bytes), "cuMemAlloc");
    CU_CHECK(cuMemcpyHtoD(dev_state, state, state_bytes), "seed HtoD");

    // smem: the dump kernels allocate ALL working memory dynamically —
    // sharedMemBytes MUST match the emission (MW_SMEM semantics, 0 = no
    // dynamic smem = immediate OOB faults in the fills). Values above the
    // 48KB default also need the opt-in attribute.
    const char* smem_env = getenv("MW_SMEM");
    unsigned smem = smem_env ? (unsigned)atoi(smem_env) : 0;
    if (smem > 48 * 1024) {
        CU_CHECK(cuFuncSetAttribute(fn, CU_FUNC_ATTRIBUTE_MAX_DYNAMIC_SHARED_SIZE_BYTES,
                                    (int)smem), "cuFuncSetAttribute");
    }

    void* params[] = { &dev_state };
    int iters = getenv("BRIEV_ITERS") ? atoi(getenv("BRIEV_ITERS")) : 20;
    int warmup = getenv("BRIEV_WARMUP") ? atoi(getenv("BRIEV_WARMUP")) : 5;
    int batch = getenv("BRIEV_BATCH") ? atoi(getenv("BRIEV_BATCH")) : 0;

    // correctness (one launch, sync, copy back)
    if (check) {
        CU_CHECK(cuLaunchKernel(fn, ctas, 1, 1, threads, 1, 1, smem, 0, params, NULL), "check launch");
        CU_CHECK(cuCtxSynchronize(), "check sync");
        CU_CHECK(cuMemcpyDtoH(state, dev_state, state_bytes), "DtoH");
        const char* gate_env = getenv("BRIEV_GEMM_F16ACC");
        double gate = gate_env ? 1e-2 : 5e-3;
        // sampled reference: fp32 accumulate over the f16 inputs
        double worst = 0.0;
        uint64_t worst_mn = 0;
        for (uint64_t sample = 0; sample < 4096; sample++) {
            uint64_t mn = ((sample * 2654435761u) >> 5) % (M * N);
            uint64_t m = mn / N, n = mn % N;
            double acc = 0.0;
            for (uint64_t k = 0; k < K; k++)
                acc += (double)f16_to_f32(a[m * K + k]) * (double)f16_to_f32(b[k * N + n]);
            double g;
            if (y_elem_sz == 4) {
                uint32_t got32;
                memcpy(&got32, state + y_off + mn * 4, 4);
                float gf;
                memcpy(&gf, &got32, 4);
                g = (double)gf;
            } else {
                uint16_t got;
                memcpy(&got, state + y_off + mn * 2, 2);
                g = (double)f16_to_f32(got);
            }
            double ref_v = acc;
            double rel = ref_v != 0.0 ? (g - ref_v) / ref_v : g;
            if (rel < 0) rel = -rel;
            if (rel > worst) { worst = rel; worst_mn = mn; }
        }
        printf("max_rel_err = %.3e %s (worst y[%llu])\n", worst, worst <= gate ? "OK" : "FAIL",
               (unsigned long long)worst_mn);
    }

    // timing
    for (int w = 0; w < warmup; w++) {
        CU_CHECK(cuLaunchKernel(fn, ctas, 1, 1, threads, 1, 1, smem, 0, params, NULL), "warmup launch");
    }
    CU_CHECK(cuCtxSynchronize(), "warmup sync");
    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    for (int it = 0; it < iters; it++) {
        CUresult r = cuLaunchKernel(fn, ctas, 1, 1, threads, 1, 1, smem, 0, params, NULL);
        if (r != CUDA_SUCCESS) { fprintf(stderr, "launch failed (%d)\n", (int)r); return 1; }
        if (!batch) CU_CHECK(cuCtxSynchronize(), "iter sync");
    }
    if (batch) CU_CHECK(cuCtxSynchronize(), "batch sync");
    clock_gettime(CLOCK_MONOTONIC, &t1);
    double ms = ((double)(t1.tv_sec - t0.tv_sec) * 1e3 + (double)(t1.tv_nsec - t0.tv_nsec) / 1e6) / iters;
    double tf = 2.0 * (double)M * (double)N * (double)K / (ms * 1e-3) / 1e12;
    printf("%llu×%llu×%llu (block=%u ctas=%u batch=%d): %.3f ms | %.2f TFLOP/s\n",
           (unsigned long long)M, (unsigned long long)N, (unsigned long long)K,
           threads, ctas, batch, ms, tf);
    return 0;
}
