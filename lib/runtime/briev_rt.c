/*
 * briev_rt.c — Minimal runtime for Briev LLVM backend
 *
 * 2026-07-15: Stripped ~70 briev_* wrapper functions that were replaced
 * by SysCall#/SysConf#/Atomic*# intrinsics. Only keeps infrastructure
 * functions (__rt_init, __rt_wait, barriers, threads, triggers) and
 * the two remaining intrinsics: briev_syscall, briev_sysconf.
 */

#define _GNU_SOURCE
#include <stddef.h>
#include <stdint.h>
#include <signal.h>
#include <time.h>
#include <string.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include <dirent.h>
#ifdef __has_include
#if __has_include(<dlfcn.h>)
#include <dlfcn.h>
#endif
#endif
#include <unistd.h>
#include <fcntl.h>
#include <pthread.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/syscall.h>
#ifdef __linux__
#include <sys/utsname.h>
#include <sys/random.h>
#include <execinfo.h>
#endif
#ifdef __APPLE__
#include <mach/mach_time.h>
#include <sys/sysctl.h>
#endif
#include <sys/ioctl.h>

// ── Integer type for Briev C ABI ──────────────────────────────────────
#ifndef _BRIEV_INT_DEFINED
#define _BRIEV_INT_DEFINED
#if defined(__LP64__) || defined(_WIN64)
typedef int64_t briev_int;
#else
typedef int32_t briev_int;
#endif
#endif

// 2026-09-23 (frgn-elimination round 2): the C string-conversion helpers
// briev_str_to_c / briev_cstr_to_briev / briev_cstring_concat were DELETED.
// Their pure-Briev twins live in lib/glue/c.bv (str_to_c zero-copy view,
// cstr_to_briev / cstring_concat over Alloc#/Copy#); the cstr doors no longer
// depend on the runtime. briev_free_briev_str stays — briev_bits_to_str
// (the Data→String door) still allocates with malloc.

/// Free a Briev string allocated by briev_cstr_to_briev, briev_bits_to_str
/// or the bitop helpers.
void briev_free_briev_str(void* handle) {
    if (handle) free(handle);
}

// 2026-08-01 (B2): The #Bit → #String ENCODING DOOR default. The bits are a
// Briev `[len: i64][bytes]` buffer (the content view of a String — see
// #String→#Bit). This re-materializes a String from those bits by copying the
// length header + payload into a fresh heap buffer. This is NOT briev_cstr_to_briev
// (which reads a null-terminated C string) — the bits carry their own length.
// The header is created by construction (copied from the bits), never inherited
// by aliasing. Returns a heap [len][bytes] String; caller frees via
// briev_free_briev_str. Sub-protocols override the lane via CastFrom(#Bit).
char* briev_bits_to_str(const char* bits) {
    if (!bits) return 0;
    int64_t len = *(const int64_t*)bits;
    if (len < 0 || len > 1024 * 1024 * 1024) return 0; // sanity
    char* buf = (char*)malloc((size_t)(len + 9));
    if (!buf) return 0;
    *(int64_t*)buf = len;
    if (len > 0) memcpy(buf + 8, bits + 8, (size_t)len);
    buf[8 + len] = '\0';
    return buf;
}

// 2026-08-01 (B1): Content bitwise ops for Briev String values (String ABI =
// ptr to [len: i64][bytes]). The result is a NEW heap buffer with the same
// length and the per-byte op applied to the payloads (band/bor/bxor) or to a
// single payload (bnot). Length must match for binary ops (asserted by the
// compiler; a mismatch returns the empty string defensively). Caller frees via
// briev_free_briev_str. These are the runtime half of the #String bitwise
// defaults; the compiler emits a call instead of treating String ptrs as ints.
static char* briev_str_bitop(const char* a, const char* b, int op) {
    if (!a) return 0;
    int64_t la = *(const int64_t*)a;
    int64_t lb = b ? *(const int64_t*)b : 0;
    if (op != 3 && la != lb) return 0;  // binary ops need equal length
    if (la < 0) return 0;
    char* out = (char*)malloc((size_t)(la + 8 + 1));
    if (!out) return 0;
    *(int64_t*)out = la;
    for (int64_t i = 0; i < la; i++) {
        unsigned char x = (unsigned char)a[8 + i];
        unsigned char y = b ? (unsigned char)b[8 + i] : 0;
        switch (op) {
            case 0: out[8 + i] = (char)(x & y); break;
            case 1: out[8 + i] = (char)(x | y); break;
            case 2: out[8 + i] = (char)(x ^ y); break;
            default: out[8 + i] = (char)(~x); break;
        }
    }
    out[8 + la] = '\0';
    return out;
}
char* briev_str_band(const char* a, const char* b) { return briev_str_bitop(a, b, 0); }
char* briev_str_bor(const char* a, const char* b)  { return briev_str_bitop(a, b, 1); }
char* briev_str_bxor(const char* a, const char* b) { return briev_str_bitop(a, b, 2); }
char* briev_str_bnot(const char* a)                { return briev_str_bitop(a, 0, 3); }

// ── CLI argv capture (Phase 3, 2026-08-01) ────────────────────────────
// The compiler's emitted `main(i32 %argc, ptr %argv)` stores its arguments
// into these module globals (see emit_main_header); the helpers below read
// them. String results follow the String ABI = ptr to [len: i64][bytes]
// (heap-allocated; caller frees via briev_free_briev_str).
// The compiler's emitted `main(i32 %argc, ptr %argv)` stores its arguments
// into these globals (see emit_main_header) — the compiler OWNS them, so the
// runtime declares them extern (not defines them). String results follow the
// String ABI = ptr to [len: i64][bytes] (heap-allocated; caller frees via
// briev_free_briev_str).
extern int32_t __briev_argc;
extern void* __briev_argv;


// 2026-08-09 (Phase 12, SPEC §19.3): `feature.^^Available` — a compile-time
// descriptor reflect that folds to a runtime symbol-availability check. An
// `optional frgn` may be missing at link time; the check tells the program
// whether the foreign symbol resolves (via dlsym on the caller's image).
// 1 = available, 0 = not. Tolerates platforms without dlfcn (returns 1 —
// the symbol is assumed present, matching a non-optional link).
int64_t briev_symbol_available(const char* symbol) {
#ifdef RTLD_DEFAULT
    void* handle = dlopen(NULL, RTLD_LAZY);
    if (!handle) {
        return 0;
    }
    void* addr = dlsym(handle, symbol);
    dlclose(handle);
    return addr != NULL ? 1 : 0;
#else
    (void)symbol;
    return 1;
#endif
}






// ── Core intrinsics (kept) ────────────────────────────────────────────




int64_t briev_syscall(int64_t num, int64_t a1, int64_t a2, int64_t a3, int64_t a4, int64_t a5, int64_t a6) {
    return syscall((long)num, (long)a1, (long)a2, (long)a3, (long)a4, (long)a5, (long)a6);
}

int64_t briev_sysconf(int64_t name) {
    return sysconf((int)name);
}

// Print# routes Float64 values here. Flush before returning so output stays
// ordered with Briev's direct writes and survives its raw-syscall endprogram.
__attribute__((always_inline)) int64_t __print_float64(double f) {
    printf("%.9g", f);
    fflush(stdout);
    return 0;
}




// 2026-07-25: ShellCmd# runtime implementation. 2026-11-19: share the
// byte-preserving capture and checked growth used by SpawnWithOutput#.
// Expected LLVM signature: call i64 @ShellCmd(i64 %cmd_bstr).
uint8_t* __briev_spawn_output(const uint8_t* cmd);
int64_t ShellCmd(int64_t cmd_bstr) {
    const uint8_t* command = (const uint8_t*)(uintptr_t)(cmd_bstr & ~3ULL);
    return (int64_t)(uintptr_t)__briev_spawn_output(command + 8);
}

/* ── Install-time host services (Plan 1 HCALL slice, 2026-08-23) ────────
 * Called by the SELF-HOSTED tamer's host dispatch (lib/tamer/vm.bv
 * exec_op 0x71) when a packaged user program issues an HCALL. Ids are the
 * canonical ones from src/backend/vm/mod.rs (canonical_host_id).
 * To undo: remove these two fns + the frgn lines in lib/tamer/main.bv. */

void briev_host_print_int(long long v) {
    printf("%lld\n", v);
    fflush(stdout);
}

/* Unknown/unsupported host service: loud failure, never silent. */
long long briev_host_fail(long long id, long long arg) {
    fprintf(stderr, "tamer: user program called unsupported host service "
                    "id %lld (arg %lld). This archive needs a newer tamer.\n",
            id, arg);
    exit(4);
    return -1;
}

/* Host-table lookup — C owns the parsed table (install_sim.c fills it via
 * briev_host_table_set); the Briev interpreter asks by id. Linear scan;
 * tables are tiny. Returns arity, or -1 when the id is unknown. */
static long long g_host_ids[64];
static long long g_host_arities[64];
static long long g_host_count = 0;

void briev_host_table_set(long long idx, long long id, long long arity) {
    if (idx < 0 || idx >= 64) return;
    g_host_ids[idx] = id;
    g_host_arities[idx] = arity;
    if (idx + 1 > g_host_count) g_host_count = idx + 1;
}

long long briev_host_arity_of(long long id) {
    for (long long i = 0; i < g_host_count; i++) {
        if (g_host_ids[i] == id) return g_host_arities[i];
    }
    return -1;
}

// ── 2026-08-23 (process.bv revival): process/environment intrinsics ────
// Exit-code convention: 0 = success, nonzero = failure (errno-ish).

int64_t __briev_spawn(const uint8_t* cmd) {
    int status = system((const char*)cmd);
    if (status == -1) return -1;
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    return -1;
}

// 2026-11-19: a process capture is an owned [i64 length][bytes][NUL] String,
// including an empty capture. Never return a C-string pointer or a static
// fallback that callers could mistake for an owned String. Allocation failure
// aborts rather than returning an invalid String; I/O/start failures yield empty.
static uint8_t* briev_empty_capture(void) {
    uint8_t* result = (uint8_t*)calloc(9, 1);
    if (!result) abort();
    return result;
}

uint8_t* __briev_spawn_output(const uint8_t* cmd) {
    FILE* fp = popen((const char*)cmd, "r");
    if (!fp) return briev_empty_capture();
    size_t cap = 4096, len = 0;
    uint8_t* buf = (uint8_t*)malloc(cap + 9);
    if (!buf) { pclose(fp); abort(); }
    size_t n;
    while ((n = fread(buf + 8 + len, 1, cap - len, fp)) > 0) {
        len += n;
        if (len != cap) continue;
        if (cap > (SIZE_MAX - 9) / 2 || cap > INT64_MAX / 2) {
            free(buf); pclose(fp); abort();
        }
        size_t next_cap = cap * 2;
        uint8_t* grown = (uint8_t*)realloc(buf, next_cap + 9);
        if (!grown) { free(buf); pclose(fp); abort(); }
        buf = grown;
        cap = next_cap;
    }
    int failed = ferror(fp);
    pclose(fp);
    if (failed) { free(buf); return briev_empty_capture(); }
    *(int64_t*)buf = (int64_t)len;
    buf[8 + len] = 0;
    return buf;
}

int64_t __briev_setenv(const uint8_t* k, const uint8_t* v) {
    return (int64_t)setenv((const char*)k, (const char*)v, 1);
}


// 2026-09-28 (Family H cleanup): the C task/event machine (async Phase C/D,
// briev_task_spawn/cancel/await + briev_event_alloc/read/fire/ready/
// strict_trap, ~200 lines) was DELETED. Family H (7a3b821d) migrated the
// scheduler to pure-Briev twins in lib/std/cast_lanes.bv over the backend-
// owned @__briev_sched / @__briev_events head globals (TaskCall# dispatch);
// the C bodies had no remaining callers. The stale C-symbol declares in
// emit_task_runtime were deleted in the same change.

/* 2026-11-12: request-scoped coll backing store. Generic `coll_resize` now
 * calls `coll_alloc` (lib/std/cast_lanes.bv); include the arena so every
 * runtime link (compiler, harness, tests) defines it. Inert until a caller
 * opens a request (`coll_arena_begin`). */
#include "../std/c/coll_arena.c"

/* ── 2026-11-16 (per-worker arena plan, Stage 2a): explicit worker threads ──
 *
 * A program that declares `on thread <N>` runs each worker's reactive slice in
 * its own OS thread. The compiler emits one tick function per worker
 * (`@reactor_tick` for worker 0, `@reactor_tick_w<N>` for the rest) and calls
 * this pool once per tick:
 *
 *   briev_workers_start(count, table, state)   — once, before the loop
 *   briev_workers_release()                    — main: wait for the tick
 * barrier
 *   ... main runs worker 0's tick itself ...
 *   briev_workers_wait()                       — main: wait for the tick to
 * finish briev_workers_stop()                       — at .end
 *
 * The two barriers give the memory ordering for the whole tick: every worker's
 * writes to `%State` are visible to the others after the `done` barrier, and no
 * worker starts a tick before the previous one finished everywhere. That is why
 * no per-tick fences are emitted.
 *
 * Soundness precondition: the compiler refuses a pinned program whose workers
 * are not provably independent (distinct globals, at most one observable sink,
 * at most one allocating worker while the arena cursor is shared). This file
 * provides the mechanism; the frontend owns the proof.
 */
#define BRIEV_MAX_WORKERS 63

static pthread_barrier_t briev_worker_release_barrier;
static pthread_barrier_t briev_worker_done_barrier;
static long briev_worker_count;
static int briev_worker_stop;
static void (*briev_worker_fn[BRIEV_MAX_WORKERS])(void *);
static void *briev_worker_state;
static pthread_t briev_worker_thread[BRIEV_MAX_WORKERS];

static void *briev_worker_main(void *arg) {
  long index = (long)arg;
  for (;;) {
    pthread_barrier_wait(&briev_worker_release_barrier);
    if (briev_worker_stop) {
      return 0;
    }
    void (*fn)(void *) = briev_worker_fn[index];
    if (fn) {
      fn(briev_worker_state);
    }
    pthread_barrier_wait(&briev_worker_done_barrier);
  }
}

/* Start `count` workers (worker 0 is the calling thread). `table` is the
 * compiler-emitted function-pointer array; entry 0 is unused. Returns 0 when a
 * pool was started, 1 when the program needs no threads (count <= 1). */
int64_t briev_workers_start(int64_t count, void *table, void *state) {
  if (count <= 1 || count > BRIEV_MAX_WORKERS) {
    return 1;
  }
  void **fns = (void **)table;
  briev_worker_count = count;
  briev_worker_stop = 0;
  briev_worker_state = state;
  for (long i = 0; i < count; i++) {
    briev_worker_fn[i] = (void (*)(void *))fns[i];
  }
  pthread_barrier_init(&briev_worker_release_barrier, 0, (unsigned)count);
  pthread_barrier_init(&briev_worker_done_barrier, 0, (unsigned)count);
  for (long i = 1; i < count; i++) {
    if (pthread_create(&briev_worker_thread[i], 0, briev_worker_main,
                       (void *)i) != 0) {
      /* Fall back to serial execution: mark the worker absent so main
       * still balances the barriers, and let its slice run inline is NOT
       * possible — the frontend proved independence, not order. Refuse
       * loudly instead of running a wrong program. */
      fprintf(stderr, "briev: worker thread %ld could not start\n", i);
      exit(1);
    }
  }
  return 0;
}

/* Main's side of the release barrier: every worker starts its tick now. */
void briev_workers_release(void) {
  if (briev_worker_count <= 1) {
    return;
  }
  pthread_barrier_wait(&briev_worker_release_barrier);
}

/* Main's side of the done barrier: every worker finished this tick. */
void briev_workers_wait(void) {
  if (briev_worker_count <= 1) {
    return;
  }
  pthread_barrier_wait(&briev_worker_done_barrier);
}

/* Release the workers and join them. Called once, at program end. */
void briev_workers_stop(void) {
  if (briev_worker_count <= 1) {
    return;
  }
  briev_worker_stop = 1;
  pthread_barrier_wait(&briev_worker_release_barrier);
  for (long i = 1; i < briev_worker_count; i++) {
    pthread_join(briev_worker_thread[i], 0);
  }
  briev_worker_count = 0;
}
