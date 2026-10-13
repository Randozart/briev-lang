# Daily-driver quick wins — E2E gate, Args#, os/fs, LSP

**Date:** 2026-10-12 · **Status:** ACTIVE
**Context:** daily-driver gap analysis (stdlib/tooling/dogfood scout, this
session). Quick wins approved in order: build the verification net first,
then the two Tier-1 blockers (CLI args, file I/O), then the LSP wiring.

## Why this order

1. **E2E gate first** — the conformance sweep is parse+check only; nothing
   compiles+runs the everyday-features corpus (`tests/tier1/*.bv`) in CI.
   The net must exist before we add language surface (Rule 9: tests or it
   doesn't exist; Rule 20: measure/gate before you build).
2. **Args#** — `entry!`/`args!` expand to `__argv_has`/`__argv_value`,
   deleted with `briev_rt.c` (Family F). No CLI tool can parse argv today.
   The backend already stores `@__briev_argc`/`@__briev_argv`
   (`src/backend/llvm/mod.rs:3829`) — the intrinsic is a global load, the
   exact `Environ#` recipe (`src/backend/llvm/intrinsics.rs:201`).
3. **os/fs** — `lib/std/os/*.bv` are 21 stub files of duplicated consts;
   the only working system layer is raw `posix/io.bv` (Ptr API). Real
   String-path file I/O is the second hard blocker.
4. **LSP** — `src/lsp.rs` (926 lines: hover/definition/completion/
   documentSymbol/diagnostics) is dead: the VS Code client spawns
   `briev lsp` (`syntax-highlighter/client/extension.js:44`) but
   `main.rs` has no `"lsp"` arm. Minutes of wiring, large daily-driver win.

## Increment 1 — daily-use E2E gate

New `tests/daily_use_smoke.rs` (executes plan `2026-09-28-daily-use-sweep…`
step 1, never done):

- For each `tests/tier1/*.bv`: `brievc build --llvm` → harness-exact clang
  link (`-O3 -flto -march=native -ffast-math -fdata-sections
  -ffunction-sections -Wl,--gc-sections` + `lib/runtime/briev_rt.c`) → run →
  assert exit 0. Pattern of record:
  `tests/stdout_flush_tail_test.rs` (`build_and_run` helper).
- `daily_use_smoke.bv` additionally gets a golden stdout assertion
  (flagship fixture).
- Any failing fixture is triaged: fix the compiler bug, or quarantine with a
  BUGS.md entry — never silent skip.
- Gate value: pins hashmap/stringbuilder/collections/char as compile+RUN
  (not just parse+check), and catches regressions from Increments 2–3.

## Increment 2 — `Args#` intrinsic + `cli.bv` revival

Backend side (exact `Environ#` model, two globals already captured):

- `Argc#()` → Int: `load i32 @__briev_argc` + zext.
- `Args#()` → Int (argv base pointer as i64): `load ptr @__briev_argv` +
  ptrtoint.
- Sites: signature + gate list (`src/intrinsic_signatures.rs`, the
  `Environ#` entries at :287 and :571), emission arms
  (`src/backend/llvm/intrinsics.rs` beside `Environ#` :201), normalizer
  registration (`src/backend/llvm/normalizer.rs:140` is the model).

Stdlib side (`lib/std/cli.bv`, currently a retired-stub comment):

- `arg_count() -> Int`, `arg(i) -> String`, `has_flag(name) -> Bool`,
  `flag_value(name) -> Option<String>` — pure Briev over the intrinsics,
  reusing the `cstr_len` + environ-walker C-string→String technique
  (`lib/std/cast_lanes.bv:1138`), `option.bv` for the Option.
- `entry_plugin.rs`: rewrite `__argv_has`/`__argv_value` expansions to the
  new `cli.bv` defns (lowercase `__argv_*` names violate the intrinsic
  naming rule; the plugin is compile-time expansion, it targets stdlib).

Example + tests:

- `examples/cli-args.bv` — flags + positional (the first real argv program).
- Compile+run argv assertions through the E2E harness (`--flag value`
  round-trip); a regression test pinning that the old expansion no longer
  link-fails.

## Increment 3 — `os/fs.bv` real layer

Replace the stub with a String-path API over `posix/io.bv` + `string_c.bv`:

- `read_file(path: String) -> Result<String, Int>` (open → read loop with
  Alloc# growth → close; error = the syscall's negative return)
- `write_file(path: String, data: String) -> Result<Int, Int>`
  (O_CREAT|O_WRONLY|O_TRUNC)
- `file_exists(path: String) -> Bool`
- `read_lines(path: String) -> Result<List<String>, Int>`
- String→C-string via existing `string_c.bv`; Result from `result.bv`
  (errors are values — SPEC §16.3).
- Tests: temp-file write→read→assert round-trip through the E2E harness;
  `brievc check` clean (conformance sweep).
- Honest status lines in `lib/std/README.md` (it currently documents
  modules that do not exist); other 20 `os/*.bv` stubs stay stubs —
  net/time/dir are later tier.

## Increment 4 — LSP wiring

- `main.rs` dispatch: `"lsp" => { lsp::run_lsp_server(<mode>); Ok(()) }`
  + usage text (the extension already speaks stdio framing).
- Smoke test: spawn `briev lsp`, send an LSP `initialize` request
  (Content-Length framing), assert a `capabilities` response.

## Out of scope (next tiers, not this plan)

Dogfood 10/12 `lib/compiler/*.bv` check failures, segfault BUGS.md:632
(let-bound inserts), `brievc test` runner, clock/sleep, networking wrappers,
dyn list push-site boxing (plan `2026-10-11-coll-element-size.md` ADDENDUM 3),
tutorial refresh.

## Gates (every increment)

- `cargo test --lib` green (new tests included).
- `bash benchmarks/rbv_gate.sh` PASS (web lane untouched, cheap regression).
- Praetor: no NEW diagnostics in changed directories.
- For language-surface changes (Increment 2): the E2E argv test is the
  behavior gate; `--no-stdlib` bootstrap unaffected (new intrinsics are
  opt-in by name).
