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

// ── String conversion helpers (internal) ──────────────────────────────
// 2026-08-01 (B0): A Briev String value is a ptr to a length-prefixed
// [len][bytes] buffer. The old int64_t "handle" params were the address in
// disguise; they are now typed as pointers so clang's IR (ptr) matches the
// compiler's `ptr`-based frgn declares (String ABI = ptr). int64_t and
// pointers are ABI-identical on x86-64 — this is a typing change only.
char* briev_str_to_c(const char* handle) {
    // 2026-07-22: Strip tag bits (bottom 2 bits) — they mark temporary
    // flags (bit 0 = SSO inline, bit 1 = temporary concat result).
    // After stripping, we have the raw data pointer.
    uintptr_t u = (uintptr_t)handle;
    uintptr_t ptr = u & ~3ULL;
    if (u & 1) {
        // SSO string — inline data packed at bits >= 3
        // The LLVM SSO encoding: handle0 = (raw_data << 3) | 1
        // where raw_data has bytes packed in LE order.
        uint64_t raw_data = ((uint64_t)u) >> 3;
        int64_t len = 0;
        for (int i = 0; i < 6; i++) {
            if ((raw_data >> (i * 8)) & 0xFF) len = i + 1;
        }
        if (len == 0) len = 1;
        char* c_str = malloc((size_t)(len + 1));
        if (!c_str) return NULL;
        for (int64_t i = 0; i < len; i++) {
            c_str[i] = (char)((raw_data >> (i * 8)) & 0xFF);
        }
        c_str[len] = '\0';
        return c_str;
    }
    // 2026-08-03 (plan 2026-08-03-native-python-meld-composite): the fragile
    // "looks like a C string" heuristic was REMOVED — it misread any Briev
    // String whose length byte is printable ASCII (a 35-char path reads as
    // '$' → the [len][bytes] header was strlened as a bare C string). Under
    // the composite, every String value IS a [len][bytes][\0] Briev String
    // (CStr → String is marshalled through cstr_to_briev at the boundary), so
    // str_to_c only ever sees the heap form below. A bare C string passed to a
    // String-typed site is a programming error, not a runtime case to guess.
    // Heap Briev string: ptr is a pointer to [8-byte length][data][\0].
    // 2026-08-03 (plan 2026-08-03-native-python-meld-composite): every Briev
    // String allocation carries the NUL invariant (bytes[len] == '\0'), so
    // the data region IS a valid C string in place — return it directly
    // (zero-copy, the composite). Caller must NOT free; valid for the state's
    // life (the composite ABI contract). Previously this malloc'd a copy (a
    // leak for C drivers that never freed it).
    if (ptr == 0) return NULL;
    int64_t len = *(int64_t*)ptr;
    if (len < 0 || len > 1024 * 1024 * 1024) return NULL;
    return (char*)(ptr + 8);
}

/// Convert a C string (null-terminated) to a Briev string.
/// Returns a heap-allocated Briev string (8-byte length prefix + data).
/// Caller should free via briev_free_briev_str().
char* briev_cstr_to_briev(const char* c_str) {
    if (!c_str) return 0;
    int64_t len = (int64_t)strlen(c_str);
    if (len > 1024 * 1024 * 1024) return 0; // sanity check
    // Allocate: 8 bytes for length + len bytes for data + 1 for null terminator
    char* buf = (char*)malloc((size_t)(len + 9));
    if (!buf) return 0;
    *(int64_t*)buf = len;               // write length prefix
    if (len > 0) memcpy(buf + 8, c_str, (size_t)len);
    buf[8 + len] = '\0';                // null terminator for C compatibility
    return buf;
}

/// Free a Briev string allocated by briev_cstr_to_briev or similar.
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

int64_t __argv_count(void) {
    return (int64_t)__briev_argc;
}

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

// argv[i] as a Briev string (empty for out-of-range i).
char* __argv_get(int64_t i) {
    if (!__briev_argv || i < 0 || i >= __briev_argc) {
        return briev_cstr_to_briev("");
    }
    char* s = ((char**)__briev_argv)[i];
    return briev_cstr_to_briev(s);
}

// Whether any argv token equals `flag` (a Briev string). Returns 1/0.
// Skips argv[0] (the program name) — flags/commands live in argv[1..].
int64_t __argv_has(const char* flag_bstr) {
    char* c_flag = briev_str_to_c(flag_bstr);
    if (!c_flag) return 0;
    int64_t found = 0;
    if (__briev_argv) {
        for (int64_t i = 1; i < __briev_argc; i++) {
            if (strcmp(((char**)__briev_argv)[i], c_flag) == 0) {
                found = 1;
                break;
            }
        }
    }
    free(c_flag);
    return found;
}

// The value following `flag` (e.g. `--out file` → "file"), or "" if absent.
char* __argv_value(const char* flag_bstr) {
    char* c_flag = briev_str_to_c(flag_bstr);
    if (!c_flag) return briev_cstr_to_briev("");
    char* result = NULL;
    if (__briev_argv) {
        for (int64_t i = 1; i + 1 < __briev_argc; i++) {
            if (strcmp(((char**)__briev_argv)[i], c_flag) == 0) {
                result = ((char**)__briev_argv)[i + 1];
                break;
            }
        }
    }
    free(c_flag);
    if (!result) return briev_cstr_to_briev("");
    return briev_cstr_to_briev(result);
}

// The first non-flag token in argv[1..] — the subcommand. `<prog> --verbose
// build` → "build"; "" if none. Honors $BRIEV_ENTRY_CMD (test/embedded path
// without argv) as the sole environment fallback.
char* __argv_command(void) {
    const char* env_cmd = getenv("BRIEV_ENTRY_CMD");
    if (env_cmd && env_cmd[0]) {
        return briev_cstr_to_briev(env_cmd);
    }
    if (__briev_argv) {
        for (int64_t i = 1; i < __briev_argc; i++) {
            const char* tok = ((char**)__briev_argv)[i];
            if (tok[0] != '-') {
                return briev_cstr_to_briev(tok);
            }
        }
    }
    return briev_cstr_to_briev("");
}


// ── Core intrinsics (kept) ────────────────────────────────────────────

// 2026-07-19: Returns the environ pointer (char **environ) as an Int.
// Used by pure-Briev getenv to scan the environment block.
int64_t __get_environ(void) {
    extern char **environ;
    return (int64_t)(uintptr_t)environ;
}

// 2026-07-19: Returns the value of an env var as a heap-allocated Briev string
// (null-terminated UTF-8 data preceded by 8-byte length header).
// Caller takes ownership of the returned pointer.
// 2026-08-01 (B0): key_bstr is a ptr to a Briev [len][bytes] buffer; returns
// a ptr to the same layout (String ABI = ptr, matching the compiler declares).
char* __getenv_briev(const char* key_bstr) {
    char* c_key = briev_str_to_c(key_bstr);
    if (!c_key) return 0;
    char* val = getenv(c_key);
    if (!val) return 0;
    int64_t len = (int64_t)strlen(val);
    char* bstr = (char*)malloc((size_t)(len + 8 + 1));
    if (!bstr) return 0;
    *(int64_t*)bstr = len;
    memcpy(bstr + 8, val, (size_t)len);
    bstr[8 + len] = '\0';
    return bstr;
}

// 2026-07-19: Returns the value of an env var parsed as Int.
int64_t __getenv_int(const char* key_bstr) {
    char* c_key = briev_str_to_c(key_bstr);
    if (!c_key) return 0;
    char* val = getenv(c_key);
    if (!val) return 0;
    return atol(val);
}

int64_t briev_syscall(int64_t num, int64_t a1, int64_t a2, int64_t a3, int64_t a4, int64_t a5, int64_t a6) {
    return syscall((long)num, (long)a1, (long)a2, (long)a3, (long)a4, (long)a5, (long)a6);
}

int64_t briev_sysconf(int64_t name) {
    return sysconf((int)name);
}

// 2026-08-01 (audit): the Print# convenience intrinsic routes Float64 (double)
// values here — %.9g round-trips any double uniquely (~17 sig digits for the
// mantissa+exponent range, more than enough for a canonical print).
__attribute__((always_inline)) int64_t __print_float64(double f) {
    printf("%.9g", f);
    return 0;
}

// ── Timer infrastructure (used by trigger system) ─────────────────────

int32_t __trg_timerfd_open(int64_t hz) {
    return 0;
}

int32_t __trg_timerfd_read(int32_t fd) {
    (void)fd;
    return 0;
}

int32_t __trg_signalfd_open(const char* name) {
    (void)name;
    return 0;
}

int32_t __trg_signalfd_read(int32_t fd) {
    (void)fd;
    return 0;
}

// ── Async runtime infrastructure ─────────────────────────────────────
// Forward declaration for atexit cleanup handler
void briev_thread_pool_shutdown(void);
// 2026-07-17: Real thread pool implementation using pthreads.
// Protocol:
//   1. __thread_pool_init__ creates N worker threads, each pinned to a
//      function pointer from the fn_ptrs array.
//   2. Each tick: main calls __set_async_state__, __barrier_release__
//      (workers run their body), then reactor_tick + __barrier_wait__.
//   3. briev_thread_pool_shutdown joins all workers.

typedef struct {
    unsigned id;
    void (*fn)(void*);
} WorkerArg;

static pthread_t* workers = NULL;
static WorkerArg* worker_args = NULL;
static unsigned worker_count = 0;
static void* async_state = NULL;

static pthread_mutex_t barrier_mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t barrier_cond = PTHREAD_COND_INITIALIZER;
static volatile int barrier_phase = 0;  // 0 = wait, 1 = go
static volatile int workers_done = 0;

static void* worker_thread(void* arg) {
    WorkerArg* wa = (WorkerArg*)arg;
    while (1) {
        pthread_mutex_lock(&barrier_mutex);
        while (barrier_phase == 0) {
            pthread_cond_wait(&barrier_cond, &barrier_mutex);
        }
        // barrier_phase == 1: workers are released
        pthread_mutex_unlock(&barrier_mutex);

        // Execute the async body function with the current state
        if (wa->fn && async_state) {
            wa->fn(async_state);
        }

        // Signal completion
        pthread_mutex_lock(&barrier_mutex);
        workers_done++;
        pthread_cond_signal(&barrier_cond);
        pthread_mutex_unlock(&barrier_mutex);
    }
    return NULL;
}

// 2026-07-17: Thread pool shutdown registered as atexit handler so worker
// threads are cleaned up when main() returns.
static void __rt_cleanup(void) {
    briev_thread_pool_shutdown();
}

void __rt_init(void) {
    signal(SIGPIPE, SIG_IGN);
    atexit(__rt_cleanup);
}

void __set_async_state__(void* state) {
    async_state = state;
}

void __thread_pool_init__(unsigned num_workers, void** fn_ptrs) {
    if (num_workers == 0) return;
    worker_count = num_workers;
    workers = (pthread_t*)calloc(num_workers, sizeof(pthread_t));
    worker_args = (WorkerArg*)calloc(num_workers, sizeof(WorkerArg));
    for (unsigned i = 0; i < num_workers; i++) {
        worker_args[i].id = i;
        worker_args[i].fn = (void (*)(void*))fn_ptrs[i];
        pthread_create(&workers[i], NULL, worker_thread, &worker_args[i]);
    }
}

void __barrier_release__(void) {
    pthread_mutex_lock(&barrier_mutex);
    workers_done = 0;
    barrier_phase = 1;
    pthread_cond_broadcast(&barrier_cond);
    pthread_mutex_unlock(&barrier_mutex);
}

void __barrier_wait__(void) {
    pthread_mutex_lock(&barrier_mutex);
    while (workers_done < worker_count) {
        pthread_cond_wait(&barrier_cond, &barrier_mutex);
    }
    barrier_phase = 0;
    pthread_mutex_unlock(&barrier_mutex);
}

void briev_thread_pool_shutdown(void) {
    if (!workers) return;
    // Workers loop forever — cancel them on shutdown
    for (unsigned i = 0; i < worker_count; i++) {
        pthread_cancel(workers[i]);
        pthread_join(workers[i], NULL);
    }
    free(workers);
    free(worker_args);
    workers = NULL;
    worker_args = NULL;
    worker_count = 0;
}

void __wait_for_trigger__(void) {
    // 2026-07-15: Async event loop — wait for next trigger event
    pause();
}

// ── File I/O (used by stdlib) ─────────────────────────────────────────
// 2026-08-01 (B0): path_bstr/data_bstr are ptrs to Briev [len][bytes] buffers
// (String ABI = ptr).

int64_t __read_file__(const char* path_bstr) {
    // 2026-08-03 (P2): briev_str_to_c returns the IN-PLACE data pointer (the
    // composite) for heap Strings — arena-owned, must NOT be freed.
    char* c_path = briev_str_to_c(path_bstr);
    if (!c_path) return -1;
    FILE* f = fopen(c_path, "r");
    if (!f) return -1;
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    char* buf = malloc((size_t)(size + 1));
    if (!buf) { fclose(f); return -1; }
    size_t n = fread(buf, 1, (size_t)size, f);
    fclose(f);
    buf[n] = '\0';
    return (int64_t)(uintptr_t)buf;
}

int64_t __write_file__(const char* path_bstr, const char* data_bstr) {
    // 2026-08-03 (P2): str_to_c results are borrowed arena pointers — the old
    // free() calls freed invalid pointers (P2 made str_to_c zero-copy).
    char* c_path = briev_str_to_c(path_bstr);
    char* c_data = briev_str_to_c(data_bstr);
    if (!c_path || !c_data) return -1;
    FILE* f = fopen(c_path, "w");
    if (!f) return -1;
    size_t len = strlen(c_data);
    size_t written = fwrite(c_data, 1, len, f);
    fclose(f);
    return (int64_t)written;
}

// ── Event loop ───────────────────────────────────────────────────────

void __rt_wait(void) {
    pause();
}

void __rt_poll(void) {
    pause();
}

// ── TTY / Terminal (used by stdlib) ──────────────────────────────────

int64_t tty_raw_mode(int64_t enable) {
    (void)enable;
    return -1;
}

int64_t tty_size(void) {
    return -1;
}

int64_t __tty_raw_mode__(int64_t enable) {
    return tty_raw_mode(enable);
}

int64_t __tty_size__(void) {
    return tty_size();
}

int64_t __tty_read_key__(void) {
    return -1;
}

int64_t __readln__(void) {
    return -1;
}

int64_t __sort_list__(int64_t list_bstr) {
    (void)list_bstr;
    return -1;
}

int64_t __reverse_list__(int64_t list_bstr) {
    (void)list_bstr;
    return -1;
}

int64_t briev_ttyname(int64_t fd) {
    return (int64_t)(uintptr_t)ttyname((int)fd);
}

// 2026-07-25: ShellCmd# runtime implementation.
// Runs a shell command via popen() and returns stdout as a Briev String.
// Expected LLVM signature: call i64 @ShellCmd(i64 %cmd_bstr)
int64_t ShellCmd(int64_t cmd_bstr) {
    // Extract C string from Briev String handle
    int64_t handle = cmd_bstr & ~3ULL;  // strip tag bits
    int64_t len = *(int64_t*)(uintptr_t)handle;  // read length prefix
    char* cstr = (char*)(uintptr_t)(handle + 8);  // data starts after length
    char* buf = (char*)calloc(len + 32, 1);
    if (!buf) return 0;
    memcpy(buf, cstr, len);
    
    // Run command via popen
    FILE* f = popen(buf, "r");
    free(buf);
    if (!f) return 0;
    
    // Read output into a growing buffer
    size_t out_cap = 4096;
    size_t out_len = 0;
    char* out = (char*)malloc(out_cap);
    if (!out) { pclose(f); return 0; }
    while (fgets(out + out_len, out_cap - out_len, f) != NULL) {
        out_len = strlen(out);
        if (out_len + 1024 > out_cap) {
            out_cap *= 2;
            out = (char*)realloc(out, out_cap);
            if (!out) { pclose(f); return 0; }
        }
    }
    pclose(f);
    
    // Pack as Briev String: {i64 length, i8 data[]}
    int64_t total = 8 + out_len;
    int64_t* result = (int64_t*)malloc(total + 8);  // extra padding
    if (!result) { free(out); return 0; }
    result[0] = out_len;
    memcpy(result + 1, out, out_len);
    free(out);
    return (int64_t)(uintptr_t)result;
}

// 2026-07-18: All __utf8_* functions now implemented as pure Briev in utf8view.bv
// (uses Load# + convergent txn). Find byte substring in byte string.
// Returns offset or -1.
// (implemented in pure Briev in lib/std/types/utf8view.bv)

// 2026-08-01 (C3): required-watchdog failure exit. A `![cond]` watchdog that
// fires without an on-fire handler is a fatal program error — the loop engine
// calls this on the fire path.
void __watchdog_fail(void) {
    fprintf(stderr, "briev: required watchdog fired\n");
    exit(1);
}

// 2026-08-01 (D2): garbage-scheduling calibration. The scheduler's scheduled
// frees route through __briev_free so a benchmark can assert frees == allocs
// (no premature free, no leak). __briev_free_count() is the observable getter.
static long __briev_free_total = 0;

void __briev_free(void* p) {
    if (p) __briev_free_total++;
    free(p);
}

long __briev_free_count(void) {
    return __briev_free_total;
}

// 2026-08-01 (D2): `Now#` — monotonic clock in nanoseconds, for the watchdog
// `within N ms` deadline compare (the deadline is `now - start >= N ms`).
#include <time.h>
int64_t __briev_now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}

/// Concatenate two nul-terminated C strings into a new heap buffer.
/// 2026-08-03: the C_String sub-protocol's Concat cross-op binding — a C
/// string is not [len][data], so the generic inline concat is wrong for it.
char* briev_cstring_concat(const char* a, const char* b) {
    size_t la = a ? strlen(a) : 0;
    size_t lb = b ? strlen(b) : 0;
    char* out = (char*)malloc(la + lb + 1);
    if (!out) return NULL;
    if (la) memcpy(out, a, la);
    if (lb) memcpy(out + la, b, lb);
    out[la + lb] = '\0';
    return out;
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

uint8_t* __briev_spawn_output(const uint8_t* cmd) {
    FILE* fp = popen((const char*)cmd, "r");
    if (!fp) return NULL;
    size_t cap = 4096, len = 0;
    uint8_t* buf = (uint8_t*)malloc(cap);
    if (!buf) { pclose(fp); return NULL; }
    size_t n;
    while ((n = fread(buf + len, 1, cap - len - 1, fp)) > 0) {
        len += n;
        if (cap - len < 2) { cap *= 2; buf = (uint8_t*)realloc(buf, cap); }
    }
    pclose(fp);
    buf[len] = 0;
    return buf;
}

int64_t __briev_setenv(const uint8_t* k, const uint8_t* v) {
    return (int64_t)setenv((const char*)k, (const char*)v, 1);
}

uint8_t* __briev_getcwd(void) {
    char buf[4096];
    if (!getcwd(buf, sizeof(buf))) return NULL;
    return (uint8_t*)strdup(buf);
}

int64_t __briev_chdir(const uint8_t* p) {
    return (int64_t)chdir((const char*)p);
}

/* ════════════════════════════════════════════════════════════════════
   2026-08-26 (async Phase C, docs/plans/2026-08-26-async-phase-c-
   segmented-lowering.md): compiled task concurrency — segmented
   continuations. A task is a fixed record holding its captured args, a
   segment-function table, and a cursor. ONLY parameters carry across a
   segment boundary (reference rule, SPEC §12.2) — each segment is a plain
   function of the argv block, so no stack switching is needed.

   briev_await drives the deterministic round-robin exactly like the
   reference interpreter's Await handler: one segment per runnable task per
   pass, spawn-id order, stop when the awaited task reaches DONE; an empty
   runnable pool with the target unfinished returns the handle unchanged
   (deadlock posture — no hang).

   TEMP undo: delete this block + the backend's __task_* emission to return
   to eager-inline spawns.
   ════════════════════════════════════════════════════════════════════ */

#define BRIEV_TASK_MAX 64
#define BRIEV_TASK_MAX_ARGS 8

typedef struct {
    long long value;
    int finished;
} BrievSegOut;

typedef BrievSegOut (*BrievSegFn)(long long* argv);

enum { BR_TASK_READY = 0, BR_TASK_YIELDED = 1, BR_TASK_DONE = 2,
       BR_TASK_CANCELLED = 3, BR_TASK_WAITING = 4 };

/* The task whose segment is executing, or -1 outside any task. Set by
 * briev_await around each segment call — the compiled twin of the
 * interpreter's CURRENT_TASK thread-local (Phase D). */
static long long __briev_current_task = -1;

static struct {
    int status;
    int current_segment;
    int segment_count;
    int arg_count;
    long long args[BRIEV_TASK_MAX_ARGS];
    long long result;
    const BrievSegFn* segments;
} briev_tasks[BRIEV_TASK_MAX];

static int briev_task_next = 0;

long long briev_task_spawn(const BrievSegFn* segments, int nseg, int nargs,
                           const long long* argv) {
    if (briev_task_next >= BRIEV_TASK_MAX || nargs > BRIEV_TASK_MAX_ARGS) {
        /* Out of scheduler capacity: fail loudly rather than corrupt. */
        fprintf(stderr, "briev: task table exhausted\n");
        abort();
    }
    int id = briev_task_next++;
    briev_tasks[id].status = BR_TASK_READY;
    briev_tasks[id].current_segment = 0;
    briev_tasks[id].segment_count = nseg;
    briev_tasks[id].arg_count = nargs;
    briev_tasks[id].result = 0;
    briev_tasks[id].segments = segments;
    for (int i = 0; i < nargs; i++) briev_tasks[id].args[i] = argv[i];
    return (long long)id;
}

void briev_task_cancel(long long handle) {
    if (handle < 0 || handle >= briev_task_next) return;
    int st = briev_tasks[handle].status;
    if (st != BR_TASK_DONE && st != BR_TASK_CANCELLED) {
        briev_tasks[handle].status = BR_TASK_CANCELLED;
    }
}

static int briev_task_done(long long h) {
    return h >= 0 && h < briev_task_next &&
           briev_tasks[h].status == BR_TASK_DONE;
}

long long briev_await(long long target) {
    while (!briev_task_done(target)) {
        int ran_any = 0;
        for (int id = 0; id < briev_task_next; id++) {
            int st = briev_tasks[id].status;
            if (st != BR_TASK_READY && st != BR_TASK_YIELDED) continue;
            if (briev_tasks[id].current_segment >= briev_tasks[id].segment_count)
                continue;
            ran_any = 1;
            __briev_current_task = id;
            BrievSegOut out = briev_tasks[id].segments[
                briev_tasks[id].current_segment](briev_tasks[id].args);
            __briev_current_task = -1;
            if (out.finished == 1) {
                briev_tasks[id].status = BR_TASK_DONE;
                briev_tasks[id].result = out.value;
                if ((long long)id == target) goto done;
            } else if (out.finished == 2) {
                /* BLOCKED on an unready port: the cursor does NOT advance —
                 * the read heads its segment (registration presplit), so the
                 * post-wake re-run starts AT the read. The waiter is already
                 * registered; a fire flips status back to READY. */
                briev_tasks[id].status = BR_TASK_WAITING;
            } else {
                briev_tasks[id].current_segment++;
                briev_tasks[id].status = BR_TASK_YIELDED;
            }
        }
        /* Empty runnable pool with the target unfinished: deadlock posture
         * — return the handle instead of hanging (cooperative scheduler,
         * no preemption). */
        if (!ran_any) break;
    }
done:
    if (briev_task_done(target)) return briev_tasks[target].result;
    return target;
}

/* ════════════════════════════════════════════════════════════════════
   2026-08-26 (async Phase D, docs/plans/2026-08-26-async-phase-d-
   compiled-ports.md): compiled event ports. A wire is an i64 handle to a
   one-slot event record; firing stores the payload and wakes blocked
   tasks. Reads inside a task BLOCK when unready (level-triggered, SPEC
   §9.5/§12.2) — the segment re-runs from its head after wake, which is
   exact-once by construction because registration presplits segments so
   the read heads its own.
   ════════════════════════════════════════════════════════════════════ */

#define BRIEV_EVENT_MAX 128
#define BRIEV_EVENT_WAITERS 8

static struct {
    int ready;
    long long payload;
    int waiters[BRIEV_EVENT_WAITERS];
    int nwaiters;
} briev_events[BRIEV_EVENT_MAX];

static int briev_event_next = 0;

long long briev_event_alloc(void) {
    if (briev_event_next >= BRIEV_EVENT_MAX) {
        fprintf(stderr, "briev: event table exhausted\n");
        abort();
    }
    int id = briev_event_next++;
    briev_events[id].ready = 0;
    briev_events[id].payload = 0;
    briev_events[id].nwaiters = 0;
    return (long long)id;
}

static void briev_task_mark_waiting(long long tid) {
    if (tid < 0 || tid >= briev_task_next) return;
    if (briev_tasks[tid].status == BR_TASK_READY ||
        briev_tasks[tid].status == BR_TASK_YIELDED) {
        briev_tasks[tid].status = BR_TASK_WAITING;
    }
}

int briev_event_read(long long slot, long long* out) {
    if (slot < 0 || slot >= briev_event_next) return -1;
    if (briev_events[slot].ready) {
        *out = briev_events[slot].payload;
        return 1;
    }
    /* Unready INSIDE a task: block — register as waiter, mark WAITING.
     * Outside a task this returns 2 (strict-gate violation); the caller
     * emitted code traps on it (top-level reads gate on .^Ready). */
    long long tid = __briev_current_task;
    if (tid < 0) return 2;
    if (briev_events[slot].nwaiters < BRIEV_EVENT_WAITERS) {
        briev_events[slot].waiters[briev_events[slot].nwaiters++] = (int)tid;
    }
    briev_task_mark_waiting(tid);
    return 0;
}

void briev_event_fire(long long slot, long long payload) {
    if (slot < 0 || slot >= briev_event_next) return;
    briev_events[slot].ready = 1;
    briev_events[slot].payload = payload;
    /* Drain waiters: every still-WAITING task becomes schedulable again.
     * Cancelled/Done ids are skipped — a freed task never resurrects. */
    for (int i = 0; i < briev_events[slot].nwaiters; i++) {
        long long tid = briev_events[slot].waiters[i];
        if (tid < 0 || tid >= briev_task_next) continue;
        if (briev_tasks[tid].status == BR_TASK_WAITING) {
            briev_tasks[tid].status = BR_TASK_READY;
        }
    }
    briev_events[slot].nwaiters = 0;
}

int briev_event_ready(long long slot) {
    if (slot < 0 || slot >= briev_event_next) return 0;
    return briev_events[slot].ready;
}

/* Phase D: a port read fired outside any task on an unready wire — the
 * compiled form of the interpreter's strict top-level error (SPEC §9.5:
 * gate top-level reads with .^Ready). */
void briev_event_strict_trap(void) {
    fprintf(stderr,
        "briev: read of an unready event port outside a task — "
        "gate the read with .^Ready\n");
    abort();
}
