// gemm_bench.c — correctness + perf for the .abv GEMM lane (plan
// 2026-09-01-m2-gemm). y[m*N+n] = sum_k A[m*K+k] * B[k*N+n].
// Usage: gemm_bench <kernel.spv> [M] [N] [K] [batch] [tiled]
//   correctness: R sampled output rows against a double reference.
//   perf: ITERS launches; batch=1 runs them as ONE submission per iter
//   group (per-call = wall/ITERS — the deployment-loop row).
//   tiled=1 → the blob is a 64x64-tile shared-memory kernel (LocalSize
//   16x16): dispatch workgroups = (M/64)*(N/64), nx items = wg*16.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>
#include <time.h>
#include "briev_accel_rt.h"

#define WARMUP 5
#define ITERS 20
#define VERIFY_ROWS 16

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e3 + (double)ts.tv_nsec / 1e6;
}

static unsigned char* state = NULL;
static uint64_t state_bytes = 0;

int main(int argc, char** argv) {
    if (argc < 2) { fprintf(stderr, "usage: %s <kernel.spv> [M] [N] [K] [batch]\n", argv[0]); return 2; }
    const uint64_t M = argc > 2 ? strtoull(argv[2], NULL, 10) : 4096;
    const uint64_t N = argc > 3 ? strtoull(argv[3], NULL, 10) : 4096;
    const uint64_t K = argc > 4 ? strtoull(argv[4], NULL, 10) : 4096;
    const int batch = argc > 5 ? atoi(argv[5]) : 0;
    const int tiled = argc > 6 ? atoi(argv[6]) : 0;

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

    // Name-sorted state: a, b, i, y. Host layout packed; device projection
    // 16B-aligns the vec4-eligible arrays (FnLowerer::projection_offsets).
    uint64_t off_a = 0;
    uint64_t off_b = off_a + M * K * 4;
    uint64_t off_i = (off_b + K * N * 4 + 7) & ~(uint64_t)7;
    uint64_t off_y = off_i + 8;
    state_bytes = off_y + M * N * 4 + 64;
    state = calloc(1, state_bytes);
    if (state == NULL) { fprintf(stderr, "oom\n"); return 2; }

    uint64_t proj_a = 0;
    uint64_t proj_b = (off_b + 15) & ~(uint64_t)15;
    uint64_t proj_i = proj_b + K * N * 4;
    uint64_t proj_y = (proj_i + 8 + 15) & ~(uint64_t)15;

    BrievField fields[] = {
        { "a", 1, off_a, 4, M * K, 1, proj_a },
        { "b", 1, off_b, 4, K * N, 1, proj_b },
        { "i", 2, off_i, 8, 1, 0, proj_i },
        { "y", 1, off_y, 4, M * N, 1, proj_y },
    };
    BrievKernelDesc desc = { "gemm", spv, (uint32_t)spv_len, 4, fields };

    float* a = (float*)(state + off_a);
    float* b = (float*)(state + off_b);
    float* y = (float*)(state + off_y);
    for (uint64_t j = 0; j < M * K; j++) a[j] = (float)((int)(j % 7)) * 0.25f;
    for (uint64_t j = 0; j < K * N; j++) b[j] = (float)((int)(j % 5)) * 0.5f;

    if (!briev_accel_init(&desc, 1)) {
        fprintf(stderr, "no GPU device\n"); return 1;
    }
    printf("# fingerprint: device=%s spv=%ldB M=%llu N=%llu K=%llu warmup=%d iters=%d batch=%d tiled=%d\n",
           briev_accel_device_name(), spv_len,
           (unsigned long long)M, (unsigned long long)N, (unsigned long long)K,
           WARMUP, ITERS, batch, tiled);

    // Dispatch geometry: naive items vs tiled workgroups*local_x.
    uint64_t dispatch_n = M * N;
    if (tiled) {
        dispatch_n = ((M + 63) / 64) * ((N + 63) / 64) * 16;
    }

    // Warm-up + verify (the state after warm-up holds real outputs).
    for (int w = 0; w < WARMUP; w++) {
        *(int64_t*)(state + off_i) = 0;
        int ok = briev_accel_launch_resident(0, state, dispatch_n);
        if (!ok) { fprintf(stderr, "dispatch failed\n"); return 1; }
    }
    if (!briev_accel_download(0, state)) {
        fprintf(stderr, "download failed\n"); return 1;
    }

    // Sampled correctness against a double reference.
    double max_rel = 0.0;
    uint64_t rows[VERIFY_ROWS];
    for (int r = 0; r < VERIFY_ROWS; r++) {
        rows[r] = (uint64_t)(r * 7919) % M;  // spread, deterministic
    }
    for (int r = 0; r < VERIFY_ROWS; r++) {
        const uint64_t m = rows[r];
        for (uint64_t n = 0; n < N; n++) {
            double ref = 0.0;
            for (uint64_t k = 0; k < K; k++) {
                ref += (double)a[m * K + k] * (double)b[k * N + n];
            }
            double rel = ref == 0.0 ? (double)y[m * N + n]
                                    : fabs((double)y[m * N + n] - ref) / fabs(ref);
            if (rel > max_rel) max_rel = rel;
        }
    }
    printf("# correctness: max_rel_err = %.3e (%s)\n", max_rel,
           max_rel <= 1e-3 ? "OK" : "FAIL");

    // Steady-state perf.
    double t0 = now_ms();
    if (batch) {
        *(int64_t*)(state + off_i) = 0;
        if (!briev_accel_launch_resident_batch(0, state, dispatch_n, 1, ITERS)) return 1;
        double wall = now_ms() - t0;
        double per = wall / ITERS;
        printf("GPU  per-call %.3f ms  %.2f GFLOP/s  (batched x%d)\n",
               per, 2.0 * (double)M * N * K / (per * 1e6), ITERS);
        // GFLOP/s = FLOP / (ms * 1e6) — ms*1e6 converts ms to picoseconds-
        // scaled GFLOP denominators; FLOP/(ms*1e6) = GFLOP/s directly.
    } else {
        double sum = 0, mn = 1e30, mx = 0;
        for (int it = 0; it < ITERS; it++) {
            *(int64_t*)(state + off_i) = 0;
            double t1 = now_ms();
            int ok = briev_accel_launch_resident(0, state, dispatch_n);
            double dt = now_ms() - t1;
            if (!ok) { fprintf(stderr, "dispatch failed\n"); return 1; }
            sum += dt; if (dt < mn) mn = dt; if (dt > mx) mx = dt;
        }
        printf("GPU  avg %.3f ms  min %.3f ms  max %.3f ms  %.2f GFLOP/s\n",
               sum / ITERS, mn, mx, 2.0 * (double)M * N * K / ((sum / ITERS) * 1e6));
    }
    double cpu_ref_t = now_ms();
    double ref0 = 0.0;
    for (uint64_t k = 0; k < K; k++) ref0 += (double)a[k] * (double)b[k * N];
    (void)ref0; (void)cpu_ref_t;

    briev_accel_shutdown();
    return 0;
}
