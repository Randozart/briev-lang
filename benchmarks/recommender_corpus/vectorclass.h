// Minimal stub for github.com/odecode/cpp-vs-briev — the bundled VCL
// (version2-2.01.04) headers were not committed to that repo. The only VCL
// consumer in recommender_singlepass.cpp is weighted_reduce(), which is DEAD
// CODE in the single-pass build (main() uses the scalar loop). Any compiling
// definition suffices; semantics below mirror Vec8f for completeness.
#pragma once
#include <immintrin.h>

struct Vec8f {
    __m256 v;
    Vec8f() : v(_mm256_setzero_ps()) {}
    Vec8f(float f) : v(_mm256_set1_ps(f)) {}
    explicit Vec8f(__m256 x) : v(x) {}
    inline Vec8f load(const float* p) { return Vec8f(_mm256_loadu_ps(p)); }
    inline Vec8f operator*(Vec8f o) const { return Vec8f(_mm256_mul_ps(v, o.v)); }
    inline Vec8f operator+(Vec8f o) const { return Vec8f(_mm256_add_ps(v, o.v)); }
    inline Vec8f operator+=(Vec8f o) { v = _mm256_add_ps(v, o.v); return *this; }
};

static inline Vec8f mul_add(Vec8f a, Vec8f b, Vec8f c) {
    return Vec8f(_mm256_fmadd_ps(a.v, b.v, c.v));
}
static inline float horizontal_add(Vec8f a) {
    __m128 lo = _mm256_castps256_ps128(a.v);
    __m128 hi = _mm256_extractf128_ps(a.v, 1);
    lo = _mm_add_ps(lo, hi);
    __m128 s = _mm_hadd_ps(lo, lo);
    s = _mm_hadd_ps(s, s);
    return _mm_cvtss_f32(s);
}
