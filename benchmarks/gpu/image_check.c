// image_check.c — device verification for the image storage path (plan
// 2026-09-02-image-and-dehashtag, revised). Builds the texel_img kernel's
// descriptor (SSBO fields i/x + the image-resident array), runs one pass,
// and gates the image readback against x+1. Exit 0 = pass.
// Build: cc -O2 -I<out-dir> -o image_check image_check.c -lm -ldl -lpthread
//   (the <out-dir> holds the runtime copies brievc build places beside the
//   runner — the same single-TU include the runner uses).
#define BRIEV_IMAGE_FORMAT_R32F 1u
#include "briev_accel_rt.h"
int main(int argc, char** argv) {
    FILE* f = fopen(argv[1], "rb");
    fseek(f, 0, SEEK_END); long spv_len = ftell(f); fseek(f, 0, SEEK_SET);
    uint8_t* spv = malloc((size_t)spv_len); fread(spv, 1, (size_t)spv_len, f); fclose(f);
    const uint64_t N = 65536;
    // Name-sorted layout: i@0 (8B), img@8 (N*4), x@262152 (N*4) — from the runner.
    uint64_t off_i = 0, off_img = 8, off_x = 262152;
    unsigned char* state = calloc(1, off_x + N*4 + 64);
    BrievField fields[] = {
        { "i", 2, off_i, 8, 1, 0, 0 },
        { "x", 1, off_x, 4, N, 1, 16 },
    };
    BrievImageDesc images[] = { { "img", off_img, 256, 256, BRIEV_IMAGE_FORMAT_R32F } };
    BrievKernelDesc desc = { "fill", spv, (uint32_t)spv_len, 2, fields, 1, images };
    float* x = (float*)(state + off_x);
    float* img = (float*)(state + off_img);
    for (uint64_t j = 0; j < N; j++) x[j] = (float)j * 0.5f;
    if (!briev_accel_init(&desc, 1)) { fprintf(stderr, "init failed\n"); return 1; }
    if (!briev_accel_launch_resident(0, state, N)) {
        fprintf(stderr, "LAUNCH FAILED\n");
        return 1;
    }
    if (!briev_accel_download(0, state)) {
        fprintf(stderr, "DOWNLOAD RETURNED 0\n");
    }
    double max_err = 0; uint64_t bad = 0;
    for (uint64_t j = 0; j < N; j++) {
        float ref = x[j] + 1.0f;
        double d = fabs((double)img[j] - (double)ref);
        if (d > max_err) max_err = d;
        if (d > 1e-3) bad++;
    }
    printf("image path: %llu/%llu wrong, max_err=%.3e, img[0]=%.1f img[N-1]=%.1f (%s)\n",
           (unsigned long long)bad, (unsigned long long)N, max_err,
           img[0], img[N-1], bad == 0 ? "OK" : "FAIL");
    briev_accel_shutdown();
    return bad == 0 ? 0 : 1;
}
