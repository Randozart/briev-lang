// arena_churn_sym_c — C reference for arena_churn_sym.bv.
// Bump arena (64KB, grows via realloc) with VOLATILE store/load so the memory
// work is observable and cannot be optimized away. Same sum as arena_churn.
#include <stdlib.h>
#include <stdio.h>

int main(void) {
    const char* env = getenv("BOUND");
    long N = env ? atol(env) : 50000000L;
    long* buf = malloc(65536);
    long cap = 8192;
    long used = 0;
    long ops = 0, sum = 0;
    volatile long* vbuf;
    for (; ops < N; ) {
        if (used + 1 > cap) {
            cap *= 2;
            buf = realloc(buf, cap * sizeof(long));
        }
        vbuf = buf;
        vbuf[used] = ops;
        sum += vbuf[used];
        used++;
        ops++;
        if (ops % 5000000 == 0)
            fprintf(stdout, "%ld\n", sum);
    }
    free(buf);
    return 0;
}
