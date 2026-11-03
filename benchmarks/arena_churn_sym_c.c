// arena_churn_sym_c — C reference for arena_churn_sym.bv.
// Bump arena with a 64-BYTE allocation per iteration (mirrors Alloc#(64)),
// VOLATILE store/load so the memory work is observable. Grows via realloc.
#include <stdlib.h>
#include <stdio.h>

int main(void) {
    const char* env = getenv("BOUND");
    long N = env ? atol(env) : 50000000L;
    long cap = 1 << 20;           /* bytes */
    unsigned char* buf = malloc(cap);
    long used = 0;
    long ops = 0, sum = 0;
    volatile long* v;
    for (; ops < N; ) {
        if (used + 64 > cap) {
            cap *= 2;
            buf = realloc(buf, cap);
        }
        v = (volatile long*)(buf + used);
        *v = ops;
        sum += *v;
        used += 64;
        ops++;
        if (ops % 5000000 == 0)
            fprintf(stdout, "%ld\n", sum);
    }
    free(buf);
    return 0;
}
