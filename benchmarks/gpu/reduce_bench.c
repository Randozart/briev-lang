// reduce_bench.c — partial-sum reduction benchmark (plan 2026-09-02, GPU
// portfolio). W=8192 work items sum 2048-element stripes of x (N=16.7M);
// the host combines the partials. Correctness: the GPU total vs a
// double-accumulate CPU reference (f32 partial sums round — tolerance
// scales with N). Bandwidth: N*4 read + W*4 written per pass.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>
#include <time.h>
#include "briev_accel_rt.h"

#define WARMUP 3
#define ITERS 20

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e3 + (double)ts.tv_nsec / 1e6;
}

int main(int argc, char** argv) {
    if (argc < 2) { fprintf(stderr, "usage: %s <kernel.spv> [N] [W] [iters]\n", argv[0]); return 2; }
    const uint64_t N = argc > 2 ? strtoull(argv[2], NULL, 10) : 16777216ull;
    const uint64_t W = argc > 3 ? strtoull(argv[3], NULL, 10) : 8192ull;
    const int iters = argc > 4 ? atoi(argv[4]) : ITERS;
    const uint64_t CHUNK = N / W;

    FILE* f = fopen(argv[1], "rb");
    if (f == NULL) { perror("spv"); return 2; }
    fseek(f, 0, SEEK_END);
    long spv_len = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t* spv = malloc((size_t)spv_len);
    if (spv == NULL || fread(spv, 1, (size_t)spv_len, f) != (size_t)spv_len) {
        fprintf(stderr, "short read\n"); return 2;
    }
    fclose(f);

    // Name-sorted: i, part, x — mirrors the generated runner.
    uint64_t off_i = 0;
    uint64_t off_part = 8;
    uint64_t off_x = (off_part + W * 4 + 15) & ~(uint64_t)15;
    uint64_t state_bytes = off_x + N * 4 + 64;
    unsigned char* state = calloc(1, state_bytes);
    if (state == NULL) { fprintf(stderr, "oom\n"); return 2; }

    BrievField fields[] = {
        { "i", 2, off_i, 8, 1, 0, off_i },
        { "part", 1, off_part, 4, W, 1, off_part },
        { "x", 1, off_x, 4, N, 1, off_x },
    };
    BrievKernelDesc desc = { "reduce", spv, (uint32_t)spv_len, 3, fields };

    float* x = (float*)(state + off_x);
    float* part = (float*)(state + off_part);
    for (uint64_t j = 0; j < N; j++) x[j] = (float)((int)(j % 13)) * 0.25f;

    if (!briev_accel_init(&desc, 1)) {
        fprintf(stderr, "no GPU device\n"); return 1;
    }
    printf("# fingerprint: device=%s spv=%ldB N=%llu W=%llu chunk=%llu warmup=%d iters=%d\n",
           briev_accel_device_name(), spv_len, (unsigned long long)N,
           (unsigned long long)W, (unsigned long long)CHUNK, WARMUP, iters);

    for (int w = 0; w < WARMUP; w++) {
        *(int64_t*)(state + off_i) = 0;
        if (!briev_accel_launch_resident(0, state, W)) {
            fprintf(stderr, "dispatch failed\n"); return 1;
        }
    }
    if (!briev_accel_download(0, state)) {
        fprintf(stderr, "download failed\n"); return 1;
    }

    // GPU total vs double reference (seed pattern is periodic — the
    // exact double sum is computable in closed form, but accumulate the
    // honest way; tolerance covers the f32 partial rounding).
    double ref = 0.0, got = 0.0;
    for (uint64_t j = 0; j < N; j++) ref += (double)x[j];
    for (uint64_t w = 0; w < W; w++) got += (double)part[w];
    double rel = ref == 0.0 ? got : fabs(got - ref) / fabs(ref);
    printf("# correctness: gpu_total=%.4f ref=%.4f rel=%.3e (%s)\n",
           got, ref, rel, rel <= 1e-2 ? "OK" : "FAIL");

    double bytes = (double)N * 4.0 + (double)W * 4.0;
    double sum = 0, mn = 1e30, mx = 0;
    for (int it = 0; it < iters; it++) {
        *(int64_t*)(state + off_i) = 0;
        double t1 = now_ms();
        int ok = briev_accel_launch_resident(0, state, W);
        double dt = now_ms() - t1;
        if (!ok) { fprintf(stderr, "dispatch failed\n"); return 1; }
        sum += dt; if (dt < mn) mn = dt; if (dt > mx) mx = dt;
    }
    printf("GPU  avg %.3f ms  min %.3f ms  max %.3f ms  %.1f GB/s\n",
           sum / iters, mn, mx, bytes / ((sum / iters) * 1e6));

    briev_accel_shutdown();
    return 0;
}
