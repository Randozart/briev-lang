# Core-Usable Cleanup — plain names, .bv-native logic, gap fill

**2026-10-15.** Campaign: make Briev's core language surface daily-driver
clean — zero `frgn` in anything core logic depends on, zero `__`-prefixed
entries visible in `.bv` source, and the remaining daily-use gaps (dir
listing, PRNG, temp files) filled.

Decisions locked with the operator (2026-10-15):

1. **frgn boundary** — "Wherever we can use `.bv`, try. JS interop may be
   required. Same with GLUE — that is all about language interop. We just
   shouldn't be reliant on another language to handle the core logic."
   → frgn survives only where interop IS the feature: `lib/std/web/`
   (JS), `lib/tamer/` (tooling host), `lib/glue/` (the GLUE product
   emits frgn for user FFI). Everything else migrates to `.bv` or dies.
2. **Dunder rename** — plain names, matching cast_lanes' house style
   (`byte_at`, `str_to_int`, `float_to_str`). The `__` convention
   historically marked frgn-adjacent helpers; regretted, removed.
3. **Stub modules** — fill `dir` + `rand` + `temp` now; the other 16
   `os/*.bv` stubs get honest headers, filled by a later campaign.

## What this campaign is NOT

- **Not** the `briev-native-runtime` family (intrinsics whose C bodies
  are the bootstrap exceptions — `Spawn#`, print `--no-std` path,
  `Alloc#`). Rules 3/4 stand: primitives may be C-backed; frgn may not
  be *core logic*.
- **Not** renaming C-ABI pinned symbols (`briev_rt.c`, `@__briev_*`
  globals emitted at IR level), Rust IR gensyms (`%__x`,
  `__variant_`, `__arena_*`, `__trg_epfd`), or dogfood `llvm.bv`
  IR-text strings. Those are not Briev entries.
- **Not** SPEC syntax change: no keyword, delimiter, or grammar
  change. SPEC.md's `frgn ... from #System` design is untouched.
- **Not** the web backend, GLUE FFI, or tamer lanes (live foreign
  work).

## Inventory (verified 2026-10-15)

### frgn — 23 live declarations, none in daily-driver core

| Location | Count | Status |
|---|---|---|
| `lib/std/web/*` (canvas, console, dom, fetch, router, time, pages) | 13 | KEEP — JS interop (decision 1) |
| `lib/tamer/*` (emit, main, vm) | 6 | KEEP — tooling host bridge |
| `lib/std/ffi/xxhash.bv` | 2 | MIGRATE — reimplement XXH64/XXH32 in pure `.bv` (zero importers today) |
| `lib/glue/generator.bv`, `lib/glue/test_bridge.bv` | (0 decls; frgn *text* in generator output) | KEEP — GLUE product |
| prelude (`plugins/parsed/*.bv`), `lib/std` core, `lib/compiler`, `lib/ffi` | 0 | already clean (comments only) |

### Dunder — source-visible Briev identifiers (in scope)

| Class | Definition sites | Reference sites |
|---|---|---|
| Print/stdout/exit family | `lib/std/cast_lanes.bv`: `out defn __stdout_flush`, `__stdout_byte`, `__print_str`, `__print`, `__eprint_str`, `__print_int`, `__print_bool`, `__print_float`, `__print_char`, `__exit`; top-level state `let __stdout_buf`, `let __stdout_fill` (+ their body-locals) | `print_plugin.rs` dispatch; backend emission strings (`intrinsics.rs` ×8, `mod.rs` ×6, `emit_stmt.rs`, `emit_toplevel.rs`); `defn_liveness.rs` `intrinsic_helpers` rows; `tests.rs` emission strings ×6 |
| Argv family | `lib/std/cli.bv`: `defn __argv_has`, `defn __argv_value` | `entry_plugin.rs` (`args!`/`entry!` expansion emits calls) |
| Runtime bridges | `lib/std/cast_lanes.bv`: `out defn __briev_coll_resize`, `__briev_now`, `__watchdog_fail`, `__briev_getcwd`, `__briev_chdir`, `__briev_free`, `__wait_for_trigger__`, `__briev_event_strict_trap` | backend emission (scheduler auto-free → `__briev_free`, idle-tick → `__wait_for_trigger__`, watchdog, capture paths); `defn_liveness.rs` rows |
| Script state | injected by `src/plugin/script_plugin.rs`: `let __script_done`, `node __script_main` | same file (guard/body) |
| Call sites | `tests/tier1/daily_use_smoke.bv`, `tests/fixtures/event_model.bv`, `tests/fixtures/phase4/ffi_print.bv`, `tests/fixtures/inop_{alias,meld,multi,state}.bv`, `examples/ptr-runtime.bv`, `lib/std/cli.bv` | — |
| Filenames | `lib/ffi/__init__.bv` (dunder filename, zero importers found) | — |

Note: `__print_float64` appears in `defn_liveness.rs` rows and backend
strings but NO `.bv` defn was found — Phase 0 probe resolves whether it
is C-side only (`--no-std` symbol) or a missed defn.

### Gaps (daily-driver surface)

- `lib/std/os/dir.bv` — stub (76 duplicate consts, 0 defns);
  `SYS_GETDENTS64 = 217` const already exported by `posix/syscall.bv`.
- PRNG — absent everywhere (no intrinsic, no stdlib).
- `lib/std/os/temp.bv` — stub; mkstemp is not a raw syscall (compose:
  `open(O_CREAT|O_EXCL)` + rand suffix + retry).
- Legacy landmines: root `lib/std/time.bv` = 11 duration helpers but NO
  clock (real clock is `lib/std/os/time.bv`, landed 2026-10-14);
  `lib/std/io.bv` header still teaches `__print_int`.
- 19/21 `lib/std/os/*.bv` are const-only stubs (fs.bv and time.bv are
  the only two real modules).

## Phase 0 — Probes (writes to `/tmp/opencode/core-usable/` only)

No repo changes. Each probe records its finding in this file's
"Phase 0 findings" section before Phase 1 starts.

1. **Duplicate-defn behavior.** `brievc check` a file defining
   `defn print_str(...)` with prelude active. Outcomes:
   (a) loud diagnostic → plain rename safe as-is;
   (b) silent override → MUST add a redefinition diagnostic in
   Phase 1 (a user fn can hijack backend-lowered calls today —
   independent soundness bug).
2. **Mechanical sweep.** Grep-verify every in-scope `__` site across
   `lib/`, `plugins/`, `tests/`, `examples/`, `src/`:
   `defn __`, `out defn __`, `let __`, `@__`-in-`.bv`,
   `"__`-strings in Rust, plugin injections. Reconcile against the
   inventory table above; record deltas. Resolve `__print_float64`.
3. **Name-contract map.** For each runtime bridge, enumerate exact
   Rust emission sites (file:line) that spell the name, so both sides
   rename in one commit. The backend IR-scan net is the safety net
   (loud compile error naming the missing helper) but the map keeps
   commits atomic anyway.
4. **Wrapping arithmetic.** Confirm `Int` (i64) wrapping mul/shift/rotate
   suffices for XXH64 u64 mixing — probe: does overflow trap or wrap?
   Record the correct operator/intrinsic set for Phase 3's xxhash.

## Phase 1 — Dunder purge → plain names

Rename rule: drop the `__` prefix; disambiguate only on collision
(Phase-0 probe 1/2 decide finals). Planned mapping (finals confirmed by
probe):

| Old | New | Old | New |
|---|---|---|---|
| `__print_str` | `print_str` | `__exit` | `exit_prog` |
| `__print_int` | `print_int` | `__argv_has` | `argv_has` |
| `__print_float` | `print_float` | `__argv_value` | `argv_value` |
| `__print_bool` | `print_bool` | `__briev_free` | `rt_free` |
| `__print_char` | `print_char` | `__briev_now` | `rt_now_ns` |
| `__print` | `print_str_raw` (probe: semantics = newline-less generic) | `__briev_getcwd` | `rt_getcwd` |
| `__eprint_str` | `eprint_str` | `__briev_chdir` | `rt_chdir` |
| `__stdout_flush` | `stdout_flush` | `__briev_coll_resize` | `coll_resize` |
| `__stdout_byte` | `stdout_byte` | `__watchdog_fail` | `watchdog_fail` |
| `__stdout_buf` | `stdout_buf` | `__wait_for_trigger__` | `wait_for_trigger` |
| `__stdout_fill` | `stdout_fill` | `__briev_event_strict_trap` | `event_strict_trap` |
| `__script_main` | `script_main` | `__script_done` | `script_done` |

Rules for the rename:

- **Atomic per family**: `.bv` defn + state + every Rust emission
  string + liveness row + plugin injection + call sites + comments in
  ONE commit per family (print/stdout/exit → argv → script →
  runtime-bridges).
- **Out of scope**: C-ABI symbols (`briev_rt.c`), IR-level
  `@__briev_*` globals, IR gensyms, dogfood IR-text strings.
- If a plain final collides with an existing export or a test fixture
  name, pick the distinct final documented in the probe findings —
  never keep a dunder to dodge a collision.
- Redefinition diagnostic (if probe 1 = silent) lands in the FIRST
  family commit, with a behavioral test.

Acceptance (per family commit and at phase end):

- `cargo test --lib` green (updated `tests.rs` emission strings in the
  same commit).
- `fs_smoke`, `daily_use_smoke`, `cli_args_test`, `lsp_smoke` green.
- New **dunder gate test**: assert zero `defn __` / `out defn __` /
  `let __` / `@__` inside `lib/**/*.bv` and `plugins/parsed/*.bv`
  (allowlist: empty). C-side/IR-level names are not in `.bv`, so the
  gate is exact.
- Praetor baseline-diff on every changed Rust file.

## Phase 2 — frgn: `.bv`-native core, interop-only escape

1. **xxhash in pure `.bv`** — rewrite `lib/std/ffi/xxhash.bv`:
   XXH64 + XXH32 per spec (primes, avalanche, rotate), zero `frgn`.
   Test against published vectors:
   `XXH64("", 0) = EF46DB3751D8E999`, `XXH64("", 1) = 1F302D9C2A818D5D`
   (verify exact vectors during implementation against the C source in
   `lib/std/c/xxhash/`, which stays as the C-side reference only if
   still referenced — else removed with the frgn it served).
   **Probe 4 result (2026-10-15, COMPLETED — not blocking):**
   - `Int` (i64) `+`/`*`/`<<` **wrap mod 2^64** (verified: `i64::MAX+1` →
     i64::MIN, `i64::MAX*2` → -2, `-8<<61` → 0). So XXH64's u64 mixing
     needs **no masking** — signed i64 bit patterns == u64.
   - `>>` is **arithmetic** (ashr), not logical: `-8>>1` → -4. Emulate
     logical shift right via mask: `lshr64(x,k) = (x>>k) & ((1<<(64-k))-1)`
     (verified: `-8>>1 & (1<<63)-1` = 0x7FFFFFFFFFFFFFFC). Needed for
     rotate-left and the avalanche `>>33/>>29/>>32`.
   - `&`,`|`,`^` all exist on Int (verified). Bitwise ops on i64 = u64.
   - Hex literals `0x…` lex only when ≤ i64::MAX (from_str_radix overflow
     above); xxHash primes > i64::MAX are written as **signed decimals**
     (P64_1 = -7046029288634856825, P64_2 = -4417276706812531889,
     P64_4 = -8796714831421723037; P64_3/P64_5 positive). XXH32 is mod
     2^32 — mask `& 0xFFFFFFFF` after every op.
   - String bytes via `byte_at(s, i)` (cast_lanes.bv, pure-Briev Load#).
   - Feasible; C stays as vendored reference (compiled by
     `tests/vendored_c_libs.rs`, independent of the Briev frgn).
2. **frgn gate test** — parse all `lib/**/*.bv` line-wise for live
   `frgn`/`export frgn` declarations (comments excluded); allowlist:
   `lib/std/web/`, `lib/tamer/`, `lib/glue/`. Everything else → test
   failure. `lib/std/ffi/` ends frgn-free.
3. **Comment/doc cleanup** — `lib/std/time.bv` placeholder block
   ("could be frgn or defn"), `lib/std/io.bv` header (`__print_int`
   teaching — updated by Phase 1 anyway, re-checked here), any
   `learn-briev/` line implying frgn for ordinary I/O (07-ffi.md is
   the FFI lesson itself — keep).

Acceptance: both gates green; `cargo test --lib`; conformance sweep;
xxhash vector test green (backend AND interpreter if importable there
— follow `json.bv` precedent).

## Phase 3 — Gap fill (dir + rand + temp + legacy)

All pure stdlib over the variadic `syscall` (posix/syscall.bv) or
existing intrinsics — Rule 14, no new intrinsics.

1. **`lib/std/os/dir.bv`** — replace stub with real module:
   - `export defn entries(path: String) -> Result<List<String>, Int>`
     — `Alloc#` buffer + `SYS_GETDENTS64` loop, names parsed from
     `linux_dirent64` (offset 19, reclen at 16), buffer grown on
     `EINVAL`, freed on every path; `Dir#`-style handle via
     `SYS_OPENAT`/`SYS_OPEN` (fs.bv's `fs_open` reused).
   - Contracts follow fs.bv style; entries exclude `.`/`..`? — NO,
     POSIX-honest: include them, document. (Decision: raw fidelity;
     callers filter. Deterministic contract beats magic.)
   - Behavioral test: list a temp dir created by the test (with
     `.bv` files), assert sorted contents.
2. **`lib/std/os/rand.bv`** — splitmix64:
   - `seed_rand(seed: Int)`, `rand_u64()`, `rand_range(lo, hi)
     [lo < hi][lo <= term && term < hi]`.
   - State: one top-level `let` (reactor-state idiom), advanced by
     `rand_u64`. Deterministic: same seed → identical sequence.
   - Tests: same-seed equality, range bounds across ≥1000 draws,
     two consecutive draws differ.
3. **`lib/std/os/temp.bv`** — `create_temp() -> Result<String, Int>`:
   `open(O_RDWR|O_CREAT|O_EXCL, 0600)` with `rand.bv` suffix + bounded
   retry; needs **`remove_file(path)`** added to `fs.bv`
   (`SYS_UNLINK = 87` — add const to `posix/syscall.bv`) so temp files
   do not leak. Test: create → exists → remove → gone.
4. **Legacy surfaces**:
   - Root `lib/std/time.bv`: de-landmine — becomes a re-export of the
     real clock (`import "std/os/time"` + forwarding or file
     replacement per probe of who imports it: `lib/ffi/__init__.bv`
     today); placeholder comment block dies.
   - `lib/std/io.bv` header: correct (Phase 1 already renames
     `__print_int` mentions; verify final text).
   - `lib/ffi/__init__.bv` → `lib/ffi/init.bv` (zero references
     found; probe re-verifies before rename).
   - Remaining 16 stubs: honest header — "Status: constants only, not
     yet implemented — do not import expecting a working surface."
     One sweep commit.

Acceptance: new behavioral tests green under harness-exact
`clang -O3 -flto` link AND at `-O0` if they catch dead-store class
(ir-inversion gate: behavioral tests must NOT rely on `-O3` to pass);
`brievc test` runs the new suites; full battery.

## Phase 4 — End-to-end + close

1. **Mini-CLI acceptance test** under `brievc test`: one `.bv` using
   `entry!`/`args!` + `read_file`/`write_file` + `dir.entries` +
   `rand_range` + `now_ns`/`sleep_ms`, golden stdout — the whole
   daily-driver surface in one program.
2. **Full gate battery**: `cargo test --lib` · `fs_smoke` ·
   `daily_use_smoke` · `cli_args_test` · `lsp_smoke` · conformance
   sweep · `rbv_gate` · Praetor baseline-diff · dunder gate · frgn
   gate.
3. **Docs**: this file's Status section appended (timestamped, never
   retro-edited); architecture comments updated in the same commits as
   the code they describe (`defn_liveness.rs` table header,
   cast_lanes/io headers); root INDEX note if the reference index
   changes; BUGS.md entry for the redefinition-diagnostic fix (if
   probe 1 required it).

## Working rules for this campaign

- One commit per family/feature; `cargo test --lib` before every
  commit; `git add` by `git status` sweep of intended files — never
  file-list guesswork (four staging misses documented this session).
- Never `git checkout --` / `git restore` uncommitted work.
- Praetor baseline-diff per changed Rust file (`/tmp` copies, unique
  dir names — `mod.rs` name collision hazard documented).
- Behavioral tests link with the harness's exact clang line; `-O0`
  variants where the test targets the dead-store class.
- No `todo!()`/stub restorations: every phase ends complete.

## Exit criterion

A user can import std, print, read/write files, list a directory, draw
random numbers, create and remove temp files, measure time, parse argv,
and run `brievc test` — with zero `frgn` and zero dunder entries in any
core surface, guaranteed forever by the two grep-gate tests.

## Session note (2026-10-15 11:11)

Commit `561e4669` briefly replaced this file with a thinner rewrite;
the operator's version (this document) is authoritative — restored with
the exit criterion merged in, recorded as a forward fix (never amend).
At 11:11 the tree also held abandoned in-flight Phase-1 family-1 work
(print/stdout/exit rename across cast_lanes, liveness, intrinsics, mod,
emit_*, print_plugin, daily_use_smoke; mtimes 10:43–10:48, no live
process) following this plan's rename table — continued, completed, and
committed under the normal Phase-1 rules.

## Phase 0 findings (recorded 2026-10-15 11:30)

1. **Duplicate-defn = LOUD error, not silent override.** A user `defn
   byte_at` alongside the prelude export fails with "import name
   'byte_at' conflicts: provided by 'std/cast_lanes.bv' and also by the
   main file — one shared root namespace means the winner would depend
   on import order. Fix: rename one side with a selective import
   (`{ Local: Exported }`) or make the two definitions identical."
   Plain rename is safe as-is; no new redefinition diagnostic needed.
2. **Sweep reconciliation.** Family-1 rename was already ~80% in-tree at
   session start (10:43–48 work); remaining were `tests.rs` fixtures,
   stale comments, and the two missing sibling tests. `__print_float64`
   resolved: C-backed in EVERY mode (`kept_runtime_symbol` list,
   mod.rs:5600; liveness row is a documented no-op — no defn). The
   `__print_*` C-fallback strings in `print_symbol`/`declare` sites are
   the `--no-stdlib` C ABI and stay (out of scope).
3. **Name-contract map (family 1).** Plain helper resolves through
   `print_symbol()`/`defn_state_prefix` (keyed on `defn_params`);
   `stdout_flush` through `has_stdout_flush` (mod.rs:2529-2531, gated
   on presence + liveness); `exit_prog` through the emit_stmt
   endprogram branch + emit_toplevel #14 noreturn attr + liveness
   `mark("exit_prog")`. All four verified in the committed tree.
4. **Wrapping arithmetic** — deferred to Phase 2 (xxhash feasibility);
   families 1-4 do not need it.

## Phase 1 status

- Family 1 (print/stdout/exit): COMPLETE (`66b72b75`).
- Family 2 (argv: `cli.bv` + `entry_plugin.rs`): COMPLETE (`692fb9bc`).
- Family 3 (script state: `script_plugin.rs`): COMPLETE (`b48a74e3`).
- Family 4 (runtime bridges): COMPLETE — `__briev_coll_resize`→`coll_resize`,
  `__briev_now`→`rt_now_ns`, `__briev_getcwd`→`rt_getcwd`,
  `__briev_chdir`→`rt_chdir`, `__briev_free`→`rt_free`,
  `__watchdog_fail`→`watchdog_fail`, `__wait_for_trigger__`→`wait_for_trigger`,
  `__briev_event_strict_trap`→`event_strict_trap`. Call sites resolve the
  plain name when the defn is in the unit, else the C-fallback symbol
  (presence-branch via `bridge_symbol`/`defn_params` — C declares in
  `emit_toplevel`/`mod.rs` keep their `__` ABI names). Two pre-existing
  latent bugs fixed in the same pass (see BUGS.md 2026-10-15): the
  stateless `event_strict_trap` call (missing `%state`) and the void
  `@__watchdog_fail()` second-engine shape.
- Dunder grep-gate test: COMPLETE — `tests/core_usable_gate.rs` (2/2):
  `no_dunder_defn_let_in_shipped_bv` (zero `defn __`/`out defn __`/`let __`
  under `lib/**/*.bv` + `plugins/parsed/*.bv`, allowlist empty, comments
  stripped) and `no_dunder_llvm_symbol_in_shipped_bv` (zero live `@__`
  outside the dogfood `lib/compiler/**`, which legitimately writes IR-text
  string literals). Verified non-vacuous (injected `defn __leak` → fails,
  reverted → passes). Phase 1 COMPLETE.
