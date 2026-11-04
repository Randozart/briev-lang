//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//

        #include <stddef.h>
        #include <stdint.h>
        #include <stdlib.h>
        static volatile int live;
        static int fail_malloc_after = -1, fail_resize;
        void *__real_malloc(size_t);
        void __real_free(void *);
        void *__real_realloc(void *, size_t);
        void *__wrap_malloc(size_t size) {
            if (fail_malloc_after == 0) return NULL;
            if (fail_malloc_after > 0) --fail_malloc_after;
            void *allocated = __real_malloc(size);
            if (allocated) ++live;
            return allocated;
        }
        void __wrap_free(void *address) { if (address) --live; __real_free(address); }
        void *__wrap_realloc(void *address, size_t size) {
            if (fail_resize) { fail_resize = 0; return NULL; }
            void *resized = __real_realloc(address,size);
            if (!address && resized) ++live;
            if (address && size == 0 && !resized) --live;
            return resized;
        }
        extern void __briev_region_finish(void);
        extern void __briev_region_enter(void *);
        extern void __briev_region_leave(void *);
        extern void *__briev_region_suspend(void);
        extern void __briev_region_restore(void *);
        extern void *__briev_region_malloc(uint64_t);
        extern void *__briev_region_adopt(void *);
        extern void *__briev_region_realloc(void *, uint64_t);
        extern void __briev_region_free(void *);
        int main(void) {
            void *outer[3], *inner[3], *nested[3];
            char *borrowed = __briev_region_malloc(32);
            borrowed[0] = 42;
            __briev_region_enter(outer);
            void *external = __wrap_malloc(24);
            if (__briev_region_adopt(external) != external) return 7;
            void *first = __briev_region_malloc(16);
            void *growing = __briev_region_malloc(16);
            __briev_region_free(first);
            __briev_region_enter(inner);
            growing = __briev_region_realloc(growing, 20000);
            if (!growing) return 2;
            __briev_region_malloc(64);
            void *saved = __briev_region_suspend();
            __briev_region_enter(nested);
            __briev_region_malloc(8);
            __briev_region_free(growing);
            __briev_region_leave(nested);
            __briev_region_restore(saved);
            __briev_region_leave(inner);
            fail_malloc_after = 1;
            if (__briev_region_malloc(16)) return 4;
            fail_malloc_after = -1;
            char *preserved = __briev_region_malloc(32);
            preserved[0] = 73;
            fail_resize = 1;
            if (__briev_region_realloc(preserved,20000)) return 5;
            if (preserved[0] != 73) return 6;
            void *zero = __briev_region_malloc(8);
            __briev_region_realloc(zero,0);
            __briev_region_leave(outer);
            __briev_region_enter(outer);
            __briev_region_malloc(8);
            __briev_region_enter(inner);
            __briev_region_malloc(16);
            __briev_region_finish();
            if (borrowed[0] != 42) return 3;
            __briev_region_free(borrowed);
            return live != 0;
        }
    