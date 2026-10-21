// arena_bump_c.c — C reference for arena_bump.bv.
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

#define CAP (1024 * 1024)

int main(void) {
    const char *b = getenv("BOUND");
    long n = b ? atol(b) : 0;
    unsigned char *base = malloc(CAP);
    long offset = 0;
    long sum = 0;
    for (long i = 0; i < n; i++) {
        if (offset + 64 > CAP) offset = 0;
        long *p = (long *)(base + offset);
        *p = i;
        sum += *p;
        offset += 64;
        // Keep the loop observable (otherwise -O3 folds the whole bump walk).
        if (i % 5000000 == 0) printf("%ld\n", sum);
    }
    printf("%ld\n", sum);
    free(base);
    return 0;
}
