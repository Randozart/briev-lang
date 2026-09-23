// ray_bench.c — deterministic raytracer benchmark (plan 2026-09-02-
// graphics-ray-and-images, Milestone A). The GPU kernel renders the same
// scene as the f64 CPU reference below (ONE pixel per work item, three
// spheres + ground plane, one directional light, sky gradient). Gate:
// per-channel max abs diff (f32 device vs f64 host — same math, different
// width). Reports GPU seconds-per-frame and Mrays/s (primary rays), plus
// the single-thread CPU reference for the score. Writes ray.ppm.
//
// Field table mirrors the generated runner: i @ 0 (8B), px @ 8 (3WH*4).
// Proj: i 0, px 16 (16B-aligned).
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>
#include <time.h>
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

// ── The shared scene (constants match ray.abv exactly) ──────────────────
// Camera o = (0, 0, -3), image plane z = +1 in ray space.
// Light L = normalize(0.5, 0.8, -0.35) = (0.4969, 0.7950, -0.3479).
// Sphere A c=(-0.6, 0.1, 2.0) r=0.7 col=(0.9, 0.25, 0.2)
// Sphere B c=( 0.7,-0.2, 2.6) r=0.5 col=(0.2, 0.4, 0.9)
// Sphere C c=( 0.1, 0.75,3.4) r=0.4 col=(0.9, 0.75, 0.15)
// Plane y=-1 col=(0.35, 0.35, 0.4), Lambert lam = Ly = 0.7950.
// Sky: horizon (0.8, 0.85, 0.9) → zenith (0.35, 0.55, 0.85) by dy.

static double fmax0(double v) { return (v + fabs(v)) * 0.5; }

static void render_reference(float* px) {
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

        // Sphere A
        double ax = 0.6, ay = -0.1, az = -5.0;
        double abh = ax * dx + ay * dy + az * dz;
        double acc = ax * ax + ay * ay + az * az - 0.49;
        double adisc = abh * abh - acc;
        double at = adisc >= 0.0 ? -abh - sqrt(adisc) : big;
        // Sphere B
        double bx = -0.7, by = 0.2, bz = -5.6;
        double bbh = bx * dx + by * dy + bz * dz;
        double bcc = bx * bx + by * by + bz * bz - 0.25;
        double bdisc = bbh * bbh - bcc;
        double bt = bdisc >= 0.0 ? -bbh - sqrt(bdisc) : big;
        // Sphere C
        double cx = -0.1, cy = -0.75, cz = -6.4;
        double cbh = cx * dx + cy * dy + cz * dz;
        double ccc = cx * cx + cy * cy + cz * cz - 0.16;
        double cdisc = cbh * cbh - ccc;
        double ct = cdisc >= 0.0 ? -cbh - sqrt(cdisc) : big;
        // Plane
        double pt = dy < 0.0 ? -1.0 / dy : big;

        double tmin = fmin(fmin(fmin(at, bt), ct), pt);
        // Priority chain mirrors the kernel exactly: a `big` sentinel (no
        // hit on that branch) never qualifies, including the all-miss
        // case where at == tmin == big.
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
        px[i * 3 + 0] = (float)r;
        px[i * 3 + 1] = (float)g;
        px[i * 3 + 2] = (float)b;
    }
}

static void write_ppm(const char* path, const float* px) {
    FILE* f = fopen(path, "wb");
    if (f == NULL) { perror("ppm"); return; }
    fprintf(f, "P6\n%d %d\n255\n", W, H);
    for (uint64_t j = 0; j < N * 3; j++) {
        int c = (int)(px[j] * 255.0f + 0.5f);
        if (c < 0) c = 0;
        if (c > 255) c = 255;
        fputc(c, f);
    }
    fclose(f);
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

    // Field table mirrors the generated runner (i, px).
    uint64_t off_i = 0;
    uint64_t off_px = 8;
    uint64_t proj_px = 16;
    uint64_t state_bytes = off_px + N * 3 * 4 + 64;
    unsigned char* state = calloc(1, state_bytes);
    if (state == NULL) { fprintf(stderr, "oom\n"); return 2; }

    BrievField fields[] = {
        { "i", 2, off_i, 8, 1, 0, 0 },
        { "px", 1, off_px, 4, N * 3, 1, proj_px },
    };
    BrievKernelDesc desc = { "render", spv, (uint32_t)spv_len, 2, fields };

    float* pxc = (float*)(state + off_px);

    if (!briev_accel_init(&desc, 1)) {
        fprintf(stderr, "no GPU device\n"); return 1;
    }
    printf("# fingerprint: device=%s spv=%ldB %dx%d=%llu rays warmup=%d iters=%d\n",
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

    // Correctness gate: CPU f64 reference re-render.
    float* ref = malloc((size_t)(N * 3) * 4);
    if (ref == NULL) { fprintf(stderr, "oom\n"); return 2; }
    double t1 = now_ms();
    render_reference(ref);
    double cpu_ms = now_ms() - t1;

    double max_abs = 0.0;
    uint64_t worst = 0;
    for (uint64_t j = 0; j < N * 3; j++) {
        double d = fabs((double)pxc[j] - (double)ref[j]);
        if (d > max_abs) { max_abs = d; worst = j; }
    }
    printf("# correctness: max_channel_err = %.3e (%s)\n", max_abs,
           max_abs <= 1e-3 ? "OK" : "FAIL");
    if (max_abs > 1e-3) {
        printf("# worst: pixel (%llu,%llu) ch %llu gpu=%.4f ref=%.4f\n",
               (unsigned long long)((worst / 3) % W),
               (unsigned long long)((worst / 3) / W),
               (unsigned long long)(worst % 3),
               pxc[worst], ref[worst]);
    }
    printf("CPU  %.1f ms/frame (%.1f Mrays/s, 1 thread)\n",
           cpu_ms, (double)N / (cpu_ms * 1e3));

    if (max_abs <= 1e-3) write_ppm("ray.ppm", pxc);

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
    printf("GPU  avg %.3f ms  min %.3f ms  max %.3f ms  %.1f Mrays/s  (%.0fx CPU)\n",
           avg, mn, mx, (double)N / (avg * 1e3), cpu_ms / avg);

    briev_accel_shutdown();
    return max_abs <= 1e-3 ? 0 : 1;
}
