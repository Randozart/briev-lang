// Async Parallel Counters — Idiomatic Optimizer C Reference
//
// History:
//   2026-07-01: Created as optimizer companion to async_counters_idio.bv.
//     Original version modeled a converged state (`g_a = N`) with dead
//     stores — clang -O3 eliminated everything and printed NOTHING. That
//     was wrong: the Briev source has observable `println!` effects (every
//     10M), which may never be folded away (observability doctrine: a
//     value is live if an effect consumes it).
//   2026-09-30: Rewritten as a semantic mirror of async_counters_idio.bv
//     (the long-standing 10-vs-1 harness MISMATCH, recorded since
//     2026-07-19). The backend's deterministic schedule drains each async
//     node's bounded counter inside one reactor firing (counted-match
//     loop): inc_a runs to N first, then inc_b — and the async bodies are
//     emitted in sorted name order (BUGS.md, 2026-09-30 determinism fix).
//     The print order across the two async nodes is contract-unspecified
//     (async = acknowledged simultaneous firing, disjoint write sets), so
//     this companion pins the schedule the backend emits today; if the
//     schedule changes, update this companion in the same commit.
//
// Build:
//   clang -O3 -march=native -o benchmarks/async_counters_idio_c \
//     benchmarks/async_counters_idio_c.c

#include <stdio.h>

#define N 50000000L

int main(void) {
    long a = 0;
    long b = 0;
    while (a < N) {
        a++;
        if (a % 10000000 == 0) printf("%ld\n", a);
    }
    while (b < N) {
        b++;
        if (b % 10000000 == 0) printf("%ld\n", b);
    }
    return 0;
}
