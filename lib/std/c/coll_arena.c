/*===-- coll_arena.c - request-scoped coll backing store ------*- C -*-===*\
|*                                                                            *|
|* SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception                    *|
|*                                                                            *|
\*===----------------------------------------------------------------------===*/

/* Request-scoped bump arena for generic `coll` backing stores. The four
 * entry points are weak: the compiler links this file as an FFI object for
 * the stdlib declarations AND briev_rt.c includes it for direct harness
 * links, so a strong definition would collide at link time. The coll
 * resize path allocates through this runtime; begin/reset are called by a
 * service request boundary. Outside a request coll allocations use libc. */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

static unsigned char *base;
static size_t cap;
static size_t off;
static int active;

__attribute__((weak)) void coll_arena_begin(long long bytes) {
    size_t want = bytes > 0 ? (size_t)bytes : 0;
    if (base == NULL || want > cap) {
        free(base);
        base = (unsigned char *)malloc(want);
        cap = base ? want : 0;
    }
    off = 0;
    active = base != NULL;
}

__attribute__((weak)) void coll_arena_reset(void) { off = 0; }
__attribute__((weak)) void coll_arena_end(void) { off = 0; active = 0; }

__attribute__((weak)) long long coll_alloc(long long bytes) {
    size_t need = bytes > 0 ? (size_t)bytes : 0;
    if (active) {
        size_t start = (off + 7) & ~(size_t)7;
        if (need <= cap - start && start <= cap) {
            off = start + need;
            return (long long)(base + start);
        }
    }
    return (long long)malloc(need);
}
