// E8a step-1 instrument: run the 2-warp bar.arrive toy and print y[0..1].
// Expected y[0]=42, y[1]=43 if producer-arrive / consumer-sync semantics
// hold on sm_86 + driver 580.178.04. Hangs if the barrier never releases.
#include <cuda.h>
#include <stdio.h>
#include <stdlib.h>

static void die(const char *what, CUresult rc) {
    const char *s = 0;
    cuGetErrorString(rc, &s);
    fprintf(stderr, "%s failed: %s (%d)\n", what, s ? s : "?", rc);
    exit(1);
}

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: %s toy.cubin\n", argv[0]); return 1; }
    CUresult rc = cuInit(0);
    if (rc != CUDA_SUCCESS) die("cuInit", rc);
    CUdevice dev;
    rc = cuDeviceGet(&dev, 0);
    if (rc != CUDA_SUCCESS) die("cuDeviceGet", rc);
    CUcontext ctx;
    rc = cuCtxCreate(&ctx, 0, 0, dev);
    if (rc != CUDA_SUCCESS) die("cuCtxCreate", rc);
    CUmodule mod;
    FILE *f = fopen(argv[1], "rb");
    if (!f) { fprintf(stderr, "cannot open %s\n", argv[1]); return 1; }
    fseek(f, 0, SEEK_END);
    long len = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *img = malloc(len);
    if (fread(img, 1, len, f) != (size_t)len) { fprintf(stderr, "short read\n"); return 1; }
    fclose(f);
    rc = cuModuleLoadData(&mod, img);
    if (rc != CUDA_SUCCESS) die("cuModuleLoadData", rc);
    CUfunction fn;
    rc = cuModuleGetFunction(&fn, mod, "main");
    if (rc != CUDA_SUCCESS) die("cuModuleGetFunction", rc);

    CUdeviceptr y = 0;
    rc = cuMemAlloc(&y, 64);
    if (rc != CUDA_SUCCESS) die("cuMemAlloc", rc);
    rc = cuMemsetD32(y, 0, 16);
    if (rc != CUDA_SUCCESS) die("cuMemset", rc);

    void *params[] = { &y };
    rc = cuLaunchKernel(fn, 1, 1, 1, 64, 1, 1, 0, 0, params, 0);
    if (rc != CUDA_SUCCESS) die("cuLaunchKernel", rc);
    rc = cuCtxSynchronize();
    if (rc != CUDA_SUCCESS) die("cuCtxSynchronize", rc);

    unsigned int host[2] = {0, 0};
    rc = cuMemcpyDtoH(host, y, sizeof host);
    if (rc != CUDA_SUCCESS) die("cuMemcpyDtoH", rc);
    printf("y[0]=%u y[1]=%u — %s\n", host[0], host[1],
           (host[0] == 42 && host[1] == 43) ? "BAR.ARRIVE OK" : "WRONG VALUES");
    return (host[0] == 42 && host[1] == 43) ? 0 : 2;
}
