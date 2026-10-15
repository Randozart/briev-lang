# Core-usable purge — plain names, .bv-native logic, gap fill

**2026-10-15.** Goal: Briev's core must be *usable* — no `frgn` where a
`.bv` implementation is possible, no dunder-prefixed entries anywhere in
source-visible language surface, and the daily-driver gaps (directory
listing, PRNG, temp files) filled. Follows the tier-2 close-out
(`2026-10-14-daily-driver-tier2.md` — all four phases shipped).

## Decisions (authoritative — do not re-litigate in-session)

1. **Plain names.** The dunder prefix was originally a marker for
   frgn-adjacent entries; the decision is to delete it from the language
   surface entirely. cast_lanes' existing house style is already plain
   (`byte_at`, `str_to_int`, `float_to_str`) — the `__` names are the
   legacy minority.
2. **`frgn` survives only where interop IS the feature**: `lib/std/web/`
   (JS interop for the web backend), `lib/tamer/` (tooling host bridge),
   `lib/glue/` (the GLUE product generates `frgn` for users — that is the
   feature). Everywhere a `.bv` implementation is possible, we write the
   `.bv` implementation.
3. **Scope of the purge is Briev-source-visible identifiers**: `defn`/
   `out defn` names, top-level state `let`s, locals/params, plugin-
   injected names, backend-emitted call targets that resolve to `.bv`
   defns, liveness-table rows, and every call site in `.bv`. NOT in
   scope (out of reach, not entries): C-ABI pinned symbols in
   `lib/runtime/briev_rt.c`, backend-emitted LLVM globals
   (`@__briev_*` in IR text), Rust-side IR gensyms (`%__x`,
   `__variant_`), and dogfood `lib/compiler/backends/llvm.bv`'s emitted
   IR-text strings (`@__wait_for_event` etc. — those are C-ABI names in
   generated IR, not Briev identifiers).
4. **Intrinsic C implementations stay** (bootstrap exceptions, Rules 3/4:
   `Spawn#`'s `__briev_spawn` C helper, the `--no-std` C print symbols).
   That is the separate `briev-native-runtime` family; this campaign does
   not touch it.
5. **Stub modules**: fill `dir`, `rand`, `temp` now; the other 16 stubs
   get an honest "not implemented — contributes constants only" header.
   A fill-all campaign is future work.
6. **No syntax change** → `spec/SPEC.md` untouched (verified: SPEC has no
   `__print`/`__argv` references; its `frgn … from #System` design is the
   FFI feature, unaffected).

## Inventory (2026-10-15, verified by grep)

### `frgn` live declarations — 23 total, ZERO in the daily-driver core

| Location | Count | Disposition |
|---|---|---|
| `lib/std/web/*.bv` | 13 | KEEP — web backend JS interop (`from "glue/web/web.js"`) |
| `lib/tamer/*.bv` | 6 | KEEP — tooling host bridge (`from "host"`, `briev_rt.c`) |
| `lib/std/ffi/xxhash.bv` | 2 | **REIMPLEMENT in pure .bv** (`XXH64`/`XXH32`; zero importers today) |
| `lib/glue/generator.bv`, `test_bridge.bv` | (text) | KEEP — GLUE emits `frgn` into generated bindings |
| Everything else (prelude, `plugins/parsed/*`, `lib/std` core) | 0 | Already frgn-free; remaining hits are comments |

### Dunder source-visible definitions — 20 (all in prelude-loaded files)

- `lib/std/cast_lanes.bv` (18 `out defn`): `__stdout_flush`,
  `__stdout_byte`, `__print_str`, `__print`, `__eprint_str`,
  `__print_int`, `__print_bool`, `__print_float`, `__print_char`,
  `__exit`, `__briev_coll_resize`, `__briev_now`, `__watchdog_fail`,
  `__briev_getcwd`, `__briev_chdir`, `__briev_free`,
  `__wait_for_trigger__`, `__briev_event_strict_trap`
- `lib/std/cast_lanes.bv` state: `let __stdout_buf`, `let __stdout_fill`
- `lib/std/cli.bv` (2): `__argv_has`, `__argv_value`

### Dunder call/reference sites

- Backend emission strings (call targets that resolve to the defns):
  `src/backend/llvm/intrinsics.rs` (8), `mod.rs` (6), `emit_stmt.rs` (1),
  `emit_toplevel.rs` (1), `tests.rs` (6)
- `src/plugin/print_plugin.rs` (`!Print`/`println!` → `__print_*`)
- `src/plugin/entry_plugin.rs` (`entry!`/`args!` → `__argv_has/value`)
- `src/plugin/script_plugin.rs` (injects `__script_main`/`__script_done`)
- `src/analysis/defn_liveness.rs` `intrinsic_helpers` rows (data rows)
- `.bv` call sites: `tests/tier1/daily_use_smoke.bv`,
  `tests/fixtures/event_model.bv`, `tests/fixtures/phase4/ffi_print.bv`,
  `tests/fixtures/inop_{alias,meld,multi,state}.bv`,
  `examples/ptr-runtime.bv`, `lib/std/cli.bv` (internal)
- Comments: `lib/std/io.bv` header, cast_lanes headers, `lib/std/os/fs.bv:40`

### Stub modules (19/21 `lib/std/os/*.bv` are duplicate-const stubs)

Filled this campaign: `dir.bv`, `rand.bv` (new), `temp.bv` (new).
Honest-header sweep: `atomic`, `debug`, `dynlib`, `ipc`, `mem`, `net`,
`process`, `resource`, `sched`, `signal`, `sysinfo`, `sysnum`, `thread`,
`tty`, `user` (and any remaining after fills).
Legacy landmines: root `lib/std/time.bv` (11 duration helpers, NO clock
— users importing `std/time` get nothing time-like; the real clock is
`std/os/time.bv`), root `lib/std/io.bv` header (teaches `__print_int`),
`lib/ffi/__init__.bv` (dunder filename, zero import references).

## Rename table (final)

| Old | New | Old | New |
|---|---|---|---|
| `__print_str` | `print_str` | `__exit` | `exit_prog` |
| `__print_int` | `print_int` | `__argv_has` | `argv_has` |
| `__print_float` | `print_float` | `__argv_value` | `argv_value` |
| `__print_bool` | `print_bool` | `__briev_free` | `rt_free` |
| `__print_char` | `print_char` | `__briev_now` | `rt_now_ns` |
| `__print` | `print_str_raw` (probe-verified final) | `__briev_getcwd` | `rt_getcwd` |
| `__eprint_str` | `eprint_str` | `__briev_chdir` | `rt_chdir` |
| `__stdout_flush` | `stdout_flush` | `__briev_coll_resize` | `coll_resize` |
| `__stdout_byte` | `stdout_byte` | `__watchdog_fail` | `watchdog_fail` |
| `__stdout_buf` | `stdout_buf` | `__wait_for_trigger__` | `wait_for_trigger` |
| `__stdout_fill` | `stdout_fill` | `__briev_event_strict_trap` | `event_strict_trap` |
| | | `__script_main`/`__script_done` | `script_main`/`script_done` |

Rules for the table: (a) both sides of the backend↔defn name contract
rename atomically in one commit per family; (b) if a target name
collides with an existing export in the flat namespace, the probe picks
a distinct final (`print_str_raw` is already the reserved fallback for
`__print`); (c) `__print_float64` has a liveness row but no `.bv` defn —
Phase-0 probe decides whether it is C-side (leave the C symbol, drop the
row if vestigial) or hidden somewhere (then rename).

## Phases

### Phase 0 — probes (writes only to `/tmp` or `target/`, no commits)

1. Duplicate-defn behavior: a user `defn print_str` alongside the
   prelude — loud error, or silent override (which today means a user fn
   can hijack backend-lowered calls → soundness bug, diagnostic added in
   Phase 1)?
2. Mechanical sweep: re-verify every `__` site by grep (defs, state,
   locals, params, plugin injections, emission strings, liveness rows,
   call sites); resolve `__print_float64`.
3. Enumerate the exact backend emission sites for the runtime-bridge
   family (`rt_*`, `watchdog_fail`, `wait_for_trigger`,
   `event_strict_trap`) so the contract renames atomically.
4. Wrapping arithmetic: confirm `Int` wraps (or `Bits(64)`/bit
   intrinsics suffice) for XXH64's u64 mixing (Phase 3 feasibility).

### Phase 1 — dunder purge (one commit per family)

Family order: (1) print/stdout/exit in `cast_lanes.bv` + backend
strings + `print_plugin` + liveness rows + call sites + `tests.rs`
strings; (2) argv family (`cli.bv` + `entry_plugin.rs`); (3) script
state (`script_plugin.rs`); (4) runtime bridges (`cast_lanes.bv` +
backend contract, both sides atomic); (5) call-site sweep leftovers +
comment headers (`io.bv`, cast_lanes, `os/fs.bv:40`).
If Phase 0 found silent override: the redefinition diagnostic lands
with family 1 (loud error naming both defns + the fix).
**Closes with the dunder grep-gate test**: zero `defn __` / `let __` /
`@__` inside any `.bv` under `lib/` + `plugins/` (empty allowlist).

### Phase 2 — frgn: .bv-native core

1. `lib/std/ffi/xxhash.bv` rewritten in pure `.bv` (XXH64/XXH32, i64
   mixing per probe 4), contracts, tested against published vectors
   (`XXH64("", 0) = EF46DB3751D8E999`, `XXH32("", 0) = 02CC5D05`).
2. frgn **allowlist gate test**: live `frgn` declarations may appear
   only under `lib/std/web/`, `lib/tamer/`, `lib/glue/`.
3. Comment cleanup: `lib/std/time.bv` "could be frgn or defn" block,
   `lib/std/io.bv` header, anywhere tutorials imply frgn for basic I/O
   (`learn-briev/07-ffi.md` STAYS — it is the FFI lesson itself).

### Phase 3 — gap fill (dir + rand + temp + legacy)

1. `lib/std/os/dir.bv` — real module: `export defn entries(path: String)
   -> Result<List<String>, Int>` over `SYS_GETDENTS64` (const already in
   `posix/syscall.bv`; add if missing), fs.bv buffer/iteration idioms,
   contracts, behavioral test (SysCall# executes in the interpreter —
   `src/interpreter/intrinsics.rs:446` — so fs.bv precedent holds).
2. `lib/std/os/rand.bv` — splitmix64: `seed_rand(s: Int)`,
   `rand_u64() -> Int`, `rand_range(lo: Int, hi: Int) -> Int
   [lo < hi]`; behavioral test: same seed → identical sequence; range
   bounds; two successive draws differ.
3. `lib/std/os/temp.bv` — `create_temp() -> Result<String, Int>`:
   `open(O_RDWR|O_CREAT|O_EXCL, 0600)` + `rand.bv` suffix + bounded
   retry (mkstemp is libc, not a raw syscall — stdlib composition,
   Rule 14). Requires `remove_file(path)` in `lib/std/os/fs.bv`
   (SYS_UNLINK — add const to `posix/syscall.bv` if missing) so temp
   files do not leak; add and test it.
4. Legacy surface sweep: root `lib/std/time.bv` de-landmined (`import
   "std/time"` must provide the clock — re-export from `os/time.bv` or
   migrate the duration helpers there); `lib/std/io.bv` header rewritten
   to the new plain names; `lib/ffi/__init__.bv` renamed `init.bv`
   (zero references verified).
5. Honest-header sweep of the remaining 16 stubs (one commit).

### Phase 4 — end-to-end + gates

Mini-CLI under `brievc test`: argv (`args!`) + `read_file`/`write_file`
+ `entries` + `rand_range` + `now_ns`/`sleep_ms`, golden stdout.

## Gates (every commit)

`cargo test --lib` · `fs_smoke` · `daily_use_smoke` · `cli_args` ·
`lsp_smoke` · conformance sweep · `rbv_gate` · Praetor baseline-diff on
changed files (compare distinct `/tmp` dir names) · the two new
grep-gates (dunder, frgn allowlist) once they exist.

## Docs (same commit as the structural change)

- This plan (written first, before work).
- `lib/std/io.bv`, cast_lanes, `os/fs.bv` comments; root
  `lib/std/time.bv` header; honest stub headers.
- `src/analysis/defn_liveness.rs` table doc-comment (renamed rows).
- `docs/plans/INDEX.md` one-line pointer after close-out.
- `spec/SPEC.md`: untouched (no syntax change).

## Risks

- Backend↔defn contract breakage is caught loud by the existing
  IR-scan net (missing helper → compile error naming the helper).
- Plain-name collision: Phase-0 probe decides; silent-override ⇒ new
  redefinition diagnostic first.
- `src/backend/llvm/tests.rs` emission strings and any `.ll` snapshot
  tests updated in-phase (same commit).
- GPU IR byte-gates unaffected (kernels do not use the print helper
  family); rbv_gate still run as the standing check.
- Commit discipline: `git status`-sweep staging, never file-lists;
  never stage `benchmarks/*_c`, `examples/gpu`, `libstrexp.a`,
  `tests/ffi_*_tests.rs`, `target/cli_probe/*`.

## Exit criterion

A user can import std, print, read/write files, list a directory, draw
random numbers, create and remove temp files, measure time, parse argv,
and run `brievc test` — with zero `frgn` and zero dunder entries in any
core surface, guaranteed forever by two grep-gate tests.
