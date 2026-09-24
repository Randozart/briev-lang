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

// 2026-08-01 (audit): the Print# convenience intrinsic routes Float64 (double)
// values here — %.9g round-trips any double uniquely (~17 sig digits for the
// mantissa+exponent range, more than enough for a canonical print).
__attribute__((always_inline)) int64_t __print_float64(double f) {
    printf("%.9g", f);
    return 0;
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

// 2026-08-01 (D2): garbage-scheduling calibration. The scheduler's scheduled
// frees route through __briev_free so a benchmark can assert frees == allocs
// (no premature free, no leak). __briev_free_count() is the observable getter.
static long __briev_free_total = 0;

// 2026-08-01 (D2): `Now#` — monotonic clock in nanoseconds, for the watchdog
// `within N ms` deadline compare (the deadline is `now - start >= N ms`).
#include <time.h>

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
