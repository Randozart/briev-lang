# Briev-native runtime, Electronics Briev, and family realignment

**Date:** 2026-09-09
**Status:** PLANNED — authoritative plan for the Briev family overhaul
**Supersedes:** the `.ebv`-embedded / `.cbv`-circuit extension model described in
`spec/SPEC.md` §3 and `docs/architecture/backend-strategy.md` (updated in this
same work stream).

---

## 0. Summary

Briev's default runtime becomes **Briev-native**: the compiler and standard
library no longer depend on `lib/runtime/briev_rt.c` or libc for any program.
C remains available *to the user* through the existing `frgn`/`#System`/GLUE
mechanisms, but it is never a compiler dependency.

The language family is realigned:

| Extension | Role | Backend |
|---|---|---|
| `.bv` | General Briev — **includes freestanding/embedded** (triple-driven) | LLVM |
| `.ebv` | **Electronics Briev** (KiCad PCB, closed-world strict) | Electronics (new) |
| `.sbv` | **Silicon Briev** (reclaims the formerly forbidden `.sbv`; `.cbv` retires) | CIRCT |
| `.abv` | Accelerator Briev | SPIR-V |
| `.rbv` | Rendered Briev | Webstack |
| `.dbv` / `.dbvl` | Data Briev | Data parser |

Logos swap shapes and colors between Electronics and Silicon, reusing the exact
existing hex values.

Parity is a first-class requirement: every migrated family must reproduce
byte-identical program output and at-or-above baseline performance, enforced by
a committed golden corpus and the existing benchmark A/B baseline.

---

## 1. Principles

### 1.1 Briev stands on its own

The compiler's default runtime is Briev-native. Briev does not *depend* on C:

- The default runtime = pure Briev stdlib + `SysCall#` inline asm + LLVM-level
  emission. No `briev_rt.c`, no libc required to build or run any program.
- System interaction (processes, env, files, sockets, timers) reaches the OS
  through `SysCall#` inline asm (`syscall` on x86_64, `svc #0` on aarch64),
  captured-environ reads, and Briev-native stdlib wrappers.
- User-facing C FFI stays: `frgn ... from #System` (→ `-lc`), `from "path"`,
  `#Link<name>`, `extern HDL`, GLUE bridges. The `#System` resolver
  (`src/target.rs:78-133`) is untouched — it is the user's door to libc, not
  the compiler's crutch.
- Consequence: there is no "embedded runtime" and "native runtime". There is
  **one Briev-native runtime**; the target triple selects the platform surface.
  Embedded/freestanding is a target configuration, not a source variant.

### 1.2 Parity is a requirement

Every migrated family must close a parity gate before the C symbols are
deleted: byte-identical output vs the pre-migration compiler, and no benchmark
regression vs the baseline worktree. See §6.

### 1.3 Electronics is a closed system

Electronics Briev (`.ebv`) uses the Briev philosophy — topology, contracts,
nodal reasoning, compile-time proving — to build PCBs. Its core fundamentals
follow physical electronic components, not software types. Because electronics
are a **closed system** (finite, deterministic, fully knowable), strict
semantics are allowed and mandatory:

- No OS, no dynamic allocation, no open-world FFI, no concurrency ambiguity.
- Every program either proves its contracts or fails — there is no "maybe".
- Capabilities const is the strictest in the family.

---

## 2. Part A — Eliminate `briev_rt.c`

### 2.1 The file

`lib/runtime/briev_rt.c` (1407 lines) provides: casting-graph lane helpers
(`int_to_str` etc.), the print/exit family, string operations, collection/vector
ops, `briev_syscall`/`briev_sysconf`, CLI/argv/env, TTY/timer/trigger (mostly
stubs), a pthread async pool + segmented-continuation task/event machine,
process/spawn helpers, and Tamer HCALL host services.

It is linked in four places today:
1. `src/compile.rs:1866` (`compile_ll_to_library` — cc/ar into `.so`).
2. `src/compile.rs:1693` (`collect_extra_objects` — frgn-derived `.o`).
3. `benchmarks/build_and_bench.sh:251,292` (one-step fallback links).
4. `src/backend/llvm/tests.rs:6631,6721` (two tests).

Plus `import "link/briev_rt.c"` in bench/trophy `.bv` files, and ~50 dead
`briev_*` declares in `emit_declares()` (no C definition, no caller).

### 2.2 Family-by-family migration

Each family migrates to a Briev-native home, gated by the parity checklist
(§6.3). Families are prerequisite-ordered.

#### Family A — Casting lanes

Symbols: `int_to_str`, `uint_to_str`, `float_to_str`, `bool_to_str`,
`char_to_str`, `str_to_int`, `str_to_uint`, `str_to_float`, `str_to_bool`,
`str_first_char`, `briev_str_char_at`, `__chr_to_str`.

Home: Briev defns in `lib/std/string.bv` and `lib/std/char.bv`. The pattern is
already proven by `lib/std/string.ebv` (Briev digit-loop `int_to_str`, etc.).

Gap: `float_to_str`, `char_to_str`, `str_to_int`, `str_to_uint`,
`str_to_float`, `str_to_bool` in `string.ebv` are stubs — they need real
pure-Briev implementations.

**`float_to_str` is the single riskiest piece of the entire overhaul.** It must
reproduce C `printf("%.9g")` byte-for-byte on the float corpus (§6.2). This is a
fixed-precision (9 significant digits) formatting algorithm with correct
rounding and exponent-form switching — implementable in pure Briev via
`Bits<64>`/`#Bit` bit manipulation, but intricate. **Do it as the first spike,
gated by `float_cases.bv`, before committing the print family.**

Decision (per user): **Briev-native always.** If a cast benchmark regresses,
fix the Briev implementation until parity — never re-add the C lane.

#### Family B — Print and exit

Symbols: `__print`, `__print_int`, `__print_float`, `__print_float64`,
`__print_char`, `__print_bool`, `__print_str`, `__eprint_str`, `__exit`.

Home: `lib/std/ffi/io.bv` Briev defns. Each print lowers to: cast lane → String
buffer → `SysCall#(SYS_write)` on fd 1 (stdout) or fd 2 (stderr). `__exit`
→ `SysCall#(SYS_exit_group)`.

**Zero backend change required.** The print pipeline already resolves symbols
through the `frgn_map` by name (`frgn__print_int` → C symbol `__print_int`,
intrinsics.rs `frgn_symbol`, 2026-09-08); the declare-guard
(`mod.rs:3264-3277`) already skips the C declare when a Briev defn provides the
symbol. The Briev defns provide the symbol; the map resolves; the guard skips.

#### Family C — String operations

Symbols: `briev_str_eq`, `briev_str_band/bor/bxor/bnot`, `briev_str_substr`,
`briev_str_next_char` (UTF-8 decode), `briev_char_len`, `briev_bits_to_str`,
`briev_cstr_to_briev`, `briev_str_to_c`, `briev_cstring_concat`.

Home: Briev defns in `lib/std/string.bv` (most already have Briev equivalents).
`briev_str_to_c`/`briev_cstr_to_briev` are the GLUE bridge doors — they stay
for `lib/glue/*.bv` users but become Briev defns (zero-copy views) where
possible; where a true C interop pointer is needed, that is a user-facing frgn
to libc, not a runtime dependency.

#### Family D — Collection / vector operations

Symbols: `briev_mask_select`, `briev_mask_select64`, `briev_mask_select_f32`,
`briev_mask_select64_i8mask`, `briev_mask_select_f32_i8mask`,
`briev_slice_range`, `briev_slice_range64`, `briev_slice_range_f32`,
`__briev_coll_resize`.

Home: LLVM-native `shufflevector`/vector ops emitted by the backend (the
compiler already has vector machinery) and Briev-native collection resize in
`collections.bv`.

**Hot path — benchmark-gated.** `cancel_math`, `series_converge`, sweep
benchmarks exercise these. Perf parity gate is mandatory before deleting C.

#### Family E — Syscall and sysconf

Symbols: `briev_syscall`, `briev_sysconf`.

Home: `SysCall#` inline asm already emits `syscall` (x86_64) / `svc #0`
(aarch64) (`intrinsics.rs:1209-1294`). `SysConf#` currently routes to
`@briev_sysconf` — migrate to inline asm for the specific queries Briev uses.
The fallback `call void @briev_syscall()` for unknown targets is removed with
the file.

#### Family F — CLI / argv / env

Symbols: `__argv_count`, `__argv_get`, `__argv_has`, `__argv_value`,
`__argv_command`, `__get_environ`, `__getenv_briev`, `__getenv_int`.

Home:
- The compiler already emits `@__briev_argc` / `@__briev_argv` globals —
  Briev readers via `Alloc#`/Ptr read them natively.
- **getenv**: no syscall exists on Linux (the env array lives on the process
  stack). **Compiler-captured environ** (user decision): the compiler captures
  the `environ` pointer into a global at startup (alongside `@__briev_argv`),
  and `getenv`/`GetEnvInt#` read it via Ptr. Requires an `_start`-level stub
  that reads the initial stack layout. Works freestanding, no libc.

#### Family G — TTY / timer / trigger

Symbols: `tty_raw_mode`, `tty_size`, `__tty_raw_mode__`, `__tty_size__`,
`__tty_read_key__`, `__readln__`, `__sort_list__`, `__reverse_list__`,
`briev_ttyname`, `__trg_timerfd_open/read`, `__trg_signalfd_open/read`.

Home:
- Timerfd/signalfd are real syscalls → `SysCall#`.
- `tty_*` / `__readln__` / `__sort_list__` / `__reverse_list__` are **stubs
  returning -1** today — drop them and let the missing symbol be a capability
  error, or implement `sort`/`reverse` Briev-natively in stdlib.
- `briev_ttyname` (real `ttyname()` libc call) → `SysCall#(SYS_ttyname? does
  not exist)` — replace with `ioctl(TCGETS)`/`TCGETA` via `SysCall#` or drop;
  only used by terminal tooling.

#### Family H — Async / thread-pool / barrier / task / event

Symbols: `worker_thread`, `__rt_cleanup`, `__rt_init`, `__set_async_state__`,
`__thread_pool_init__`, `__barrier_release__`, `__barrier_wait__`,
`briev_thread_pool_shutdown`, `__wait_for_trigger__`, `briev_task_spawn`,
`briev_task_cancel`, `briev_task_done`, `briev_await`, `briev_event_alloc`,
`briev_task_mark_waiting`, `briev_event_read`, `briev_event_fire`,
`briev_event_ready`, `briev_event_strict_trap`.

Home: **Included now** (user decision). The existing event machine is already
segmented-continuation cooperative scheduling; the pthread pool is only the
parallel substrate. Native plan:
- Emit the event machine Briev-natively (declare-guard mechanism, same as the
  lanes).
- Barriers (`sync<group>`) and `__wait_for_trigger__` become cooperative
  yields. Reactor semantics (Rule 22 `async`/`sync<group>` classification) are
  honored — simultaneous-firing acknowledgment is a *semantic* contract, not a
  thread count.
- OS-thread parallelism becomes a documented follow-on: `SysCall#(SYS_clone)` +
  futex substrate, only if the cooperative model regresses async benchmarks
  (see §6.4).

**Observable-behavior note:** cooperative scheduling loses true multicore
parallelism on `async` workloads. Output parity is preserved (same firing
sequence); throughput is the open question, gated by A/B.

#### Family I — Process / spawn / misc

Symbols: `ShellCmd`, `__briev_spawn`, `__briev_spawn_output`, `__briev_setenv`,
`__briev_getcwd`, `__briev_chdir`, `__briev_now`, `__briev_free`,
`__briev_free_count`, `__watchdog_fail`.

Home:
- `fork`/`execve`/`getcwd`/`chdir`/`clock_gettime` are syscalls → `SysCall#`.
- `__briev_now` → `clock_gettime(CLOCK_MONOTONIC/REALTIME)` via `SysCall#`.
- `__watchdog_fail` → Briev-native trap (`llvm.trap`).
- `__briev_free`/`__briev_free_count` → the Briev-native allocator (§2.3).
- `ShellCmd`/`__briev_spawn_output` (popen/system): these are inherently
  libc-flavored; reimplement via fork+exec+pipe (`SysCall#`) or keep as
  user-facing frgns to libc (allowed — user FFI, not a compiler dependency).

#### Family J — Tamer HCALL

Symbols: `briev_host_print_int`, `briev_host_fail`, `briev_host_table_set`,
`briev_host_arity_of`.

Home: Tamer stdlib, self-hosted (`.dbv`/`.bv`), low priority. Tamer is the
self-hosted VM layer; its host services become Briev defns.

### 2.3 The allocator (memory model change)

With `briev_rt.c` gone, native `Malloc#` cannot route to libc `@malloc`.
Native moves to the arena model the embedded path already uses:
`@embedded_heap` bump arena seeded with initial size, growing via
`SysCall#(SYS_brk/mmap)` instead of `@realloc`.

**Perf-sensitive.** `arena_churn`, `linked_list` benchmarks are the gate. The
arena grow path returns `null` when the static heap is exhausted; native must
grow (brk/mmap) rather than fail, unlike the freestanding "grow returns null"
path — this is the one behavioral fork between freestanding and hosted
triples, and it lives in the allocator, not the language.

### 2.4 Deletions

- `lib/runtime/briev_rt.c` — kept only as `lib/runtime/briev_rt.legacy.c`
  during migration for golden re-derivation; deleted when the last family
  closes.
- The ~50 dead `briev_*` declares in `emit_declares()` (no defn, no caller).
- Dead `frgn` declarations in `lib/std/ffi/*.bv` that point at stripped symbols
  (`__to_upper`, `__now`, `__base64_encode`, `__http_get`, `__shm_open`, …).
- All four link sites for `briev_rt.c`.
- `import "link/briev_rt.c"` in bench/trophy files.

### 2.5 What survives

`#System` → `-lc` resolution (user FFI), GLUE, `extern HDL`, `#Link<name>`,
`frgn ... from "path"`. Tests that link `briev_rt.c` directly are reworked to
link the Briev-native runtime or become pure-IR assertions.

---

## 3. Part B — Fold Embedded Briev into `.bv`

After Part A there is one Briev-native runtime, so "embedded" is just a target
triple that is freestanding:

- `triple_is_freestanding(triple) -> bool` — extract the existing
  `["arm", "thumb", "aarch64", "cortex", "riscv"]` family check
  (`emit_stmt.rs:2059`, the halt→wfi gate) into a single helper.
- Replace `ext == ".ebv"` → `with_embedded_mode(true)` at
  `compile.rs:1262/1338/1430` with triple-driven activation: a target profile's
  `target_triple` is freestanding, or the backend default triple is
  freestanding. Profile may override and tune arena size.
- `prefer_ebv` resolver preference and the `string.ebv` sibling **dissolve** —
  there is no `.ebv` stdlib variant after Family A moves the lanes into
  `string.bv`.
- `--optimize-size --budget 0` defaults (dead metadata, no consumer) are
  removed.
- `SourceKind::Embedded` (conformance.rs) and `.ebv` embedded classification
  are removed; the `.ebv` extension is reclaimed by Electronics (Part C).

---

## 4. Part C — Electronics Briev (`.ebv`) skeleton

### 4.1 Identity

Electronics Briev uses the Briev philosophy (topology, contracts, nodal
reasoning, compile-time proving) to design printed circuit boards. Its core
fundamentals follow physical electronic components. It is **strict by nature**:
electronics are a closed system — no OS, no dynamic allocation, no open-world
FFI, no concurrency ambiguity. Every program proves or fails; there is no
"maybe."

### 4.2 Routing and classification

- `config/targets.dbvl`: `.ebv: electronics; ; prelude;` → new
  `BackendKind::Electronics`.
- `conformance.rs`: `SourceKind::Electronics` for `.ebv`; drop
  `SourceKind::Embedded`.
- `vocab.rs`/`target.rs` golden tests updated.
- `syntax-highlighter`: `.ebv` id/aliases updated to Electronics Briev.

### 4.3 Fundamentals (minimal skeleton, first increment)

- **Component declarations** — physical types, not software types:
  `let r1: Resistor(value: "10k", package: "0805");` where `value`,
  `package`, `footprint`, `rating` are the type's physical metadata (`spec`).
- **Pin connections** — `<->` operator: `r1.pin(1) <-> led1.pin(2);`.
- **Derived nets** (user decision): a netlist is the *transitive closure* of
  explicit connections. The compiler derives nets from the connection graph
  (union-find over pins). Guardrails:
  1. **Dangling-pin detection** — a single-pin net is a compile-time contract
     error (`[no dangling pins]`): the #1 PCB error must never be silent.
  2. **Opt-in naming** — `let vbus = r1.pin(1) <-> led1.pin(2);` binds the
     *derived* net to a name for contracts (`[vbus.current <= 2A]`) and
     metadata (trace width, impedance). Unnamed nets get auto names.
  3. **Role metadata, not direction** — a net is one electrical node; a
     driver/driven pin role is metadata on the pin, never a second net.
- **Electrical contracts** — the existing contract engine maps directly:
  `[max_current <= 2A]`, `[voltage <= 3.3V]` on nets/pins; a 5V net wired into
  a 3.3V pin fails to compile. Contracts are mandatory (strict).

### 4.4 Backend

- New `src/backend/electronics/` emitting KiCad 6+ schematic S-expressions
  (`.kicad_sch`).
- Strictest capabilities const in the family (closed component universe,
  static netlist, no runtime).
- Demo (skeleton deliverable): resistor + LED + one derived net →
  `.kicad_sch` that opens in KiCad.
- Electrical-contract checking wires the existing proving engine to net
  topology (follow-on within the skeleton stream; contract infrastructure
  exists).

---

## 5. Part D — Silicon Briev: `.sbv`, retire `.cbv`

Silicon Briev (CIRCT) **reclaims the `.sbv` extension** currently on the
forbidden list (`conformance.rs:323` rejects `main.sbv`). `.cbv` retires to the
forbidden list.

- `config/targets.dbvl`: `.cbv: circt` → `.sbv: circt; ; prelude-hw;`.
- `conformance.rs`: `"sbv" => SourceKind::Silicon`; `"cbv"` joins the removed
  list; `SourceKind::Circuit` renamed `SourceKind::Silicon`.
- `vocab.rs` extension list: `["bv","ebv","abv","sbv","rbv","dbv","dbvl"]`.
- `target.rs` golden tests; `syntax-highlighter` (`sbv` id/scope, "Silicon
  Briev" aliases); `spec/SPEC.md` §3; README family tables; docs sweep.
- Rename "Circuit Briev" → "Silicon Briev" in all prose (README, README_DRAFT,
  docs, learn-briev). Use `docs/plans/2026-06-19-tier-renames.md` as the
  execution-order template (the `.hebv → .cbv` precedent, AGENTS_HISTORY.md
  :806-816, lists the full touched surface). No `.cbv` fixture files exist, so
  no test-asset rename burden.

---

## 6. The parity plan

Parity means: **after each family migrates, every program behaves
byte-identically and runs at-or-above baseline.** Four mechanisms.

### 6.1 Output parity — committed golden corpus

Before any migration begins (P0), freeze goldens from the current C-backed
compiler:

- Every runtime benchmark, at a BOUND producing 2-3 print lines →
  `benchmarks/parity/goldens/<name>.stdout`.
- `tests/tier1/*.bv` that print → same.
- Dedicated formatting corpora (below).
- Goldens are **committed to the repo** and enforced by `cargo test` + the
  parity harness — same principle as the baseline worktree.

### 6.2 Format parity — targeted corpora

- `float_cases.bv`: ~50 edge values (NaN, ±inf, ±0.0, denormals, `0.1`,
  `1e-9`, `1e9`, `1.5e308`, rounding boundaries) → pins `float_to_str` to C
  `printf("%.9g")`.
- `int_cases.bv` / `uint_cases.bv`: `%ld` semantics, negatives, INT64
  extremes.
- `parse_cases.bv`: `str_to_int` on `"+42"`, `"-0"`, `" 42 "`, `"0x1F"`,
  overflow, garbage — return-value parity, not just output.
- Corpora live in `tests/tier1/` and run in `cargo test --lib`.

### 6.3 Per-family migration gate

For each family (A–J):
1. Implement the Briev-native family in stdlib.
2. `cargo test --lib` green.
3. Parity harness: all goldens byte-identical.
4. `compare_baseline.sh` A/B: no perf regression.
5. **Only then** delete the C symbols.

### 6.4 Performance risks (ranked) and gates

1. **`float_to_str` `%.9g` rounding parity** — first spike; gated by
   `float_cases.bv`; must match glibc rounding on the corpus.
2. **Async cooperative throughput** — A/B on `async_counters` etc.; if a real
   regression appears, the follow-on is a `SysCall#(SYS_clone)` + futex thread
   substrate. Reactor semantics honored either way.
3. **Native arena memory model** — `arena_churn`, `linked_list` A/B; hosted
   arena must grow via brk/mmap, freestanding returns null (the one allocator
   fork, target-driven).
4. **Collection/vector ops** — sweep/cancel_math/series_converge A/B.

Regression is never accepted and never excused as noise (Rule 12b) — each risk
has a defined resolution path.

### 6.5 Parity harness shape

`bash benchmarks/parity/run.sh` — rebuilds the corpus `.bv` files, diffs
stdout/stderr against committed goldens, reports pass/fail per family.
`briev_rt.legacy.c` supports re-deriving a golden if a corpus case proves
ambiguous; deleted when the last family closes.

---

## 7. Part E — Logo swap

Reuse the exact existing hex values and swap shapes + colors between the two
variants:

| Asset (before) | Shape/color today | After |
|---|---|---|
| `assets/c-briev-*` (copper `#cf9f46` shape) | Circuit Briev | recolored to green `#01ff7c` → renamed `e-briev-*` (Electronics) |
| `assets/e-briev-*` (green `#01ff7c` shape) | Embedded Briev | recolored to copper `#cf9f46` → renamed `s-briev-*` (Silicon) |

`a-briev-*`, `d-briev-*`, `r-briev-*`, root `briev-*` unchanged. README logo
mosaic updated. Note: the logo *content* swap is a recolor of the existing
shapes, not a redesign.

---

## 8. Ordering and dependencies

1. **P0 — Parity freeze** (goldens + corpora) and `float_to_str` spike. First
   commit; goldens must predate all change.
2. **Family A** (casting lanes) → **B** (print/exit) → **C** (string ops) →
   **D** (vector/collection) → **E** (syscall/sysconf) → **F** (argv/env) →
   **G** (tty/timer) → **H** (async) → **I** (spawn/misc) → **J** (tamer).
   Each closes its gate before the next starts.
3. **Part B** (embedded folding) rides on A–F (no `.ebv` stdlib remains after
   A).
4. **Part C** (Electronics) depends on the `.ebv` extension being free —
   blocked until B completes.
5. **Part D** (`.sbv`) and **Part E** (logos) share the naming/extension sweep
   with C — one coordinated migration.
6. **Delete step** — `briev_rt.c`/legacy removal, dead declares, dead ffi
   frgns, link-site rework — last.

## 9. Risks

| Risk | Mitigation |
|---|---|
| `float_to_str` cannot match glibc `%.9g` on the corpus | Spike first; corpus is the gate; algorithm is fixed-precision (9 sig digits) Ryu-class, tractable |
| Async cooperative loses multicore throughput | A/B; clone+futex substrate is the documented follow-on |
| Native arena memory model regresses allocators | A/B on arena_churn/linked_list; hosted arena grows via brk/mmap |
| Vector ops on the hot path regress | A/B on sweep/cancel_math/series_converge |
| Electronics strictness too restrictive for a first skeleton | Capabilities const grows in follow-on increments; the skeleton is deliberately minimal |
| `.sbv` reclaim conflicts with historical docs | Sweep uses the tier-renames template; forbidden list updated symmetrically (`.cbv` in, `.sbv` out) |

## 10. Docs touched (same commit as each phase)

- `spec/SPEC.md` §3 (extension table, dotted-profile example), §13.1 (`cbv`→
  `sbv`, native/embedded), §19 (`C runtime backing` → Briev-native), CI scope
  line.
- `docs/architecture/backend-strategy.md` (embedded section → target-driven
  freestanding), `backend-contracts.md`, `c-surface-inventory.md`,
  `agent-reference.md`, `overview.md`, `casting-protocol.md`/`hash-words.md`
  (`.ebv`→ASCII claims, now moot).
- README / README_DRAFT family tables and logo mosaic; syntax-highlighter.
- `AGENTS.md` reference index; plan docs are historical and unedited.
- New: `docs/architecture/electronics-briev.md` (Electronics fundamentals,
  when implemented), `docs/architecture/briev-native-runtime.md` (runtime
  model).
---

# Amendments — 2026-09-10 (expressiveness closure)

Appended during the Family C session. The work below was discussed as the
capability question — "is Briev theoretically capable of writing an
LLVM-class system?" — and resolved into four concrete workstreams. The
governing principle:

> **Expressiveness closure.** The compiler's chosen optimum must never be
> more powerful than the language. If the compiler can do X, a user must be
> able to build X in Briev. Rule 14 applied to features becomes, applied to
> *techniques*: every optimization trick CS has or will invent must be
> expressible in Briev itself — a new hypothetical optimal path must itself
> be implementable, or the compiler's cleverness is a ceiling instead of a
> floor.

## A. Allocator ownership (Family E, upgraded)

`config/alloc-strategies.dbvl` already provides custom allocation strategies
as config (quoted strategy names, LLVM-IR templates, per-strategy `Free#`
dispatch). The upgrade: **the strategy functions become pure-Briev defns**
(`@pool_alloc` et al. live in a stdlib file; declare-guard resolves them —
the print-family mechanism), and `Alloc#`/`Malloc#` take their final Rule 14
form:

- The compiler keeps ONLY the bootstrap: the static fallback heap that
  `--no-std` requires.
- The allocation STRATEGY is stdlib-owned: `lib/std/alloc.bv` implements the
  hosted arena over `SysCall#(SYS_brk/mmap)` in pure Briev (~30 lines; the
  primitives were proven by the cast_lanes work). `alloc-strategies.dbvl`
  rows point at Briev defns instead of C symbols.
- Collection growth (`__briev_coll_resize`, Family D) routes through the
  same stdlib-owned allocator.

This replaces plan §2.3's "Rust emits arena+brk" — Briev *is* the arena.

## B. Asm# fundamental (two-mode)

One intrinsic, two modes, replacing the retired top-level `asm`/`AsmFn`
surface (no active shipped `.bv` declares an AsmFn — the emitter exists but
is unused; run the deprecation playbook):

1. **Abstract mode** — `Asm#("prefetch", addr)`: a Briev-level abstract
   instruction, lowered per target through `config/asm-lowering.dbvl`
   (arrow-row style like `bindings.dbvl`): unknown op or unsupported target
   = capability error (what/why/fix, per capabilities.rs doctrine).
2. **Raw mode** — `Asm#("raw", template, ...operands)`: dialect-specific
   text with `$N` operand binding, reusing the `SysCall#` inline-asm emitter
   (proven). `observable: true` (never DCE'd). Forbidden at source level in
   `.s` strict programs; stdlib wrappers bear the proof burden.

**Named-intrinsic duality**: common ops promote to real intrinsics
(`Prefetch#`, `Rdtsc#`) with `bindings.dbvl` templates and typechecker
signatures; the long tail stays `Asm#`. Both tables are config — adding an
op is a data change, never a Rust change (closure preserved at the asm
surface itself).

**Verification**: abstract ops get operand contracts on their stdlib
wrappers (`lib/std/asm.bv`); raw gets structural checks (operand count vs
`$N` references) + the `.s` gate. Emission reuses the `SysCall#` machinery;
capability declarations per backend in `capabilities.rs`.

## C. `inline_frgn!` plugin

Retires the top-level `frgn` ritual for one-off FFI. A plugin (Rust AST
manipulator, print_plugin precedent) that at `$(Parsed)`:
1. synthesizes the `frgn` declaration at module scope, and
2. rewrites the call site.

Reuses the ENTIRE existing path — frgn_map registration,
`collect_extra_objects` linking, the declare-guard, the state-prefix call
adaptation. An `InlineFrgn#` intrinsic was rejected: it would duplicate all
of that inside the compiler (Rule 14 prefers the plugin).

Shape (explicit signature — the one-time ritual tax paid inline):

```
inline_frgn!("__print_int", "lib/runtime/briev_rt.c", "fn(n: Int) -> Int", my_n);
```

## D. Capability frontier doc

`docs/architecture/briev-capability-frontier.md` records the principle, the
tier table (expressible today / one primitive away / analysis-only), the
session evidence, and the self-hosting endgame (a QBE-scale native emission
tier breaking the rustc→LLVM bootstrap chain; LLVM itself is explicitly NOT
the goal — theoretical capability is).

## E. Part B — Explicit Bare Suffix (`.b.bv`) + proof-based constraints

### E.1 Suffix convention

Replace the opaque `.ebv` extension with a `.b` dotted-profile prefix,
consistent with `.f` (formatted) and `.s` (strict). The `.b` segment goes
BEFORE the base extension, stackable with `.f`/`.s`:

| Before | After | Meaning |
|---|---|---|
| `main.ebv` | `main.b.bv` | bare (freestanding) |
| — | `main.bf.bv` | bare + formatted |
| — | `main.bs.bv` | bare + strict |
| `main.bv` | `main.bv` | hosted (default, unchanged) |

Clean break — no backward-compat shim for `.ebv`.

### E.2 Bare-metal behavioral model

The `.b` suffix activates "bare-metal proof obligations." The compiler
applies stricter analysis when it is set. The three physical constraints
and how the compiler handles them:

| Constraint | Error (provably wrong) | Warn (likely wrong) | Strategy keyword |
|---|---|---|---|
| Recursion depth | No base case, or depth provably exceeds stack | Base case exists but depth unbounded | `fn foo(...) -> T : recursion<64>` — programmer asserts bound |
| Thread lifecycle | Join path provably unreachable | Join exists but may not execute | `fire_and_forget ThreadCreate#(...)` — explicit intent |
| Heap budget | — | — | `config/targets.dbvl` per triple (not user code) |

Strategy keywords sit in the function signature (like `async`, `seq`) or
as statement modifiers (like `trap`, `halt`). They are NOT contracts —
they declare how the compiler handles what it cannot prove.

### E.3 Implementation steps

1. **`is_bare()` in conformance.rs** — add `is_bare(path) -> bool`
   mirroring `is_formatted()`/`is_strict()`. Remove
   `SourceKind::Embedded` variant and `"ebv"` classification arm.

2. **compile.rs** — replace all `get_extension(file_path) == ".ebv"`
   with `is_bare(Path::new(file_path))`. Remove `prefer_ebv` resolver
   call.

3. **import_resolver.rs** — delete `prefer_ebv` field, builder method,
   and three-tier resolution logic. Resolution always picks `.bv`.

4. **Target/config cleanup** — delete `.ebv` entry from
   `config/targets.dbvl`. Delete `prefer_ebv` from `TargetSettings` in
   `config_tuning.rs`. Delete `.ebv` entry from `target.rs` map.

5. **Delete `lib/std/string.ebv`** — 89 lines, mostly stubs. Family A
   moved core lanes into `string.bv`. No code imports it directly.

6. **Fix message prefix** — change `"TargetError:"` to
   `"TargetWarning:"` on threading/recursion messages in
   `check_embedded_restrictions`. These are warnings, not errors.

7. **Add proof engine** (incremental):
   - Recursion: detect base cases in the call graph; warn only when
     depth is unbounded. Acyclic calls with known base cases pass clean.
   - Threading: detect join reachability; warn only when lifecycle is
     unprovable. `ThreadCreate#` followed by `ThreadJoin#` on the same
     handle passes clean.

8. **Add strategy keywords**:
   - `: recursion<N>` on function signatures — suppresses recursion
     warning; compiler verifies `N` fits the target stack size.
   - `fire_and_forget` before `ThreadCreate#` — suppresses threading
     warning; programmer declares intent.

9. **Update tests** — rename `.ebv` test files to `.b.bv`. Update
   embedded tests to use `is_bare()`.

10. **Docs** — update `spec/SPEC.md` §3 extension table,
    `docs/architecture/briev-native-runtime.md` embedded references.

### E.4 What does NOT change

- `is_embedded` flag and all behavioral forks (arena model, heap routing,
  briev_rt.c skip, threading/recursion warning infrastructure)
- Halt gate (triple-based, not affected)
- `EmbeddedConfig` struct (dead scaffolding, cleaned up separately)
- All Families A–I work

### E.5 Files touched

| File | Change |
|---|---|
| `src/conformance.rs` | Add `is_bare()`, remove `SourceKind::Embedded` |
| `src/compile.rs` | Replace `.ebv` checks with `is_bare()`, remove `prefer_ebv` |
| `src/import_resolver.rs` | Remove `prefer_ebv` field + resolution logic |
| `src/config_tuning.rs` | Remove `prefer_ebv` field + loader + test |
| `src/target.rs` | Remove `.ebv` entry |
| `config/targets.dbvl` | Remove `.ebv` line |
| `src/dbriev/config_db.rs` | Update `.ebv` test reference |
| `lib/std/string.ebv` | Delete |
| `src/backend/llvm/mod.rs` | Fix `"TargetError:"` → `"TargetWarning:"` prefix |
| `src/backend/llvm/tests.rs` | Update embedded test filenames |
| `spec/SPEC.md` | Update §3 extension table |
| `docs/architecture/briev-native-runtime.md` | Update embedded references |

## Sequencing note

Family D resumes first (str_to_float, then vector ops — both gate-gated).
Allocator ownership (A) lands with Family E; Asm# (B) and `inline_frgn!` (C)
are independent fundamentals that can land in either order after D. The
capability doc ships immediately with these amendments.

---

## Amendment 2 (2026-09-11, post-implementation): Part C as built

Part C landed with two decisions superseding §4.3:

1. **`<->` is DEAD — nets are contract-inferred.** Pin connections are
   inferred from precondition pin-equality obligations; nets are the
   transitive closure (union-find). "What triggers what" reuses the reactor
   trigger analysis. Postconditions are physics, never wiring.
2. **Pins are first-class, not metadata.** `pin <name> [= <n>];` in type
   bodies (lexer token, canonical vocab, `PinDecl` on `TypeDefBody`).
   Auto-numbering continues after the highest explicit number; numbers ≥ 1,
   unique per type. Typechecker resolves pins as fields (to the prelude
   `Pin` type) but never as literal-construction fields.

Also superseded: §4.2's `config/targets.dbvl` row exists again with
`.ebv: electronics; ; prelude-electronics;` (Part B had removed the row
because `.ebv` was still Embedded; Electronics reclaims it). Grammar
additions: `pin` keyword only — component declarations use existing
struct-literal form (`let r1: Resistor = Resistor { value: "330" };`),
empty literals `T { }` now parse.

As built: `src/analysis/electronics.rs` (derivation),
`src/backend/electronics/mod.rs` (KiCad 7 emission),
`lib/std/electronics.bv` + `plugins/parsed/prelude-electronics.bv`
(extension surface), `examples/electronics/led_blinker.ebv` (demo:
connector → 330 Ω → LED, compiles to a `.kicad_sch` that opens in
KiCad). Architecture: `docs/architecture/electronics-frontend.md`.
Deferred: named nets, pin electrical roles, unit suffixes, footprint
validation, PinDecl beast-serialization.

---

## Amendment 3 (2026-09-11): fundamentals doctrine supersedes Parts C–E syntax notes

`2026-09-11-fundamentals-doctrine-and-electronics.md` is authoritative for
the electronics surface: category hashwords retired (hard errors), bare
fundamentals are base type + protocol, electrical SI bases are parentless
prelude types, pin/reference/tolerance are structural clauses (metadata
path deleted), nets are contract-inferred, and current bounds are proven
via compile-time Ohm's law. The `<->` model (Amendment 2's note 1) remains
superseded.
