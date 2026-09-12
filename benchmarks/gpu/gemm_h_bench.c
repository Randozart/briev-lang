// gemm_h_bench.c — correctness + perf for the Float16 GEMM lane (plan
// 2026-09-02, resuming 2026-08-31-vitriol-gemm-comparison). The f16 twin of
// gemm_bench.c: y[m*N+n] = sum_k A[m*K+k] * B[k*N+n], f16 storage /
// f32 compute (the kernel widens at the SSBO boundary and stores round
// back to f16 — one final rounding).
// Usage:
//   gemm_h_bench <kernel.spv> [M] [N] [K] [iters] [dispatch] [batch] [warmup]
//   gemm_h_bench <kernelA.spv> [M] [N] [K] [iters] [dispatch] 1 [warmup] <kernelB.spv>
// The two-kernel form is the in-process A/B (2026-09-04-gemm-perf-blocks):
// per round, one batched submission per kernel back-to-back — both kernels
// share the same DVFS window, so between-round clock skew (8ms boost vs
// 21ms throttle windows, 2026-09-04 A/B) cancels launch-by-launch.
// State layout mirrors the generated gemm_h_runner.c exactly
// (name-sorted: a, b, i, y; f16 arrays 2B/elem, proj == host offsets):
//   a @ 0          (M*K * 2 bytes)
//   b @ a_end      (K*N * 2)
//   i @ b_end (8-aligned) (8 bytes)
//   y @ i_end + 8  (M*N * 2)
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>
#include <time.h>
#include "briev_accel_rt.c"

#define DEFAULT_WARMUP 5
#define AB_ROUNDS 5
#define VERIFY_ROWS 16

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e3 + (double)ts.tv_nsec / 1e6;
}

// IEEE-754 binary16 encode, round-to-nearest-even — mirrors the backend's
// f32_to_f16_hex (plan fundamental-parent-membership). Seeds must be f16
// EXACT so the reference is exact.
static uint16_t f32_to_f16(float v) {
    uint32_t bits;
    memcpy(&bits, &v, 4);
    uint16_t sign = (uint16_t)((bits >> 16) & 0x8000u);
    int32_t exp = (int32_t)((bits >> 23) & 0xff);
    uint32_t mant = bits & 0x007fffffu;
    if (exp == 255) {
        return (uint16_t)(mant == 0 ? sign | 0x7c00u : sign | 0x7e00u | (uint16_t)((mant >> 13) & 0x3ff));
    }
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
    if (unbiased >= -25) {
        uint32_t combined = 0x00800000u | mant;
        uint32_t d = (uint32_t)(-(unbiased + 1));
        uint32_t f10 = combined >> d;
        uint32_t rem = combined & ((1u << d) - 1u);
        uint32_t half = 1u << (d - 1);
        if (rem > half || (rem == half && (f10 & 1u) == 1u)) f10 += 1;
        if (f10 >= 0x400u) return (uint16_t)(sign | (1u << 10));
        return (uint16_t)(sign | f10);
    }
    return sign;
}

// f16 decode (exact — for the reference comparison).
static double f16_to_f64(uint16_t h) {
    uint32_t sign = (uint32_t)(h & 0x8000u) << 16;
    uint32_t exp = (h >> 10) & 0x1fu;
    uint32_t mant = h & 0x3ffu;
    uint32_t bits;
    if (exp == 0) {
        if (mant == 0) { bits = sign; }
        else {
            int32_t e = -1;
            uint32_t m = mant;
            while ((m & 0x400u) == 0) { m <<= 1; e -= 1; }
            m &= 0x3ffu;
            bits = sign | (uint32_t)((127 - 15 + e + 1 + 10) << 23) | (m << 13);
        }
    } else if (exp == 31) {
        bits = sign | 0x7f800000u | (mant << 13);
    } else {
        bits = sign | ((exp - 15 + 127) << 23) | (mant << 13);
    }
    float f;
    memcpy(&f, &bits, 4);
    return (double)f;
}

// f16-exact seeds: 0.25/0.5 multiples of small ints — the reference is
// exact and the kernel's single store-rounding is the only error.
static void seed_state(unsigned char* state, uint64_t off_a, uint64_t off_b,
                       uint64_t M, uint64_t K, uint64_t N) {
    uint16_t* a = (uint16_t*)(state + off_a);
    uint16_t* b = (uint16_t*)(state + off_b);
    for (uint64_t j = 0; j < M * K; j++) a[j] = f32_to_f16((float)((int)(j % 7)) * 0.25f);
    for (uint64_t j = 0; j < K * N; j++) b[j] = f32_to_f16((float)((int)(j % 5)) * 0.5f);
}

// Sampled correctness against a double reference. f16 storage rounds
// once per output — bound the relative error at ~2^-10 (f16 epsilon)
// plus slack for the ~K-term f32 accumulation.
static double max_rel_err(const unsigned char* state, uint64_t off_a,
                          uint64_t off_b, uint64_t off_y,
                          uint64_t M, uint64_t N, uint64_t K);

// The f16acc gate: the plan's per-tier contract (5e-3 f32-acc, 1e-2
// f16acc). The old hardcoded 5e-3 silently over-tightened the f16acc
// tier (the f16 accumulation walk is order-sensitive and reaches
// ~6.4e-3 under the D5 panel rotation).
static double rel_gate(void) {
    return getenv("BRIEV_GEMM_F16ACC") != NULL ? 1e-2 : 5e-3;
}

static void report_correctness(const char* tag, const unsigned char* st,
                               uint64_t off_a, uint64_t off_b, uint64_t off_y,
                               uint64_t M, uint64_t N, uint64_t K) {
    double max_rel = max_rel_err(st, off_a, off_b, off_y, M, N, K);
    // sample dump: the first got vs ref at row 0
    const uint16_t* a = (const uint16_t*)(st + off_a);
    const uint16_t* b = (const uint16_t*)(st + off_b);
    const uint16_t* y = (const uint16_t*)(st + off_y);
    double ref0 = 0.0;
    for (uint64_t k = 0; k < K; k++) ref0 += f16_to_f64(a[k]) * f16_to_f64(b[k * N + 0]);
    printf("# sample[%s]: y[0]=%.4f ref=%.4f (f16 grid %.4f)\n", tag,
           f16_to_f64(y[0]), ref0, (double)f32_to_f16((float)ref0));
    printf("# correctness[%s]: max_rel_err = %.3e (%s)\n", tag, max_rel,
           max_rel <= rel_gate() ? "OK" : "FAIL");
}

// Sampled correctness against a double reference. f16 storage rounds
// once per output — bound the relative error at ~2^-10 (f16 epsilon)
// plus slack for the ~K-term f32 accumulation.
static double max_rel_err(const unsigned char* state, uint64_t off_a,
                          uint64_t off_b, uint64_t off_y,
                          uint64_t M, uint64_t N, uint64_t K) {
    const uint16_t* a = (const uint16_t*)(state + off_a);
    const uint16_t* b = (const uint16_t*)(state + off_b);
    const uint16_t* y = (const uint16_t*)(state + off_y);
    double max_rel = 0.0;
    uint64_t rows[VERIFY_ROWS];
    for (int r = 0; r < VERIFY_ROWS; r++) {
        rows[r] = (uint64_t)(r * 7919) % M;
    }
    for (int r = 0; r < VERIFY_ROWS; r++) {
        const uint64_t m = rows[r];
        for (uint64_t n = 0; n < N; n++) {
            double ref = 0.0;
            for (uint64_t k = 0; k < K; k++) {
                ref += f16_to_f64(a[m * K + k]) * f16_to_f64(b[k * N + n]);
            }
            double got = f16_to_f64(y[m * N + n]);
            double rel = ref == 0.0 ? got : fabs(got - ref) / fabs(ref);
            if (rel > max_rel) max_rel = rel;
        }
    }
    return max_rel;
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

int main(int argc, char** argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <kernel.spv> [M] [N] [K] [iters] [dispatch] [batch] [warmup] [kernelB.spv]\n", argv[0]);
        return 2;
    }
    const uint64_t M = argc > 2 ? strtoull(argv[2], NULL, 10) : 4096;
    const uint64_t N = argc > 3 ? strtoull(argv[3], NULL, 10) : 4096;
    const uint64_t K = argc > 4 ? strtoull(argv[4], NULL, 10) : 4096;
    const int iters = argc > 5 ? atoi(argv[5]) : 20;
    // 0 = naive tier (M*N work items). The TENSOR tier's geometry differs
    // — mirror the generated runner: (M*N / (16*R*64)) * 32 work items.
    uint64_t dispatch_override = argc > 6 ? strtoull(argv[6], NULL, 10) : 0;
    // batch=1: the steady-state loop as ONE batched submission (times =
    // iters) — no idle gap between launches, so the GPU never drops to
    // idle clocks mid-measurement (per-launch fence wake ~33us + DVFS
    // ramp otherwise dominate; 2026-09-02 investigation: clocks pulsed
    // 1837/139MHz with GPU_IDLE throttle between fence-waited launches).
    int batch = argc > 7 ? atoi(argv[7]) : 0;
    const int warmup = argc > 8 ? atoi(argv[8]) : DEFAULT_WARMUP;
    // Non-NULL ⇒ in-process A/B: kernel 0 = argv[1], kernel 1 = argv[9].
    // argv[10] = kernel B's dispatch count — REQUIRED when B has different
    // tile geometry (e.g. R=2 needs 2× the workgroups of R=4; sharing A's
    // count under-dispatches B → half the output stays zero, rel = 1.0).
    const char* spv2_path = argc > 9 ? argv[9] : NULL;
    uint64_t dispatch2_override = argc > 10 ? strtoull(argv[10], NULL, 10) : 0;
    int ab_mode = spv2_path != NULL;
    if (ab_mode && !batch) {
        fprintf(stderr, "A/B mode requires batch=1\n");
        return 2;
    }

    long spv_len = 0;
    uint8_t* spv = read_spv(argv[1], &spv_len);
    if (spv == NULL) return 2;
    long spv2_len = 0;
    uint8_t* spv2 = NULL;
    if (ab_mode) {
        spv2 = read_spv(spv2_path, &spv2_len);
        if (spv2 == NULL) return 2;
    }

    uint64_t off_a = 0;
    uint64_t off_b = off_a + M * K * 2;
    uint64_t off_i = (off_b + K * N * 2 + 7) & ~(uint64_t)7;
    uint64_t off_y = off_i + 8;
    uint64_t state_bytes = off_y + M * N * 2 + 64;
    unsigned char* stateA = calloc(1, state_bytes);
    unsigned char* stateB = ab_mode ? calloc(1, state_bytes) : NULL;
    if (stateA == NULL || (ab_mode && stateB == NULL)) {
        fprintf(stderr, "oom\n");
        return 2;
    }

    BrievField fields[] = {
        { "a", 1, off_a, 2, M * K, 1, off_a },
        { "b", 1, off_b, 2, K * N, 1, off_b },
        { "i", 2, off_i, 8, 1, 0, off_i },
        { "y", 1, off_y, 2, M * N, 1, off_y },
    };
    // 2026-09-10 (PTX CUDA tier): argv[11] = dynamic shared bytes for the
    // staged mw kernel (0 = none); block_threads 256 = the (2,4) mw config.
    // Tail-of-struct fields (n_images/images/block_threads/shared_bytes)
    // zero-fill for the Vulkan path.
    const char* mw_smem_env = getenv("MW_SMEM");
    uint32_t mw_smem = mw_smem_env ? (uint32_t)atoi(mw_smem_env) : 0;
    // 2026-09-11: MW_BT must match the kernel's baked thread count — the
    // (2,8)/(4,4) f16acc mw kernels index smem and tiles with 512-thread
    // constants; launching 256 threads of a 512-thread kernel faults on
    // out-of-tile global reads (same contract class as MW_SMEM).
    const char* mw_bt_env = getenv("MW_BT");
    uint32_t mw_bt = mw_bt_env ? (uint32_t)atoi(mw_bt_env) : 256;
    BrievKernelDesc descs[2] = {
        { "gemm", spv, (uint32_t)spv_len, 4, fields, 0, NULL, mw_bt, mw_smem },
        { "gemm", spv2, (uint32_t)spv2_len, 4, fields, 0, NULL, mw_bt, mw_smem },
    };
    uint32_t n_kernels = ab_mode ? 2 : 1;

    seed_state(stateA, off_a, off_b, M, K, N);
    if (ab_mode) seed_state(stateB, off_a, off_b, M, K, N);

    if (!briev_accel_init(descs, n_kernels)) {
        fprintf(stderr, "no GPU device\n");
        return 1;
    }
    printf("# fingerprint: device=%s spv=%ldB%s M=%llu N=%llu K=%llu warmup=%d iters=%d%s\n",
           briev_accel_device_name(), spv_len,
           ab_mode ? "" : " (single)",
           (unsigned long long)M, (unsigned long long)N, (unsigned long long)K,
           warmup, iters,
           ab_mode ? " MODE=ab-alternating-batch" : "");

    // Dispatch: naive = one work item per output element; a nonzero
    // override mirrors the generated runner's tier geometry.
    uint64_t dispatch_n = dispatch_override != 0 ? dispatch_override : M * N;
    uint64_t dispatch_n2 = ab_mode
        ? (dispatch2_override != 0 ? dispatch2_override : dispatch_n)
        : 0;

    // Warm-up: alternating launches keep both kernels' clock state hot.
    uint64_t disp_by_idx[2] = { dispatch_n, dispatch_n2 };
    unsigned char* state_by_idx[2] = { stateA, stateB };
    for (int w = 0; w < warmup; w++) {
        *(int64_t*)(stateA + off_i) = 0;
        if (!briev_accel_launch_resident(0, stateA, dispatch_n)) {
            fprintf(stderr, "dispatch failed (A)\n");
            return 1;
        }
        if (ab_mode) {
            *(int64_t*)(stateB + off_i) = 0;
            if (!briev_accel_launch_resident(1, stateB, dispatch_n2)) {
                fprintf(stderr, "dispatch failed (B)\n");
                return 1;
            }
        }
    }
    if (!briev_accel_download(0, stateA)) {
        fprintf(stderr, "download failed (A)\n");
        return 1;
    }
    report_correctness("A", stateA, off_a, off_b, off_y, M, N, K);
    if (ab_mode) {
        if (!briev_accel_download(1, stateB)) {
            fprintf(stderr, "download failed (B)\n");
            return 1;
        }
        report_correctness("B", stateB, off_a, off_b, off_y, M, N, K);
    }

    if (ab_mode) {
        // Interleaved A/B: per round, one batched submission per kernel,
        // back-to-back — one fence gap per round, shared clock window.
        // ORDER ALTERNATES each round (2026-09-04): a same-SPV self-A/B
        // showed a monotonic positional bias when the clock state is
        // transient (first slot 9.9→13.7ms while second 12.5→8.6ms across
        // rounds) — alternating cancels it in the per-kernel sums.
        double tot_a = 0.0, tot_b = 0.0, min_a = 1e30, min_b = 1e30;
        for (int r = 0; r < AB_ROUNDS; r++) {
            uint32_t k0 = (r & 1) == 0 ? 0 : 1;
            uint32_t k1 = 1 - k0;
            unsigned char* st0 = state_by_idx[k0];
            unsigned char* st1 = state_by_idx[k1];
            *(int64_t*)(st0 + off_i) = 0;
            double t0 = now_ms();
            if (!briev_accel_launch_resident_batch(k0, st0, disp_by_idx[k0], 1, iters)) {
                fprintf(stderr, "batch dispatch failed (%c)\n", k0 == 0 ? 'A' : 'B');
                return 1;
            }
            double dt0 = now_ms() - t0;
            *(int64_t*)(st1 + off_i) = 0;
            double t1 = now_ms();
            if (!briev_accel_launch_resident_batch(k1, st1, disp_by_idx[k1], 1, iters)) {
                fprintf(stderr, "batch dispatch failed (%c)\n", k1 == 0 ? 'A' : 'B');
                return 1;
            }
            double dt1 = now_ms() - t1;
            double dt_a = k0 == 0 ? dt0 : dt1;
            double dt_b = k0 == 0 ? dt1 : dt0;
            tot_a += dt_a;
            tot_b += dt_b;
            if (dt_a < min_a) min_a = dt_a;
            if (dt_b < min_b) min_b = dt_b;
            double per_a = dt_a / iters, per_b = dt_b / iters;
            printf("GPU  round %d (%c first)  A %.3f ms/call (%.0f GF/s)  B %.3f ms/call (%.0f GF/s)  ratio %.3f\n",
                   r + 1, k0 == 0 ? 'A' : 'B',
                   per_a, 2.0 * (double)M * N * K / (per_a * 1e6),
                   per_b, 2.0 * (double)M * N * K / (per_b * 1e6),
                   dt_a / dt_b);
        }
        double per_a = tot_a / (AB_ROUNDS * iters);
        double per_b = tot_b / (AB_ROUNDS * iters);
        printf("GPU  A avg %.3f ms/call (%.0f GF/s, min-round %.3f)  B avg %.3f ms/call (%.0f GF/s, min-round %.3f)  avg-ratio %.3f\n",
               per_a, 2.0 * (double)M * N * K / (per_a * 1e6), min_a / iters,
               per_b, 2.0 * (double)M * N * K / (per_b * 1e6), min_b / iters,
               tot_a / tot_b);
    } else if (batch) {
        *(int64_t*)(stateA + off_i) = 0;
        double t0 = now_ms();
        if (!briev_accel_launch_resident_batch(0, stateA, dispatch_n, 1, iters)) {
            fprintf(stderr, "batch dispatch failed\n");
            return 1;
        }
        double wall = now_ms() - t0;
        double per = wall / iters;
        printf("GPU  per-call %.3f ms  %.2f GFLOP/s  (batched x%d)\n",
               per, 2.0 * (double)M * N * K / (per * 1e6), iters);
    } else {
        double sum = 0, mn = 1e30, mx = 0;
        for (int it = 0; it < iters; it++) {
            *(int64_t*)(stateA + off_i) = 0;
            double t1 = now_ms();
            int ok = briev_accel_launch_resident(0, stateA, dispatch_n);
            double dt = now_ms() - t1;
            if (!ok) { fprintf(stderr, "dispatch failed\n"); return 1; }
            sum += dt; if (dt < mn) mn = dt; if (dt > mx) mx = dt;
        }
        printf("GPU  avg %.3f ms  min %.3f ms  max %.3f ms  %.2f GFLOP/s\n",
               sum / iters, mn, mx, 2.0 * (double)M * N * K / ((sum / iters) * 1e6));
    }

    briev_accel_shutdown();
    free(stateA);
    free(stateB);
    free(spv);
    free(spv2);
    return 0;
}
