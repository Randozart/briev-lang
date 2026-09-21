// saxpy_bench.c — memory-bound elementwise benchmark (plan 2026-09-02,
// GPU portfolio: the anti-overfit spread beyond matmul). y = a*x + y at
// N = 16.7M f32 elements: 3×67MB of DRAM traffic per pass. Correctness
// gated against a CPU reference, then bandwidth reported (bytes moved /
// time — the ONLY meaningful number for this pattern).
// State layout mirrors the generated saxpy_runner.c:
//   i @ 0 (8B), x @ 8 (N*4), y @ 8+N*4 (N*4)
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
    const int iters = argc > 3 ? atoi(argv[3]) : ITERS;
    const float ALPHA = 2.0f;

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

    // HOST layout (packed) vs DEVICE projection (16B-aligned vec4-eligible
    // arrays — FnLowerer::projection_offsets). The runtime pushes/pulls at
    // the PROJ offsets; the generated runner is the authority (the
    // saxpy-runner pairs: i 0/0, x 8/16, y 8+4N/16+4N).
    // HOST layout (packed) vs DEVICE projection (16B-aligned vec4-eligible
    // arrays — FnLowerer::projection_offsets); the generated runner is the
    // authority (i 0/0, x 8/16, y ..., z ...).
    uint64_t off_i = 0;
    uint64_t off_x = 8;
    uint64_t off_y = off_x + N * 4;
    uint64_t off_z = off_y + N * 4;
    uint64_t proj_i = 0;
    uint64_t proj_x = 16;
    uint64_t proj_y = (proj_x + N * 4 + 15) & ~(uint64_t)15;
    uint64_t proj_z = (proj_y + N * 4 + 15) & ~(uint64_t)15;
    uint64_t proj_bytes = proj_z + N * 4 + 64;
    uint64_t state_bytes = off_z + N * 4 + 64;
    if (proj_bytes > state_bytes) { state_bytes = proj_bytes; }
    unsigned char* state = calloc(1, state_bytes);
    if (state == NULL) { fprintf(stderr, "oom\n"); return 2; }

    BrievField fields[] = {
        { "i", 2, off_i, 8, 1, 0, proj_i },
        { "x", 1, off_x, 4, N, 1, proj_x },
        { "y", 1, off_y, 4, N, 1, proj_y },
        { "z", 1, off_z, 4, N, 1, proj_z },
    };
    BrievKernelDesc desc = { "saxpy", spv, (uint32_t)spv_len, 4, fields };

    float* x = (float*)(state + off_x);
    float* y = (float*)(state + off_y);
    float* z = (float*)(state + off_z);
    for (uint64_t j = 0; j < N; j++) x[j] = (float)(j % 17) * 0.5f;
    for (uint64_t j = 0; j < N; j++) y[j] = (float)(j % 11);

    if (!briev_accel_init(&desc, 1)) {
        fprintf(stderr, "no GPU device\n"); return 1;
    }
    printf("# fingerprint: device=%s spv=%ldB N=%llu warmup=%d iters=%d\n",
           briev_accel_device_name(), spv_len, (unsigned long long)N, WARMUP, iters);

    for (int w = 0; w < WARMUP; w++) {
        *(int64_t*)(state + off_i) = 0;
        // z = alpha*x + y is IDEMPOTENT (separate destination) — every
        // pass writes the same z, so pass count cannot corrupt the check.
        if (!briev_accel_launch_resident(0, state, N)) {
            fprintf(stderr, "dispatch failed\n"); return 1;
        }
    }
    if (!briev_accel_download(0, state)) {
        fprintf(stderr, "download failed\n"); return 1;
    }

    double max_abs = 0.0;
    for (uint64_t j = 0; j < N; j++) {
        float ref = ALPHA * x[j] + (float)(j % 11);
        double d = fabs((double)z[j] - (double)ref);
        if (d > max_abs) max_abs = d;
    }
    printf("# correctness: max_abs_err = %.3e (%s)\n", max_abs,
           max_abs <= 1e-3 ? "OK" : "FAIL");

    double bytes = (double)N * 4.0 * 3.0; // 2 reads + 1 write
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
