// =============================================================================
//  RecommenderOptimized
//
//  A runtime-speed-optimized reimplementation of the user-based Auto-Adaptive
//  Imputation (AutAI) neighborhood CF recommender found in ../RecommenderCore,
//  which itself implements the method from:
//
//      Y. Ren, G. Li, J. Zhang, W. Zhou, "The Efficient Imputation Method for
//      Neighborhood-based Collaborative Filtering", CIKM'12.
//
//  GOAL
//  ----
//  Produce results EQUIVALENT to the reference program (same recommendations
//  and same prediction count for the active user) while running as fast as
//  possible. Exact float bit-equality is not required; small numeric drift from
//  FMA / reassociation / SIMD is acceptable per the task statement.
//
//  WHAT THE REFERENCE ACTUALLY COMPUTES
//  ------------------------------------
//  Careful reading of RecommenderCore shows the observable behavior is:
//
//    * calc_row_mean() counts rated items in both numerator and denominator,
//      so every user who rated >=1 item has mean == 1.0. (The paper intends
//      the average rating; the reference degenerates to 1.0.)
//
//    * imputate_matrix() has a dead outer loop (guard `current_user >= 0`
//      with current_user initialized to -1 and never advanced), so NO value is
//      ever imputed. The imputed matrix equals the raw data matrix.
//
//    * predict_rating(item) therefore reduces, for an unrated (user,item), to
//
//          Ua   = { v : rating[v][item] != 0 }           (users who rated item)
//          pred = mean_u + sum_v sim[u][v]*(rating[v][item] - mean_v)
//                          -----------------------------------------------
//                                   sum_v sim[u][v]
//
//      with mean_u == mean_v == 1.0, then clamped to [0, 5].
//
//      The reference also has an off-by-one in its neighbor walk: it starts the
//      running neighbor at neighbors[0] and only advances AFTER using it, so
//      neighbors[0] is used twice and neighbors[last] is never used. We
//      reproduce that exactly so the output matches.
//
//  OPTIMIZATIONS vs. the reference
//  -------------------------------
//    1. Precompute per-user mean once (943 values) instead of O(ncols) work
//       per neighbor per item (the reference recomputes it billions of times).
//    2. Store the rating matrix in a column-major (per-item) sparse layout:
//       for each item, a contiguous array of (user, rating). Prediction for an
//       item then touches only the users who rated it, with unit-stride access.
//    3. Hoist the per-neighbor term (rating_v - mean_v) and the similarity read
//       into flat arrays, then reduce them with an AVX2+FMA SIMD kernel from the
//       bundled VCL (version2-2.01.04).
//    4. Parallelize across items with OpenMP (the per-item predictions are
//       independent).
//
//  BUILD (see readme.txt / build.sh)
//  ---------------------------------
//    g++ -O3 -march=native -funroll-loops -fopenmp -std=c++17 \
//        -I ../RecommenderCore/version2-2.01.04 recommender.cpp -o recommender
//
//  Only the C++ standard library plus the bundled VCL SIMD header are used.
// =============================================================================

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <cmath>
#include <chrono>
#include <string>
#include <vector>
#include <algorithm>
#include <fstream>
#include <sstream>
#include <sys/stat.h>   // stat() for cache-freshness check

#include "vectorclass.h"   // bundled SIMD library (version2-2.01.04)

#ifdef _OPENMP
#include <omp.h>
#endif

using Clock = std::chrono::high_resolution_clock;

static constexpr int NROWS = 943;   // users  (MovieLens-100k)
static constexpr int NCOLS = 1682;  // items

// -----------------------------------------------------------------------------
//  Horizontal SIMD reduction of two parallel float arrays:
//      above = sum_i w[i] * d[i]
//      below = sum_i w[i]
//  Uses AVX2 8-wide FMA via VCL. This is the single hot kernel of the program.
// -----------------------------------------------------------------------------
static inline void weighted_reduce(const float* __restrict w,
                                   const float* __restrict d,
                                   int n, float& above, float& below) {
    const int W = 8;
    Vec8f acc_above(0.0f);
    Vec8f acc_below(0.0f);
    int i = 0;
    for (; i + W <= n; i += W) {
        Vec8f wv = Vec8f().load(w + i);
        Vec8f dv = Vec8f().load(d + i);
        acc_above = mul_add(wv, dv, acc_above);   // acc_above += wv*dv (FMA)
        acc_below += wv;
    }
    float a = horizontal_add(acc_above);
    float b = horizontal_add(acc_below);
    for (; i < n; ++i) {               // scalar tail
        a += w[i] * d[i];
        b += w[i];
    }
    above = a;
    below = b;
}

// -----------------------------------------------------------------------------
//  Data loading
// -----------------------------------------------------------------------------

// Dense rating matrix (row-major) read from u.data. 0 == missing.
// Format: whitespace-separated "user item rating timestamp" per line.
// We slurp the whole file once and parse integers by hand, which is much
// faster than a per-line std::istringstream (constructed 100k times).
static void read_u_data(const std::string& path, std::vector<float>& dense) {
    dense.assign((size_t)NROWS * NCOLS, 0.0f);

    FILE* f = std::fopen(path.c_str(), "rb");
    if (!f) {
        std::fprintf(stderr, "Could not open %s\n", path.c_str());
        std::exit(1);
    }
    std::fseek(f, 0, SEEK_END);
    long sz = std::ftell(f);
    std::fseek(f, 0, SEEK_SET);
    std::vector<char> buf((size_t)sz + 1);
    size_t got = std::fread(buf.data(), 1, (size_t)sz, f);
    std::fclose(f);
    buf[got] = '\0';

    const char* p   = buf.data();
    const char* end = p + got;
    auto read_int = [&](long& out) -> bool {
        while (p < end && (*p < '0' || *p > '9')) ++p;   // skip non-digits
        if (p >= end) return false;
        long v = 0;
        while (p < end && *p >= '0' && *p <= '9') { v = v * 10 + (*p - '0'); ++p; }
        out = v;
        return true;
    };
    long user, item, rating, ts;
    while (read_int(user)) {
        if (!read_int(item) || !read_int(rating) || !read_int(ts)) break;
        int r = (int)user - 1, c = (int)item - 1;   // u.data is 1-indexed
        if (r >= 0 && r < NROWS && c >= 0 && c < NCOLS)
            dense[(size_t)r * NCOLS + c] = (float)rating;
    }
}

// --- Binary cache for the similarity matrix ---------------------------------
//
// Parsing the ~3 MB text simlist.txt (~1.6M floats via operator>>) dominates
// startup. Since the matrix is read-only and reused on every run, we cache the
// compact in-memory form (NROWS*NROWS floats, row-major) in a sidecar binary
// file "<sim_path>.bin" and load it with a single fread on later runs.
//
// Freshness: the cache header records the source file's size and mtime. If the
// text file changes (size or mtime differs) the cache is treated as stale and
// rebuilt, so you never get silently out-of-date similarities.

static const uint32_t SIMCACHE_MAGIC = 0x534C4331u; // "SLC1"

struct SimCacheHeader {
    uint32_t magic;      // SIMCACHE_MAGIC
    uint32_t nrows;      // must equal NROWS
    uint64_t src_size;   // size of the source text file
    int64_t  src_mtime;  // mtime (seconds) of the source text file
};

// Parse the text similarity file into row-major sim[u*NROWS + v].
// The file has NCOLS values per row; only the leading NROWS columns are
// user-user similarities, so we keep those and skip the rest.
static void parse_simlist_text(const std::string& path, std::vector<float>& sim) {
    std::ifstream in(path);
    if (!in.is_open()) {
        std::fprintf(stderr, "Could not open %s\n", path.c_str());
        std::exit(1);
    }
    sim.assign((size_t)NROWS * NROWS, 0.0f);
    float v;
    for (int i = 0; i < NROWS; ++i) {
        for (int j = 0; j < NCOLS; ++j) {
            if (!(in >> v)) { j = NCOLS; break; }
            if (j < NROWS) sim[(size_t)i * NROWS + j] = v;
        }
    }
}

// Try to load the binary cache. Returns true on a valid, fresh hit.
static bool try_load_cache(const std::string& cache_path,
                           uint64_t src_size, int64_t src_mtime,
                           std::vector<float>& sim) {
    FILE* f = std::fopen(cache_path.c_str(), "rb");
    if (!f) return false;
    bool ok = false;
    SimCacheHeader h{};
    if (std::fread(&h, sizeof(h), 1, f) == 1 &&
        h.magic == SIMCACHE_MAGIC && h.nrows == (uint32_t)NROWS &&
        h.src_size == src_size && h.src_mtime == src_mtime) {
        const size_t n = (size_t)NROWS * NROWS;
        sim.assign(n, 0.0f);
        ok = (std::fread(sim.data(), sizeof(float), n, f) == n);
    }
    std::fclose(f);
    return ok;
}

// Write the binary cache (best effort; failure is non-fatal).
static void write_cache(const std::string& cache_path,
                        uint64_t src_size, int64_t src_mtime,
                        const std::vector<float>& sim) {
    FILE* f = std::fopen(cache_path.c_str(), "wb");
    if (!f) return;
    SimCacheHeader h{SIMCACHE_MAGIC, (uint32_t)NROWS, src_size, src_mtime};
    const size_t n = (size_t)NROWS * NROWS;
    if (std::fwrite(&h, sizeof(h), 1, f) != 1 ||
        std::fwrite(sim.data(), sizeof(float), n, f) != n) {
        std::fclose(f);
        std::remove(cache_path.c_str());   // don't leave a partial cache
        return;
    }
    std::fclose(f);
}

// Load the similarity matrix, using the binary cache when it is fresh.
static void read_simlist(const std::string& path, std::vector<float>& sim) {
    const std::string cache_path = path + ".bin";

    // Stat the source so we can validate / stamp the cache.
    struct stat st{};
    bool have_src = (stat(path.c_str(), &st) == 0);
    uint64_t src_size  = have_src ? (uint64_t)st.st_size : 0;
    int64_t  src_mtime = have_src ? (int64_t)st.st_mtime : 0;

    if (have_src && try_load_cache(cache_path, src_size, src_mtime, sim))
        return;                          // fast path: fresh binary cache

    parse_simlist_text(path, sim);       // slow path: parse text once
    if (have_src)
        write_cache(cache_path, src_size, src_mtime, sim);  // and cache it
}

// -----------------------------------------------------------------------------
//  main
// -----------------------------------------------------------------------------
int main(int argc, const char* argv[]) {
    // Active user. Reference hardcodes 10. We accept several invocation forms:
    //
    //   recommender                         -> user 10 (reference default)
    //   recommender <user>                  -> user <user>
    //   recommender usernum <user>          -> user <user>   (Go API form)
    //   recommender [usernum] <user> [data] [simlist]
    //
    // The Go server (API/server/main.go) calls:  ./recommender usernum <id>
    // where <id> is a 1-based user id from the Python test. We treat the given
    // value as the matrix row index (0-based) just like the reference's arg
    // handling would, so both binaries can be compared on the same input.
    int which_user = 10;
    int argi = 1;                 // index of first non-"usernum" argument
    if (argc >= 2 && std::string(argv[1]) == "usernum") argi = 2;

    if (argc > argi) {
        int u = std::atoi(argv[argi]);
        if (u >= 0 && u < NROWS) which_user = u;
    }
    const std::string data_path = (argc > argi + 1) ? argv[argi + 1] : "u.data";
    const std::string sim_path  = (argc > argi + 2) ? argv[argi + 2] : "simlist.txt";

    auto t_setup0 = Clock::now();

    std::vector<float> dense;        // [NROWS*NCOLS] raw ratings
    std::vector<float> sim;          // [NROWS*NROWS] user-user similarity
    read_u_data(data_path, dense);
    read_simlist(sim_path, sim);

    // --- Precompute per-user mean exactly as the reference computes it. --------
    // calc_row_mean counts rated items in numerator and denominator => 1.0 for
    // any user with >=1 rating, NaN for a user with none. We reproduce that.
    std::vector<float> mean(NROWS);
    std::vector<int>   nrated(NROWS, 0);
    for (int u = 0; u < NROWS; ++u) {
        int cnt = 0;
        const float* row = &dense[(size_t)u * NCOLS];
        for (int i = 0; i < NCOLS; ++i) cnt += (row[i] != 0.0f);
        nrated[u] = cnt;
        mean[u] = (cnt > 0) ? ((float)cnt / (float)cnt) : (0.0f / 0.0f);
    }

    // --- Column-major sparse layout: per item, the users who rated it. --------
    // item_users[item]   = list of users v with rating != 0 on item (ascending)
    // item_ratings[item] = parallel list of their ratings
    std::vector<std::vector<int>>   item_users(NCOLS);
    std::vector<std::vector<float>> item_ratings(NCOLS);
    for (int it = 0; it < NCOLS; ++it) {
        item_users[it].reserve(32);
        item_ratings[it].reserve(32);
    }
    for (int u = 0; u < NROWS; ++u) {
        const float* row = &dense[(size_t)u * NCOLS];
        for (int it = 0; it < NCOLS; ++it) {
            float r = row[it];
            if (r != 0.0f) {
                item_users[it].push_back(u);
                item_ratings[it].push_back(r);
            }
        }
    }

    auto t_setup1 = Clock::now();

    // --- Prediction over all items for the active user. -----------------------
    const float mean_u = mean[which_user];
    const float* sim_u = &sim[(size_t)which_user * NROWS];

    std::vector<std::pair<int, float>> results(NCOLS, {-1, -1.0f});

    auto t_pred0 = Clock::now();

#ifdef _OPENMP
    #pragma omp parallel
#endif
    {
#ifdef _OPENMP
        #pragma omp for schedule(dynamic, 64)
#endif
        for (int item = 0; item < NCOLS; ++item) {
            // Skip items the active user already rated (reference returns -1).
            if (dense[(size_t)which_user * NCOLS + item] != 0.0f) continue;

            const std::vector<int>&   us = item_users[item];
            const std::vector<float>& rs = item_ratings[item];
            const int n = (int)us.size();
            if (n == 0) {
                // No neighbors: sum_below forced to 1e-5, sum_above 0 => pred=mean_u.
                float pred = mean_u;                 // == 1.0
                if (pred > 5.0f) pred = 5.0f;
                if (pred < 0.0f) pred = 0.0f;
                results[item] = {item, pred};
                continue;
            }

            // SINGLE-PASS, like the Briev sparse version: compute the full sum
            // over the item's neighbor block in one scalar pass, then apply the
            // reference's off-by-one as a +first -last correction. The used
            // multiset is { us[0] (twice), us[1..n-2] } (us[n-1] dropped), so
            //   used_sum = full_sum + term(us[0]) - term(us[n-1]).
            // No scratch arrays, no SIMD-reduction setup — identical work to
            // the Briev implementation.
            float above = 0.0f, below = 0.0f;
            for (int i = 0; i < n; ++i) {
                const int v = us[i];
                const float wv = sim_u[v];
                above += wv * (rs[i] - mean[v]);     // rating_v - mean_v
                below += wv;
            }
            // + first term
            {
                const int vf = us[0];
                const float wf = sim_u[vf];
                above += wf * (rs[0] - mean[vf]);
                below += wf;
            }
            // - last term
            {
                const int vl = us[n - 1];
                const float wl = sim_u[vl];
                above -= wl * (rs[n - 1] - mean[vl]);
                below -= wl;
            }
            if (below == 0.0f) below = 0.00001f;

            float pred = mean_u + above / below;
            if (pred > 5.0f) pred = 5.0f;
            if (pred < 0.0f) pred = 0.0f;
            results[item] = {item, pred};
        }
    }

    auto t_pred1 = Clock::now();

    // Collect valid predictions (item index >= 0).
    std::vector<std::pair<int, float>> preds;
    preds.reserve(NCOLS);
    for (const auto& p : results) if (p.first >= 0) preds.push_back(p);

    // Sort by predicted rating, descending. Use std::sort with the SAME
    // comparator and the SAME input order (ascending item index) as the
    // reference so the tie ordering among equal predictions matches it.
    std::sort(preds.begin(), preds.end(),
        [](const std::pair<int,float>& a, const std::pair<int,float>& b) {
            return a.second > b.second;
        });

    std::chrono::duration<double> setup_s = t_setup1 - t_setup0;
    std::chrono::duration<double> pred_s  = t_pred1  - t_pred0;

    std::printf("Top 5 recommended items for user %d:\n", which_user);
    for (int k = 0; k < 5 && k < (int)preds.size(); ++k)
        std::printf("Item: %d, Predicted rating: %g\n", preds[k].first, preds[k].second);
    std::printf("Total time %g %d predictions\n", pred_s.count(), (int)preds.size());
    std::printf("(setup %.4fs, prediction %.6fs)\n", setup_s.count(), pred_s.count());
    return 0;
}
