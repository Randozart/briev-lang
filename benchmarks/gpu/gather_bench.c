// gather_bench.c — strided-gather benchmark (plan 2026-09-02, GPU
// portfolio). dst[i] = src[i * 8] for i in 0..N: each work item reads
// from a stride-8 (32-byte) location and writes contiguously. Tests
// DRAM random-read throughput — the complement of saxpy's sequential
// stream. N = 16.7M; src = 8N elements (512MB), dst = N (64MB).
// Correctness gated against a CPU reference, then bandwidth reported.
//
// State layout mirrors the generated runner (name-sorted: dst, i, src):
//   dst @ 0 (N*4), i @ N*4 (8), src @ N*4+8 (8N*4)
// Proj offsets from the generated runner: dst 0, i 67108864, src 67108880.
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
    if (argc < 2) { fprintf(stderr, "usage: %s <kernel.spv> [N] [iters]\n", argv[0]); return 2; }
    const uint64_t N = argc > 2 ? strtoull(argv[2], NULL, 10) : 16777216ull;
    const uint64_t NSRC = N * 8;
    const int iters = argc > 3 ? atoi(argv[3]) : ITERS;

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

    // Host layout (name-sorted: dst, i, src) — derived from the generated runner.
    uint64_t off_dst = 0;
    uint64_t off_i = N * 4;
    uint64_t off_src = off_i + 8;
    uint64_t state_bytes = off_src + NSRC * 4 + 64;
    unsigned char* state = calloc(1, state_bytes);
    if (state == NULL) { fprintf(stderr, "oom\n"); return 2; }

    // Proj offsets from the generated runner's field table.
    uint64_t proj_dst = 0;
    uint64_t proj_i = off_i;    // 67108864
    uint64_t proj_src = off_i + 16; // 67108880 (16B-aligned past i)

    BrievField fields[] = {
        { "dst", 1, off_dst, 4, N, 1, proj_dst },
        { "i", 2, off_i, 8, 1, 0, proj_i },
        { "src", 1, off_src, 4, NSRC, 1, proj_src },
    };
    BrievKernelDesc desc = { "gather", spv, (uint32_t)spv_len, 3, fields };

    float* src = (float*)(state + off_src);
    float* dst = (float*)(state + off_dst);
    for (uint64_t j = 0; j < NSRC; j++) src[j] = (float)(j % 1000) * 0.125f;

    if (!briev_accel_init(&desc, 1)) {
        fprintf(stderr, "no GPU device\n"); return 1;
    }
    printf("# fingerprint: device=%s spv=%ldB N=%llu srcN=%llu warmup=%d iters=%d\n",
           briev_accel_device_name(), spv_len, (unsigned long long)N,
           (unsigned long long)NSRC, WARMUP, iters);

    for (int w = 0; w < WARMUP; w++) {
        *(int64_t*)(state + off_i) = 0;
        if (!briev_accel_launch_resident(0, state, N)) {
            fprintf(stderr, "dispatch failed\n"); return 1;
        }
    }
    if (!briev_accel_download(0, state)) {
        fprintf(stderr, "download failed\n"); return 1;
    }

    // Correctness: dst[i] should equal src[i*8] for all i.
    double max_abs = 0.0;
    for (uint64_t i = 0; i < N; i++) {
        float ref = src[i * 8];
        double d = fabs((double)dst[i] - (double)ref);
        if (d > max_abs) max_abs = d;
    }
    printf("# correctness: max_abs_err = %.3e (%s)\n", max_abs,
           max_abs <= 1e-3 ? "OK" : "FAIL");

    // Bandwidth: N reads (stride-8 = 32B apart) + N writes (contiguous).
    // Bytes moved = N*4 read + N*4 write = 2*N*4.
    double bytes = (double)N * 4.0 * 2.0;
    double sum = 0, mn = 1e30, mx = 0;
    for (int it = 0; it < iters; it++) {
        *(int64_t*)(state + off_i) = 0;
        double t1 = now_ms();
        int ok = briev_accel_launch_resident(0, state, N);
        double dt = now_ms() - t1;
        if (!ok) { fprintf(stderr, "dispatch failed\n"); return 1; }
        sum += dt; if (dt < mn) mn = dt; if (dt > mx) mx = dt;
    }
    printf("GPU  avg %.3f ms  min %.3f ms  max %.3f ms  %.1f GB/s\n",
           sum / iters, mn, mx, bytes / ((sum / iters) * 1e6));

    briev_accel_shutdown();
    return 0;
}
