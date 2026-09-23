// ray_texel_check.c — the image-path gate (plan 2026-09-02-image-and-
// dehashtag, step 5). The ray_texel kernel renders the SAME scene as
// ray_bench's f64 reference and writes Rec.601 luminance as R32Float
// texels through the device image path. Gate: max channel diff at 1e-3
// (f32 device vs f64 host). Reports Mrays/s and the image-write
// bandwidth. Build + run like image_check.c.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>
#include <time.h>
#define BRIEV_IMAGE_FORMAT_R32F 1u
#include "briev_accel_rt.h"

#define W 1920
#define H 1080
#define N ((uint64_t)W * H)
#define WARMUP 2
#define ITERS 10

static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e3 + (double)ts.tv_nsec / 1e6;
}

static double fmax0(double v) { return (v + fabs(v)) * 0.5; }

static void render_reference_lum(float* lum) {
    for (uint64_t i = 0; i < N; i++) {
        int pix = (int)(i % W);
        int py = (int)(i / W);
        double u = (double)pix / 1919.0;
        double v = (double)py / 1079.0;
        double sx = (u * 2.0 - 1.0) * 1.7778;
        double sy = 1.0 - v * 2.0;
        double inv = 1.0 / sqrt(sx * sx + sy * sy + 1.0);
        double dx = sx * inv, dy = sy * inv, dz = inv;
        const double big = 1.0e30;
        double ax = 0.6, ay = -0.1, az = -5.0;
        double abh = ax * dx + ay * dy + az * dz;
        double acc = ax * ax + ay * ay + az * az - 0.49;
        double adisc = abh * abh - acc;
        double at = adisc >= 0.0 ? -abh - sqrt(adisc) : big;
        double bx = -0.7, by = 0.2, bz = -5.6;
        double bbh = bx * dx + by * dy + bz * dz;
        double bcc = bx * bx + by * by + bz * bz - 0.25;
        double bdisc = bbh * bbh - bcc;
        double bt = bdisc >= 0.0 ? -bbh - sqrt(bdisc) : big;
        double cx = -0.1, cy = -0.75, cz = -6.4;
        double cbh = cx * dx + cy * dy + cz * dz;
        double ccc = cx * cx + cy * cy + cz * cz - 0.16;
        double cdisc = cbh * cbh - ccc;
        double ct = cdisc >= 0.0 ? -cbh - sqrt(cdisc) : big;
        double pt = dy < 0.0 ? -1.0 / dy : big;
        double tmin = fmin(fmin(fmin(at, bt), ct), pt);
        int hitA = (at < big * 0.5) && (at <= tmin);
        int hitB = (bt < big * 0.5) && (bt <= tmin);
        int hitC = (ct < big * 0.5) && (ct <= tmin);
        int hitP = (pt < big * 0.5) && (pt <= tmin);
        double r, g, b;
        if (hitA) {
            double nx = (at * dx + 0.6) / 0.7;
            double ny = (at * dy - 0.1) / 0.7;
            double nz = (at * dz - 5.0) / 0.7;
            double lam = fmax0(nx * 0.4969 + ny * 0.7950 - nz * 0.3479);
            r = 0.9 * lam; g = 0.25 * lam; b = 0.2 * lam;
        } else if (hitB) {
            double nx = (bt * dx - 0.7) / 0.5;
            double ny = (bt * dy + 0.2) / 0.5;
            double nz = (bt * dz - 5.6) / 0.5;
            double lam = fmax0(nx * 0.4969 + ny * 0.7950 - nz * 0.3479);
            r = 0.2 * lam; g = 0.4 * lam; b = 0.9 * lam;
        } else if (hitC) {
            double nx = (ct * dx - 0.1) / 0.4;
            double ny = (ct * dy - 0.75) / 0.4;
            double nz = (ct * dz - 6.4) / 0.4;
            double lam = fmax0(nx * 0.4969 + ny * 0.7950 - nz * 0.3479);
            r = 0.9 * lam; g = 0.75 * lam; b = 0.15 * lam;
        } else if (hitP) {
            r = 0.35 * 0.795; g = 0.35 * 0.795; b = 0.4 * 0.795;
        } else {
            double f = dy * 0.5 + 0.5;
            r = 0.8 - 0.45 * f; g = 0.85 - 0.30 * f; b = 0.9 - 0.05 * f;
        }
        lum[i] = (float)(0.299 * r + 0.587 * g + 0.114 * b);
    }
}

int main(int argc, char** argv) {
    if (argc < 2) { fprintf(stderr, "usage: %s <kernel.spv> [iters]\n", argv[0]); return 2; }
    const int iters = argc > 2 ? atoi(argv[2]) : ITERS;

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

    // Host layout: i @ 0 (8B), lum @ 8 (N*4) — the image array sits in the
    // host state (the SSBO projection holds only i).
    uint64_t off_i = 0;
    uint64_t off_lum = 8;
    unsigned char* state = calloc(1, off_lum + N * 4 + 64);
    if (state == NULL) { fprintf(stderr, "oom\n"); return 2; }
    BrievField fields[] = {
        { "i", 2, off_i, 8, 1, 0, 0 },
    };
    BrievImageDesc images[] = { { "lum", off_lum, W, H, BRIEV_IMAGE_FORMAT_R32F } };
    BrievKernelDesc desc = { "render_lum", spv, (uint32_t)spv_len, 1, fields, 1, images };
    float* lum = (float*)(state + off_lum);

    if (!briev_accel_init(&desc, 1)) {
        fprintf(stderr, "no GPU device\n"); return 1;
    }
    printf("# fingerprint: device=%s spv=%ldB %dx%d=%llu texels warmup=%d iters=%d\n",
           briev_accel_device_name(), spv_len, W, H, (unsigned long long)N, WARMUP, iters);

    for (int w = 0; w < WARMUP; w++) {
        *(int64_t*)(state + off_i) = 0;
        if (!briev_accel_launch_resident(0, state, N)) {
            fprintf(stderr, "dispatch failed\n"); return 1;
        }
    }
    if (!briev_accel_download(0, state)) {
        fprintf(stderr, "download failed\n"); return 1;
    }

    float* ref = malloc((size_t)N * 4);
    if (ref == NULL) { fprintf(stderr, "oom\n"); return 2; }
    double t1 = now_ms();
    render_reference_lum(ref);
    double cpu_ms = now_ms() - t1;

    double max_abs = 0.0;
    for (uint64_t j = 0; j < N; j++) {
        double d = fabs((double)lum[j] - (double)ref[j]);
        if (d > max_abs) max_abs = d;
    }
    printf("# correctness: max_lum_err = %.3e (%s)\n", max_abs,
           max_abs <= 1e-3 ? "OK" : "FAIL");

    double bytes = (double)N * 4.0;
    double sum = 0, mn = 1e30, mx = 0;
    for (int it = 0; it < iters; it++) {
        *(int64_t*)(state + off_i) = 0;
        double t = now_ms();
        int ok = briev_accel_launch_resident(0, state, N);
        double dt = now_ms() - t;
        if (!ok) { fprintf(stderr, "dispatch failed\n"); return 1; }
        sum += dt; if (dt < mn) mn = dt; if (dt > mx) mx = dt;
    }
    double avg = sum / iters;
    printf("CPU  %.1f ms/frame (1 thread)\n", cpu_ms);
    printf("GPU  avg %.3f ms  min %.3f ms  max %.3f ms  %.1f Mrays/s  %.1f GB/s texel-write  (%.0fx CPU)\n",
           avg, mn, mx, (double)N / (avg * 1e3), bytes / (avg * 1e6), cpu_ms / avg);

    briev_accel_shutdown();
    return max_abs <= 1e-3 ? 0 : 1;
}
