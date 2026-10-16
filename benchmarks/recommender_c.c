/* recommender_c.c — C reference for benchmarks/recommender.bv.
 *
 * Derived from Otto Westerlund's cpp-vs-briev single-pass reference
 * (github.com/odecode/cpp-vs-briev, commit 714130e); ported to plain C and
 * made self-contained: the MovieLens-100k rating matrix and the similarity
 * row are synthesized by the SAME deterministic LCG the Briev program uses,
 * so the harness's byte-exact correctness gate holds without data files.
 *
 * Identical work to the Briev side: dense 943x1682 synthesis, column-major
 * per-item sparse layout (count -> prefix -> fill), single-pass per-item
 * prediction with the reference's `+first -last` off-by-one compensation,
 * clamp to [0,5], top-5 by strictly-greater insertion (ties keep the lowest
 * item index). Output: the five item indices and the prediction count.
 *
 * Method: Y. Ren, G. Li, J. Zhang, W. Zhou, "The Efficient Imputation
 * Method for Neighborhood-based Collaborative Filtering", CIKM'12.
 */
#include <stdio.h>
#include <stdlib.h>

#define NROWS 943
#define NCOLS 1682
#define MAXNZ 100000
#define QUERY_USER 10

static long long lcg = 12345;
static long long next_lcg(void) {
    lcg = (lcg * 1103515245LL + 12345LL) % 2147483647LL;
    return lcg;
}

static float *mat;   /* [NROWS * NCOLS] */
static float *sim;   /* [NROWS] */
static int   *st;    /* [NCOLS + 1] prefix */
static int   *cp;    /* [NCOLS] fill cursor */
static int   *nu;    /* [MAXNZ] per-item users */
static float *nr;    /* [MAXNZ] per-item ratings */

/* Single-pass hot kernel — mirrors Briev's defn predict_item. */
static float predict_item(int it) {
    int blk_lo = st[it];
    int blk_hi = st[it + 1];
    float sa = 0.0f, sb = 0.0f;
    int first_v = -1, last_v = -1;
    float first_r = 0.0f, last_r = 0.0f;
    for (int k = blk_lo; k < blk_hi; ++k) {
        int vv = nu[k];
        if (vv != QUERY_USER) {
            float rv = nr[k];
            float w = sim[vv];
            sa += w * (rv - 1.0f);
            sb += w;
            if (first_v < 0) { first_v = vv; first_r = rv; }
            last_v = vv; last_r = rv;
        }
    }
    if (first_v >= 0) {
        float wf = sim[first_v];
        sa += wf * (first_r - 1.0f);
        sb += wf;
    }
    if (last_v >= 0) {
        float wl = sim[last_v];
        sa -= wl * (last_r - 1.0f);
        sb -= wl;
    }
    float denom = sb;
    if (denom == 0.0f) denom = 0.00001f;
    float pred = 1.0f + sa / denom;
    if (pred > 5.0f) pred = 5.0f;
    if (pred < 0.0f) pred = 0.0f;
    return pred;
}

int main(void) {
    mat = malloc((size_t)NROWS * NCOLS * sizeof(float));
    sim = malloc((size_t)NROWS * sizeof(float));
    st  = malloc((size_t)(NCOLS + 1) * sizeof(int));
    cp  = malloc((size_t)NCOLS * sizeof(int));
    nu  = malloc((size_t)MAXNZ * sizeof(int));
    nr  = malloc((size_t)MAXNZ * sizeof(float));
    if (!mat || !sim || !st || !cp || !nu || !nr) return 1;

    long long n_ratings = 0;
    for (int u = 0; u < NROWS; ++u) {
        for (int it = 0; it < NCOLS; ++it) {
            next_lcg();
            if (lcg % 16 == 0) {
                long long r128 = lcg / 65536;
                mat[(size_t)u * NCOLS + it] = (float)(r128 % 5 + 1);
                n_ratings++;
            }
        }
    }
    for (int v = 0; v < NROWS; ++v) {
        next_lcg();
        sim[v] = (float)(lcg % 997);
    }

    /* Sparse per-item layout: count -> prefix -> fill. */
    for (int c0 = 0; c0 < NCOLS; ++c0) st[c0] = 0;
    for (int uu = 0; uu < NROWS; ++uu)
        for (int it = 0; it < NCOLS; ++it)
            if (mat[(size_t)uu * NCOLS + it] != 0.0f) st[it]++;
    int acc = 0;
    for (int it2 = 0; it2 < NCOLS; ++it2) {
        int c = st[it2];
        st[it2] = acc;
        cp[it2] = acc;
        acc += c;
    }
    st[NCOLS] = acc;
    for (int uu2 = 0; uu2 < NROWS; ++uu2)
        for (int it3 = 0; it3 < NCOLS; ++it3) {
            float rv = mat[(size_t)uu2 * NCOLS + it3];
            if (rv != 0.0f) {
                int pos = cp[it3];
                nu[pos] = uu2;
                nr[pos] = rv;
                cp[it3] = pos + 1;
            }
        }

    /* Prediction: per item over its sparse block; top-5 strictly-greater. */
    long long n_preds = 0;
    int t0i = -1, t1i = -1, t2i = -1, t3i = -1, t4i = -1;
    float t0r = -1.0f, t1r = -1.0f, t2r = -1.0f, t3r = -1.0f, t4r = -1.0f;
    for (int item = 0; item < NCOLS; ++item) {
        if (mat[(size_t)QUERY_USER * NCOLS + item] != 0.0f) continue;
        float pred = predict_item(item);
        n_preds++;
        if (pred > t4r) {
            t4i = item; t4r = pred;
            if (t4r > t3r) { int ti = t3i; float tr = t3r; t3i = t4i; t3r = t4r; t4i = ti; t4r = tr; }
            if (t3r > t2r) { int ti = t2i; float tr = t2r; t2i = t3i; t2r = t3r; t3i = ti; t3r = tr; }
            if (t2r > t1r) { int ti = t1i; float tr = t1r; t1i = t2i; t1r = t2r; t2i = ti; t2r = tr; }
            if (t1r > t0r) { int ti = t0i; float tr = t0r; t0i = t1i; t0r = t1r; t1i = ti; t1r = tr; }
        }
    }

    printf("%d\n%d\n%d\n%d\n%d\n%lld\n", t0i, t1i, t2i, t3i, t4i, n_preds);
    free(mat); free(sim); free(st); free(cp); free(nu); free(nr);
    return 0;
}
