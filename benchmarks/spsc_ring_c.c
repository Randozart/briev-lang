// spsc_ring_c.c — C11-atomics reference for spsc_ring.bv.
#include <stdio.h>
#include <stdlib.h>
#include <stdatomic.h>

#define CAP 1024

int main(void) {
    const char *b = getenv("BOUND");
    long n = b ? atol(b) : 0;
    long *slots = malloc(CAP * sizeof(long));
    atomic_long head = 0, tail = 0;
    long sum = 0;
    for (long i = 0; i < n; i++) {
        // push
        long h = atomic_load_explicit(&head, memory_order_acquire);
        long t = atomic_load_explicit(&tail, memory_order_relaxed);
        long next = (t + 1) & (CAP - 1);
        if (next != h) {
            slots[t] = i;
            atomic_store_explicit(&tail, next, memory_order_release);
            // pop
            t = atomic_load_explicit(&tail, memory_order_acquire);
            h = atomic_load_explicit(&head, memory_order_relaxed);
            if (t != h) {
                long v = slots[h];
                long nhead = (h + 1) & (CAP - 1);
                atomic_store_explicit(&head, nhead, memory_order_release);
                sum += v;
            }
        }
        if (i % 5000000 == 0) printf("%ld\n", sum);
    }
    printf("%ld\n", sum);
    free(slots);
    return 0;
}
