// gemv_bench.c — M1 benchmark harness (plan 2026-08-31-gpu-next, item 2).
//
// Reusable C harness over briev_accel_rt: times the compiled gemv kernel
// (examples/gpu/gemv.abv → gemv.spv) against a single-thread CPU reference
// at the same shape. Warm-up separated from steady state; correctness
// verified against the CPU reference before timing is reported.
//
// State layout matches the generated gemv runner exactly (name-sorted
// SSBO projection: a, i, x, y; arrays f32 on device, scalar i64):
//   a @ 0            (M*K * 4 bytes)
//   i @ a_end        (8 bytes, 8-aligned)
//   x @ i_end + 0    (K * 4 bytes)
//   y @ x_end        (M * 4 bytes)
//
// 2026-09-01 (plan vec4-projection-layout): the HOST buffer keeps the packed
// layout above; the DEVICE projection 16B-aligns vec4-eligible arrays (the
// shared FnLowerer::projection_offsets rule), so the BrievField proj_offset
// entries below shift x/y up — that is what makes x vec4-loadable in the
// cooperative kernel.
//
// Usage: gemv_bench <kernel.spv> [M] [K] [coop] [batch]
//   coop=1 → dispatch as cooperative row kernels: 32 lanes x M rows.
//   batch=1 → steady-state loop as ONE batched submission (times=ITERS):
//     per-call = wall/ITERS. Isolates the kernel from the per-launch fence
//     wake (~33us) that dominates small-M per-call times (plan
//     2026-09-01-smallm-splitk). The ledger keeps BOTH rows: per-call sync
//     (apples-to-apples vs ggml's synchronous compute) and batched.
//
// Evidence rules (VITRIOL ledger): prints the config fingerprint, warm-up
// count, steady-state iterations, min/avg/max, and GFLOP/s (2*M*K flops).

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>
#include <time.h>

#include "briev_accel_rt.h"

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e3 + (double)ts.tv_nsec / 1e6;
}

static unsigned char* state = NULL;
static uint64_t state_bytes = 0;

#define WARMUP 5
#define ITERS 20

int main(int argc, char** argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <kernel.spv> [M] [K]\n", argv[0]);
        return 2;
    }
    const uint64_t M = argc > 2 ? strtoull(argv[2], NULL, 10) : 4096;
    const uint64_t K = argc > 3 ? strtoull(argv[3], NULL, 10) : 4096;
    // 2026-09-02: default 1 — gemv.abv has compiled to the cooperative-row
    // kernel since plan 2026-09-01; a 1D dispatch on it writes nothing
    // (rows decode from gid.y) and the bench silently read zeros as rel
    // 1.0. The 0.199ms ledger row was the 2D invocation.
    const int coop = argc > 4 ? atoi(argv[4]) : 1;
    const int batch = argc > 5 ? atoi(argv[5]) : 0;

    // Load the SPIR-V blob (read from file — the harness stays reusable
    // across recompiles of the kernel).
    FILE* f = fopen(argv[1], "rb");
    if (f == NULL) { perror("spv"); return 2; }
    fseek(f, 0, SEEK_END);
    long spv_len = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t* spv = malloc((size_t)spv_len);
    if (spv == NULL || fread(spv, 1, (size_t)spv_len, f) != (size_t)spv_len) {
        fprintf(stderr, "briev: short read on %s\n", argv[1]);
        return 2;
    }
    fclose(f);

    // State layout (see header comment). Keep 8-byte alignment for the i64.
    uint64_t off_a = 0;
    uint64_t off_i = (off_a + M * K * 4 + 7) & ~(uint64_t)7;
    uint64_t off_x = off_i + 8;
    uint64_t off_y = off_x + K * 4;
    state_bytes = off_y + M * 4 + 64;
    state = calloc(1, state_bytes);
    if (state == NULL) { fprintf(stderr, "oom\n"); return 2; }

    // Device projection offsets: vec4-eligible arrays (a, x) aligned to 16B.
    uint64_t proj_x = (off_x + 15) & ~(uint64_t)15;
    uint64_t proj_y = (proj_x + K * 4 + 15) & ~(uint64_t)15;

    BrievField fields[] = {
        { "a", 1, off_a, 4, M * K, 1, 0 },
        { "i", 2, off_i, 8, 1, 0, off_i },
        { "x", 1, off_x, 4, K, 1, proj_x },
        { "y", 1, off_y, 4, M, 1, proj_y },
    };
    BrievKernelDesc desc = { "gemv", spv, (uint32_t)spv_len, 4, fields };

    // Deterministic fill.
    float* a = (float*)(state + off_a);
    float* x = (float*)(state + off_x);
    float* y = (float*)(state + off_y);
    for (uint64_t j = 0; j < M * K; j++) a[j] = (float)(j % 7) * 0.25f;
    for (uint64_t k = 0; k < K; k++) x[k] = (float)(k % 5) * 0.5f;

    if (!briev_accel_init(&desc, 1)) {
        fprintf(stderr, "briev: no GPU device available\n");
        return 1;
    }
    printf("# fingerprint: device=%s spv=%ldB M=%llu K=%llu warmup=%d iters=%d\n",
           briev_accel_device_name(), spv_len,
           (unsigned long long)M, (unsigned long long)K, WARMUP, ITERS);

    // Warm-up (JIT/pipe setup excluded from steady-state numbers).
    for (int w = 0; w < WARMUP; w++) {
        *(int64_t*)(state + off_i) = 0;
        int ok = coop ? briev_accel_launch_resident_2d(0, state, 32, M)
                      : briev_accel_launch_resident(0, state, M);
        if (!ok) {
            fprintf(stderr, "briev: dispatch failed\n");
            return 1;
        }
    }
    if (!briev_accel_download(0, state)) {
        fprintf(stderr, "briev: download failed\n");
        return 1;
    }

    // Correctness gate vs the CPU reference (double accumulate).
    double max_rel = 0.0;
    for (uint64_t m = 0; m < M; m++) {
        double ref = 0.0;
        for (uint64_t k = 0; k < K; k++) {
            ref += (double)a[m * K + k] * (double)x[k];
        }
        double rel = ref == 0.0 ? (double)y[m]
                                : fabs((double)y[m] - ref) / fabs(ref);
        if (rel > max_rel) max_rel = rel;
    }
    printf("# correctness: max_rel_err = %.3e (%s)\n", max_rel,
           max_rel < 1e-3 ? "OK" : "FAIL");
    if (max_rel >= 1e-3) return 1;

    // Steady-state GPU timing.
    double gpu_min = 1e30, gpu_max = 0.0, gpu_sum = 0.0;
    if (batch) {
        // One submission carrying ITERS identical dispatches (scalars are
        // launch-invariant: i is reset once). Per-call = wall / ITERS.
        *(int64_t*)(state + off_i) = 0;
        double t0 = now_ms();
        int ok = coop
            ? briev_accel_launch_resident_batch(0, state, 32, M, ITERS)
            : briev_accel_launch_resident_batch(0, state, M, 1, ITERS);
        double dt = now_ms() - t0;
        if (!ok) {
            fprintf(stderr, "briev: dispatch failed\n");
            return 1;
        }
        gpu_sum = dt;
        gpu_min = gpu_max = dt / ITERS;
    } else {
        for (int it = 0; it < ITERS; it++) {
            *(int64_t*)(state + off_i) = 0;
            double t0 = now_ms();
            int ok = coop ? briev_accel_launch_resident_2d(0, state, 32, M)
                          : briev_accel_launch_resident_2d(0, state, 32, M) /* coop-row geometry */;
            if (!ok) {
                fprintf(stderr, "briev: dispatch failed\n");
                return 1;
            }
            double dt = now_ms() - t0;
            gpu_sum += dt;
            if (dt < gpu_min) gpu_min = dt;
            if (dt > gpu_max) gpu_max = dt;
        }
    }
    double gpu_avg = gpu_sum / ITERS;
    double gflop = 2.0 * (double)M * (double)K / 1e9;
    printf("GPU  avg %8.3f ms  min %8.3f ms  max %8.3f ms  %8.2f GFLOP/s\n",
           gpu_avg, gpu_min, gpu_max, gflop / (gpu_avg / 1e3));

    // Single-thread CPU reference (same shape, double accumulate).
    double* yt = malloc(M * sizeof(double));
    double t0 = now_ms();
    for (uint64_t m = 0; m < M; m++) {
        double acc = 0.0;
        for (uint64_t k = 0; k < K; k++) {
            acc += (double)a[m * K + k] * (double)x[k];
        }
        yt[m] = acc;
    }
    double cpu_avg = now_ms() - t0;
    printf("CPU  avg %8.3f ms                            %8.2f GFLOP/s  (1 thread)\n",
           cpu_avg, gflop / (cpu_avg / 1e3));
    printf("RATIO gpu/cpu = %.2fx\n", cpu_avg / gpu_avg);

    free(yt);
    briev_accel_shutdown();
    return 0;
}
