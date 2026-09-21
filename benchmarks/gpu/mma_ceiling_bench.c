// mma_ceiling_bench.c — Stage 0 instrument (plan 2026-09-04-beyond-coopmat):
// measures the coopmat TENSOR CEILING through the vendor SPIR-V lowering.
// The microkernel (src/backend/spirv/gemm.rs emit_mma_ceiling_kernels) is a
// register-resident mma chain — no smem, no DRAM in the loop — so its
// throughput IS the lowering's ceiling. The f16acc-vs-f32acc pair answers
// the double-pump question (doctrine abv-gpu-doctrine.md §2).
//
// Usage:
//   mma_ceiling_bench <kernel.spv> [bound] [launches] [warmup] [kernelB.spv]
// With kernelB: alternating A/B rounds (shared DVFS window, the gemm_h
// harness discipline). FLOP/launch = WGS * bound * DEPTH * 8192, fixed
// geometry WGS=4096, DEPTH=64 (must match the generator's constants).
//
// State layout (name-sorted, packed — matches the generator's SSBO):
//   i @ 0  (i64: runtime loop bound IN, final counter OUT)
//   y @ 8  (f32[256*WGS + 1]: one 16x16 tile per workgroup, +1 vec4 pad)
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>
#include <time.h>
#include "briev_accel_rt.h"

#define WGS 4096
#define DEPTH 64
#define AB_ROUNDS 5

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e3 + (double)ts.tv_nsec / 1e6;
}

static uint8_t* read_spv(const char* path, long* len_out) {
    FILE* f = fopen(path, "rb");
    if (f == NULL) { perror(path); return NULL; }
    fseek(f, 0, SEEK_END);
    long len = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t* buf = malloc((size_t)len);
    if (buf == NULL || fread(buf, 1, (size_t)len, f) != (size_t)len) {
        fprintf(stderr, "short read: %s\n", path);
        free(buf);
        fclose(f);
        return NULL;
    }
    fclose(f);
    *len_out = len;
    return buf;
}

static const uint64_t OFF_A = 0;      // a: member 0 @0 (16M halves)
static const uint64_t OFF_I = 16 * 1048576 * 2;  // i: member 1 @32MB
static const uint64_t OFF_Y = OFF_I + 8;         // y: member 2 @32MB+8
static const uint64_t Y_COUNT = (WGS + 15) * 4096 + 16;
static const uint64_t A_COUNT = 16 * 1048576;

// f32→f16 RNE (mirrors the backend's encoder) — the A seed must be f16.
static uint16_t f32_to_f16(float v) {
    uint32_t bits; memcpy(&bits, &v, 4);
    uint16_t sign = (uint16_t)((bits >> 16) & 0x8000u);
    int32_t exp = (int32_t)((bits >> 23) & 0xff);
    uint32_t mant = bits & 0x007fffffu;
    if (exp == 255) return (uint16_t)(mant == 0 ? sign | 0x7c00u : sign | 0x7e00u);
    int32_t unbiased = exp - 127;
    if (unbiased > 15) return (uint16_t)(sign | 0x7c00u);
    if (unbiased >= -14) {
        uint32_t m = mant >> 13;
        uint32_t rem = mant & 0x1fffu;
        if (rem > 0x1000u || (rem == 0x1000u && (m & 1u) == 1u)) m += 1;
        uint32_t e = (uint32_t)(unbiased + 15);
        if (m == 0x800u) { m = 0; e += 1; }
        if (e >= 31) return (uint16_t)(sign | 0x7c00u);
        return (uint16_t)(sign | (e << 10) | m);
    }
    return sign;
}

static double f16_val(uint16_t h) {  // full decoder: inf/nan/subnormals
    int32_t exp = (int32_t)((h >> 10) & 0x1f);
    double mant = (double)(h & 0x3ffu) / 1024.0;
    if (exp == 0) {
        double v = mant * ldexp(1.0, -14);
        return (h & 0x8000u) ? -v : v;
    }
    if (exp == 31) {
        if (h & 0x3ffu) return NAN;
        return (h & 0x8000u) ? -INFINITY : INFINITY;
    }
    double v = (mant + 1.0) * ldexp(1.0, exp - 15);
    return (h & 0x8000u) ? -v : v;
}

// Row-sum of the seeded A (every element of A·B with B=ones), in double —
// the expected per-mma increment. The 0.1 pattern is binary-inexact, so
// every mma rounds: hoisting C = A·B or promoting the recurrence would
// change the bits — the chain is NOT legally foldable (that's the point).
static double rowsum_a(const uint16_t* a) {
    // A·B with B=ones: element [m][n] = rowsum of A row m — all rows share
    // the same multiset of values under (r+c)%5? No: row r has (r+c)%5 —
    // per-row sums differ. The store we check is wg 0, tile row 0 → A row 0.
    double s = 0.0;
    for (int k = 0; k < 16; k++) s += f16_val(a[k]);
    return s;
}

// value check: f32acc accumulates with one rounding per mma — the honest
// result sits within a few ulps of the double reference (relative 5e-3 is
// generous); a FOLDED kernel would land on the analytic value, which is
// distinguishable only by timing (a fold shows > hardware peak). The
// f16acc variant overflows to +inf past f16's 65504 (mma #4094 ≈
// iteration 512) — inf is the CORRECT result there (no NaN path).
// `exact` is derived from the observed value class, not the variant.
static int check_y(const uint16_t* y, uint64_t bound, int exact, double expect) {
    for (uint64_t j = 0; j < Y_COUNT - 1; j++) {
        double v = f16_val(y[j]);
        if (isnan(v)) return 0;
        if (exact) {
            double rel = expect != 0.0 ? fabs(v - expect) / fabs(expect) : fabs(v);
            if (rel > 2e-2) return 0;  // f16 storage rounding at |v|~4e3: ulp=2
        } else if (!(v > 0.0)) {
            return 0;  // f16acc: +values or +inf, never 0/negative
        }
    }
    return 1;
}

int main(int argc, char** argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <kernel.spv> [bound] [launches] [warmup] [kernelB.spv]\n", argv[0]);
        return 2;
    }
    const uint64_t bound = argc > 2 ? strtoull(argv[2], NULL, 10) : 10000;
    const int launches = argc > 3 ? atoi(argv[3]) : 6;
    const int warmup = argc > 4 ? atoi(argv[4]) : 2;
    const char* spv2_path = argc > 5 ? argv[5] : NULL;
    int ab_mode = spv2_path != NULL;

    long len_a = 0, len_b = 0;
    uint8_t* spv_a = read_spv(argv[1], &len_a);
    uint8_t* spv_b = ab_mode ? read_spv(spv2_path, &len_b) : NULL;
    if (spv_a == NULL || (ab_mode && spv_b == NULL)) return 2;

    const uint64_t state_bytes = OFF_Y + Y_COUNT * 4 + 64;
    unsigned char* stateA = calloc(1, state_bytes);
    unsigned char* stateB = ab_mode ? calloc(1, state_bytes) : NULL;
    if (stateA == NULL || (ab_mode && stateB == NULL)) { fprintf(stderr, "oom\n"); return 2; }

    BrievField fields[] = {
        { "a", 1, OFF_A, 2, A_COUNT, 1, OFF_A },
        { "i", 2, OFF_I, 8, 1, 0, OFF_I },
        { "y", 1, OFF_Y, 2, Y_COUNT, 1, OFF_Y },
    };
    BrievKernelDesc descs[2] = {
        { "mma", spv_a, (uint32_t)len_a, 3, fields },
        { "mma", spv_b, (uint32_t)len_b, 3, fields },
    };
    uint32_t n_kernels = ab_mode ? 2 : 1;
    // One launch = WGS workgroups x 32 lanes (the microkernel's LocalSize).
    const uint64_t dispatch_n = (uint64_t)WGS * 32;
    // FLOP per launch: WGS * bound mma-iterations * DEPTH mmas * 8192 flops.
    const double flop_per_launch =
        (double)WGS * (double)bound * (double)DEPTH * 8192.0;

    // Seed the runtime A source (BOTH states — A/B mode seeds kernel B's
    // projection from stateB): 0.1-pattern halves, binary-inexact — the
    // fold blocker (see the generator comment in gemm.rs).
    uint16_t* a_seed = (uint16_t*)(stateA + OFF_A);
    for (uint64_t j = 0; j < A_COUNT; j++) {
        a_seed[j] = f32_to_f16(0.1f + (float)(j % 5) * 0.1f);
    }
    if (ab_mode) memcpy(stateB + OFF_A, stateA + OFF_A, A_COUNT * 2);
    // marker: if this survives the download, the download's dev→staging
    // copy never happened (the readback is the stale staging window).
    ((uint16_t*)(stateA + OFF_Y))[0] = f32_to_f16(777.0f);
    // Expected acc[0][0]: per iteration kt, the A fragment row 0 =
    // a[kt*16 + k] (the row base moves with kt), B[k][0] = a[16+k*4096]
    // (B is the SAME loaded fragment every iteration). Each chain gets
    // DEPTH/CHAINS mmas per iteration; acc[0][0] = (DEPTH/8) * sum_kt
    // sum_k A[0][k]*B[k][0]  (f16acc additionally rounds per mma and
    // overflows to inf — the value class check covers that).
    double r_sum = 0.0;
    for (uint64_t kt = 0; kt < bound; kt++) {
        for (uint64_t k = 0; k < 16; k++) {
            r_sum += f16_val(a_seed[kt * 16 + k]) * f16_val(a_seed[16 + k * 4096]);
        }
    }
    const double expect = (double)(DEPTH / 8) * r_sum;

    if (!briev_accel_init(descs, n_kernels)) {
        fprintf(stderr, "no GPU device\n");
        return 1;
    }
    printf("# fingerprint: device=%s spv=%ldB%s bound=%llu launches=%d\n",
           briev_accel_device_name(), len_a,
           ab_mode ? " MODE=ab" : " (single)",
           (unsigned long long)bound, launches);

    // Warm-up (resident seeding happens on the first launch per kernel).
    for (int w = 0; w < warmup; w++) {
        *(int64_t*)(stateA + OFF_I) = (int64_t)bound;
        if (!briev_accel_launch_resident(0, stateA, dispatch_n)) {
            fprintf(stderr, "dispatch failed (A)\n");
            return 1;
        }
        if (ab_mode) {
            *(int64_t*)(stateB + OFF_I) = (int64_t)bound;
            if (!briev_accel_launch_resident(1, stateB, dispatch_n)) {
                fprintf(stderr, "dispatch failed (B)\n");
                return 1;
            }
        }
    }

    // Correctness on the warm-up state: i must equal the bound (the loop
    // ran to completion) and y must hold the accumulated value class.
    if (!briev_accel_download(0, stateA)) { fprintf(stderr, "download failed (A)\n"); return 1; }
    {
        const uint16_t* y = (const uint16_t*)(stateA + OFF_Y);
        int64_t it = *(int64_t*)(stateA + OFF_I);
        int ok = it == (int64_t)bound && check_y(y, bound, isfinite(f16_val(y[0])), expect);
        {
            {
                const uint16_t* yh = (const uint16_t*)(stateA + OFF_Y);
                printf("# y[0..8]:");
                for (int q = 0; q < 8; q++) printf(" %.3g", f16_val(yh[q]));
                printf("  y[4096]=%.3g y[65536]=%.3g\n", f16_val(yh[4096]), f16_val(yh[65536]));
                // scan the WHOLE downloaded projection for any 1.0 half
                const uint16_t* all = (const uint16_t*)stateA;
                size_t n_all = state_bytes / 2; long hits = 0; size_t first = 0;
                for (size_t q = 0; q < n_all; q++) {
                    if (all[q] == 0x3C00) { if (hits == 0) first = q; hits++; }
                }
                printf("# ones-scan: %ld hits, first at half-index %zu (proj byte %zu)\n",
                       hits, first, first * 2);
            }
            const uint16_t* aback = (const uint16_t*)(stateA + OFF_A);
            printf("# a-seed after download: a[0]=%.5f a[1]=%.5f a[5]=%.5f (expect 0.1/0.6/0.1)\n",
                   f16_val(aback[0]), f16_val(aback[1]), f16_val(aback[5]));
        }
        printf("# correctness[A]: i=%lld y[0]=%.1f expect=%.1f (%s)\n", (long long)it,
               f16_val(y[0]), expect, ok ? "OK" : "FAIL");
    }
    if (ab_mode) {
        if (!briev_accel_download(1, stateB)) { fprintf(stderr, "download failed (B)\n"); return 1; }
        const uint16_t* y = (const uint16_t*)(stateB + OFF_Y);
        int64_t it = *(int64_t*)(stateB + OFF_I);
        int ok = it == (int64_t)bound && check_y(y, bound, isfinite(f16_val(y[0])), expect);
        printf("# correctness[B]: i=%lld y[0]=%.1f expect=%.1f (%s)\n", (long long)it,
               f16_val(y[0]), expect, ok ? "OK" : "FAIL");
    }

    if (ab_mode) {
        double tot_a = 0.0, tot_b = 0.0;
        for (int r = 0; r < AB_ROUNDS; r++) {
            uint32_t k0 = (r & 1) == 0 ? 0 : 1;
            uint32_t k1 = 1 - k0;
            unsigned char* st0 = k0 == 0 ? stateA : stateB;
            unsigned char* st1 = k1 == 0 ? stateA : stateB;
            *(int64_t*)(st0 + OFF_I) = (int64_t)bound;
            double t0 = now_ms();
            if (!briev_accel_launch_resident_batch(k0, st0, dispatch_n, 1, launches)) {
                fprintf(stderr, "batch failed (%c)\n", k0 == 0 ? 'A' : 'B');
                return 1;
            }
            double dt0 = now_ms() - t0;
            *(int64_t*)(st1 + OFF_I) = (int64_t)bound;
            double t1 = now_ms();
            if (!briev_accel_launch_resident_batch(k1, st1, dispatch_n, 1, launches)) {
                fprintf(stderr, "batch failed (%c)\n", k1 == 0 ? 'A' : 'B');
                return 1;
            }
            double dt1 = now_ms() - t1;
            double dt_a = k0 == 0 ? dt0 : dt1;
            double dt_b = k0 == 0 ? dt1 : dt0;
            tot_a += dt_a;
            tot_b += dt_b;
            printf("GPU  round %d (%c first)  A %.1f ms/launch (%.2f TF/s)  B %.1f ms/launch (%.2f TF/s)  ratio %.3f\n",
                   r + 1, k0 == 0 ? 'A' : 'B',
                   dt_a / launches, flop_per_launch / (dt_a / launches / 1e3) / 1e12,
                   dt_b / launches, flop_per_launch / (dt_b / launches / 1e3) / 1e12,
                   dt_a / dt_b);
        }
        double per_a = tot_a / (AB_ROUNDS * launches);
        double per_b = tot_b / (AB_ROUNDS * launches);
        printf("GPU  A avg %.1f ms/launch (%.2f TF/s)  B avg %.1f ms/launch (%.2f TF/s)  avg-ratio %.3f\n",
               per_a, flop_per_launch / (per_a / 1e3) / 1e12,
               per_b, flop_per_launch / (per_b / 1e3) / 1e12,
               tot_a / tot_b);
    } else {
        double sum = 0.0;
        for (int it = 0; it < launches; it++) {
            *(int64_t*)(stateA + OFF_I) = (int64_t)bound;
            double t0 = now_ms();
            if (!briev_accel_launch_resident_batch(0, stateA, dispatch_n, 1, 1)) {
                fprintf(stderr, "dispatch failed\n");
                return 1;
            }
            sum += now_ms() - t0;
        }
        double per = sum / launches;
        printf("GPU  %.1f ms/launch  %.2f TF/s  (x%d)\n",
               per, flop_per_launch / (per / 1e3) / 1e12, launches);
    }

    briev_accel_shutdown();
    free(stateA);
    free(stateB);
    free(spv_a);
    free(spv_b);
    return 0;
}
