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

`briev_rt.c` has shrunk from 1407 lines to the still-unmigrated families:
argv/env, TTY/timer/trigger, the pthread async pool + task/event machine,
process/spawn, Tamer HCALL, and the GLUE C-ABI doors (`briev_str_to_c`,
`briev_cstr_to_briev`, `briev_bits_to_str` — the Data→String door).

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
