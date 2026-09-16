// cublas_attn_bench.c — the vs-cuBLAS composition baseline for the fused
// attention (Milestone C). Runs the SAME composition the fused kernel
// replaces — S = Q·kt, S' = S·0.5, O = S'·V — as separate cuBLAS GEMMs
// plus an elementwise scale. f16 operands, f32 accumulate (CUBLAS_COMPUTE_32F).
// Compiled with nvcc: nvcc -O3 -o cublas_attn_bench cublas_attn_bench.c -lcublas -lcudart
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <cuda_runtime.h>
#include <cuda_fp16.h>
#include <cublas_v2.h>

#define CHECK_CU(x) do { cudaError_t e = (x); if (e != cudaSuccess) { fprintf(stderr, "CUDA error %s\n", cudaGetErrorString(e)); return 1; } } while (0)
#define CHECK_CUB(x) do { cublasStatus_t s = (x); if (s != CUBLAS_STATUS_SUCCESS) { fprintf(stderr, "cuBLAS error %d\n", (int)s); return 1; } } while (0)

static double now_ms(void) {
    struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e3 + (double)ts.tv_nsec / 1e6;
}

// Scale an f16 vector by 0.5 (the elementwise middle of the chain).
__global__ void scale_half(__half* x, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) x[i] = __float2half(__half2float(x[i]) * 0.5f);
}

int main(int argc, char** argv) {
    int M = argc > 1 ? atoi(argv[1]) : 512;
    int N = argc > 2 ? atoi(argv[2]) : M;
    int K = argc > 3 ? atoi(argv[3]) : M;
    int iters = argc > 4 ? atoi(argv[4]) : 20;

    cudaSetDevice(0);
    cublasHandle_t h;
    CHECK_CUB(cublasCreate(&h));

    __half *d_q, *d_kt, *d_v, *d_s, *d_o;
    size_t qb = (size_t)M * K * 2, kb = (size_t)K * N * 2, vb = (size_t)M * N * 2;
    CHECK_CU(cudaMalloc(&d_q, qb));
    CHECK_CU(cudaMalloc(&d_kt, kb));
    CHECK_CU(cudaMalloc(&d_v, (size_t)M * N * 2));
    CHECK_CU(cudaMalloc(&d_s, vb));
    CHECK_CU(cudaMalloc(&d_o, vb));
    // Warm deterministic values.
    __half* hq = (__half*)malloc(qb); for (int i = 0; i < M*K; i++) hq[i] = __float2half((float)((i % 7) * 0.375f));
    __half* hkt = (__half*)malloc(kb); for (int i = 0; i < K*N; i++) hkt[i] = __float2half((float)((i % 5) * 0.625f));
    __half* hv = (__half*)malloc(vb); for (int i = 0; i < M*N; i++) hv[i] = __float2half((float)((i % 3) * 0.25f));
    CHECK_CU(cudaMemcpy(d_q, hq, qb, cudaMemcpyHostToDevice));
    CHECK_CU(cudaMemcpy(d_kt, hkt, kb, cudaMemcpyHostToDevice));
    CHECK_CU(cudaMemcpy(d_v, hv, vb, cudaMemcpyHostToDevice));
    free(hq); free(hkt); free(hv);

    const __half alpha = __float2half(1.0f);
    const __half zero = __float2half(0.0f);
    cublasGemmEx(h, CUBLAS_OP_N, CUBLAS_OP_N, N, M, K,
                 &alpha, d_kt, CUDA_R_16F, N, d_q, CUDA_R_16F, K,
                 &zero, d_s, CUDA_R_16F, N, CUBLAS_COMPUTE_32F, CUBLAS_GEMM_DEFAULT_TENSOR_OP);
    cudaDeviceSynchronize();

    for (int w = 0; w < 5; w++) {
        cublasGemmEx(h, CUBLAS_OP_N, CUBLAS_OP_N, N, M, K,
                     &alpha, d_kt, CUDA_R_16F, N, d_q, CUDA_R_16F, K,
                     &zero, d_s, CUDA_R_16F, N, CUBLAS_COMPUTE_32F, CUBLAS_GEMM_DEFAULT_TENSOR_OP);
        scale_half<<<(M*N + 255) / 256, 256>>>(d_s, M * N);
        cublasGemmEx(h, CUBLAS_OP_N, CUBLAS_OP_N, N, M, N,
                     &alpha, d_v, CUDA_R_16F, N, d_s, CUDA_R_16F, N,
                     &zero, d_o, CUDA_R_16F, N, CUBLAS_COMPUTE_32F, CUBLAS_GEMM_DEFAULT_TENSOR_OP);
    }
    cudaDeviceSynchronize();

    double t0 = now_ms();
    for (int i = 0; i < iters; i++) {
        cublasGemmEx(h, CUBLAS_OP_N, CUBLAS_OP_N, N, M, K,
                     &alpha, d_kt, CUDA_R_16F, N, d_q, CUDA_R_16F, K,
                     &zero, d_s, CUDA_R_16F, N, CUBLAS_COMPUTE_32F, CUBLAS_GEMM_DEFAULT_TENSOR_OP);
        scale_half<<<(M*N + 255) / 256, 256>>>(d_s, M * N);
        cublasGemmEx(h, CUBLAS_OP_N, CUBLAS_OP_N, N, M, N,
                     &alpha, d_v, CUDA_R_16F, N, d_s, CUDA_R_16F, N,
                     &zero, d_o, CUDA_R_16F, N, CUBLAS_COMPUTE_32F, CUBLAS_GEMM_DEFAULT_TENSOR_OP);
    }
    cudaDeviceSynchronize();
    double t1 = now_ms();

    // Verify the output is finite (a sanity check, not a full maxrel).
    __half h0 = 0; CHECK_CU(cudaMemcpy(&h0, d_o, 2, cudaMemcpyDeviceToHost));
    printf("cuBLAS composition @%dx%dx%d: %.3f ms/iter (o[0]=%.4f)\n", M, N, K, (t1 - t0) / iters, __half2float(h0));

    cublasDestroy(h);
    CHECK_CU(cudaFree(d_q)); CHECK_CU(cudaFree(d_kt)); CHECK_CU(cudaFree(d_v));
    CHECK_CU(cudaFree(d_s)); CHECK_CU(cudaFree(d_o));
    return 0;
}