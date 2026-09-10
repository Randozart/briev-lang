# Briev-Native Runtime

**2026-09-10** · Status: ACTIVE (families A–E landed on `feat/briev-native-runtime`)
**Plan:** `docs/plans/2026-09-09-briev-native-runtime-and-family-realignment.md`
**Gate:** `bash benchmarks/parity/run.sh` — 14/14 corpora byte-identical

## The model

Briev's default runtime is Briev-native: pure-Briev stdlib + `SysCall#`
inline asm + LLVM-level emission. No `briev_rt.c`, no libc required to
build or run a program. C remains available *to the user* through
`frgn ... from #System` — never a compiler dependency.

One runtime serves every target. Freestanding (bare-metal) is a target
configuration (triple-driven), not a source variant; the allocator is the
only behavioral fork (hosted grows via `brk`, freestanding is fixed).

## Perf report: the brk arena beats the C runtime 2.5×

**Result: arena_churn at BOUND=50M — Briev-native 33–42ms vs the
C-runtime baseline 89–110ms (2.5× faster), byte-identical output.**

| Build | Arena path | 3 runs (ms) |
|---|---|---|
| Baseline compiler (`briev-compiler-baseline`) | libc `malloc` init + `realloc` grow (copies the whole region every grow) | 92, 110, 89 |
| Briev-native (`9b73499a`) | inline `brk` syscall init + in-place extend | 42, 37, 33 |

### Why it wins

1. **Zero-copy growth.** `realloc` moves the region and memcpy's every live
   byte on each grow. The brk arena EXTENDS IN PLACE: `brk(end + min_sz)`
   moves the break; the base never moves; nothing is copied. Growth cost
   drops from O(live bytes) to O(1).
2. **No call overhead on the hot path.** The bump itself was already
   inline state-field IR (the compiler's optimum — untouched); only init
   and the cold grow path changed.
3. **No libc tax.** The old path pulled libc's malloc/realloc machinery
   into every link; the new path is a two-instruction syscall sequence.

This is the expressiveness-closure thesis (see
`briev-capability-frontier.md`) paying off concretely: the "optimal
allocator path" the compiler now emits is expressible in the language's own
primitives — and the measured optimum improved when the libc layer was
removed rather than trusted.

### The cost of the gate: the int_to_str lesson

The arena_churn gate caught a crash the correctness suites missed.
`int_to_str` was written as a concat chain — `int_to_str(hi) + digit_char(lo)`.
Every `+` result is STATE-ARENA memory, but the String-concat epilogue
`free()`s operands tagged heap — handing an arena pointer to libc `free()`.
Invisible while `int_to_str` was C (malloc-on-malloc, consistent); fatal
the moment the lane became a Briev defn.

Fix: digits are built INTO an `Alloc#` buffer (two-pass count + backward
fill, `srem`-sign-safe for INT64_MIN) — arena memory is never freed. The
deeper hazard is documented: **any Briev defn that concats in a loop owns
the concat's free-flag assumptions.** The concat emitter's tagging vs
arena allocation is a follow-up (see plan §2.3 follow-ups).

## Landed families (A–E)

| Family | Symbols | Form |
|---|---|---|
| A — cast lanes | `int_to_str`, `uint_to_str`, `float_to_str` (%g), `bool_to_str`, `char_to_str` (+UTF8 encode), `str_to_int/uint` (strtol + i64 clamp), `str_to_bool`, `str_first_char` (UTF8), `str_to_float` (strtod-parity: divide by exact powers of ten), `byte_at` | Briev defns, `cast_lanes.bv` |
| B — print/exit | `__print`, `__print_str`, `__eprint_str` (write(2)), `__print_int/bool` (lanes), `__print_float` (%.9g = `float_format`), `__print_char` (1-byte write), `__exit` (SYS_exit_group) | Briev defns |
| C — string ops | `briev_str_eq`, `briev_str_substr` (Copy#), `briev_str_next_char` (UTF8 walk), `briev_char_len` (lead-byte count) | Briev defns |
| D — vector gathers | `briev_mask_select` ×5 variants, `briev_slice_range64/_f32`, `__briev_coll_resize` (arena semantics: grow = Alloc#+Copy#, no free) | Briev defns (dynamic-shape — not shufflevector-able) |
| E — allocator | arena init + grow as inline brk syscalls; `float_format` engine (Ryu-class %.9g/%g) in `float_fmt.bv` | Backend inline + Briev defns |
| I — misc | `__briev_now` (clock_gettime syscall — watchdog-hot), `__watchdog_fail` (write + exit_group), `__briev_getcwd` (SYS_getcwd, C-string ABI preserved), `__briev_chdir`, `__briev_free` (arena no-op) | Briev defns |
| G — tty/timerfd | 13 symbols DELETED — all stubs, zero referencers (timerfd/signalfd are SysCall#-expressible if wanted later; ttyname is a user-facing #System frgn) | deleted |
| H — async | pthread pool + barriers + wait stubs DELETED (~130 lines); `emit_async_phase` emits direct sequential `@async_body_*` calls + reactor_tick (deterministic order); `__wait_for_trigger__` reborn as a sched_yield defn | Backend inline + Briev defns |

`briev_rt.c` has shrunk from 1407 to ~800 lines. Still unmigrated:
argv/env (blocked on the environ-ownership + `_start` entry design), the
task/event machine (~170 lines, frgn-called; flatten the task table to
Int arrays + `CallPtr#` segment dispatch), process/spawn + `ShellCmd`
(popen → fork/pipe/execve), `__briev_setenv` (libc env-block mutation —
environ ownership), `briev_symbol_available` (dlsym), Tamer HCALL, and
the GLUE C-ABI doors (`briev_str_to_c`, `briev_cstr_to_briev`,
`briev_bits_to_str` — the Data→String door).

**Confirmed pre-existing bug (queued):** async convergence never exits —
`program_convergence` produces no counter_ge_bounds for async txns, so
bounded async programs spin in the idle-wait branch forever (bisected to
the branch base; independent of the pool-vs-cooperative substrate). Fix:
register async (counter, bound) pairs in the convergence analysis; the
idle-wait branch then becomes unreachable for bounded async programs.

## Known follow-ups

1. **Concat free-flag vs arena tagging** — the concat emitter's operand
   free flags assume libc-heap provenance; arena-provenance results must
   never be tagged freeable. (The int_to_str lesson, generalized.)
2. **Tier-box/env/heap-seq `@malloc` sites** — ~12 emission sites still
   call libc malloc directly (List boxing, closure envs, struct allocas).
   Migrating them needs the one-arena-header design (the bump state must
   live in the heap, shareable between backend IR and stdlib defns — two
   independent brk cursors are impossible).
3. **`float_to_str` %g precision > corpus** — the corpus pins %.9g and the
   common %g cases; exotic %g widths are follow-up corpus work.
4. **Arch coverage** — syscall numbers are x86_64 (aarch64 templates
   drafted); the language has no arch reflection yet.
5. **Family H (async)** — the pthread pool → cooperative run-queue is the
   remaining large design piece before the delete step.

## Family H design: async without pthreads (survey complete, design)

Survey findings (2026-09-10):

1. **The pool is live.** `async node` programs emit
   `__thread_pool_init__(N, @thread_pool_fns)` — two workers for
   async_counters. The `is_lightweight_async` path (per-txn const-bounded
   preconditions) skips the pool, but general async does not.
2. **The task/event machine is ALREADY cooperative** — segmented
   continuations with a round-robin `briev_await` scheduler, single-threaded
   by construction. Only the worker pool is parallel.
3. **The pool's protocol is embarrassingly sequential per tick:**
   `__set_async_state__(s)` → `__barrier_release__()` (workers run their
   whole body once) → `reactor_tick(s)` → `__barrier_wait__()`. Workers run
   their body TO COMPLETION each tick — there is no cross-tick worker
   state.

### The design: cooperative inline emission

Replace the three phase calls with DIRECT sequential body calls emitted by
the backend (`emit_async_phase`, loop_engine pool-init site):

```
call void @async_body_0(ptr %state)   ; was: __barrier_release__ (workers)
call void @async_body_1(ptr %state)
call void @reactor_tick(ptr %state)   ; unchanged
; __barrier_wait__ deleted (no one to wait for)
```

- `@thread_pool_fns` and `__thread_pool_init__` disappear (the backend
  knows the body names — it built the table).
- Output parity: the pthread version's stdout interleaving is RACY
  (worker prints race); the cooperative form is deterministic per tick.
  async_counters-style parity gates must compare per-counter value
  sequences, not raw line order (note for the harness).
- Throughput: true multicore parallelism on `async` bodies is LOST —
  documented (plan §6.4); the follow-on substrate is
  `SysCall#(SYS_clone)` + futex with the same release/wait protocol.
- `__wait_for_trigger__`/`__rt_wait`/`__rt_poll` (pause() stubs) become
  no-ops/deleted with the family.
- The task/event machine (`briev_task_spawn/await/event_*`) is
  single-threaded already — it migrates to Briev defns over a flattened
  Int-array task table with `CallPtr#` segment dispatch (mechanical once
  attempted; the segment tables are fn-ptr arrays the backend already
  emits).

### Sequencing

The cooperative rewrite unblocks the `briev_rt.c` delete step for the
async half (pool + barriers + wait stubs ≈ 130 lines). The task/event
machine migration (~170 lines) can follow independently — it is
frgn-called from async `.bv` programs, not backend-emitted.

### CONFIRMED pre-existing bug: async convergence never exits

Bisected to the branch base (2d3112a7, pre-A+B): a two-`async node`
program with exit conditions (`[a < N][a == N]`) runs correctly but NEVER
terminates — prints land, then the loop spins in the
`__wait_for_trigger__` branch forever. Root cause located:
`program_convergence` (analysis/loop_shape.rs) produces no
`counter_ge_bounds` for async txns, so `ctx.exit_condition` stays `None`
and the loop-emission falls into the idle-wait branch. The reactor ticks
the bodies correctly (values converge — N is reached) but the exit
predicate is never installed.

Fix path (Family H implementation work): register async txn
(counter, bound) pairs in the convergence analysis exactly as bounded
txns do, then the `has_exit_cond` branch emits the real predicate and
the idle-wait branch becomes unreachable for bounded async programs.
