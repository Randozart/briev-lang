# Frgn Elimination Round 2 — env, node_bridge, cstr doors, http ghost

**2026-09-23** — branch `feat/bad-ack-tier`.

## Problem

The previous test-fix pass RESTORED `__read_file__`/`__write_file__` C functions
(`lib/runtime/briev_rt.c`) and kept the ghost env frgn chain — exactly the
direction the native-runtime plan (your explicit "Briev-native always" decision,
`docs/plans/2026-09-09-briev-native-runtime-and-family-realignment.md`) says to
eliminate. This plan removes the remaining briev_rt.c-backed frgns in scope:

| frgn site | C symbol | Status | Route |
|---|---|---|---|
| `ffi/env.bv` `frgn__getenv_int/_briev` | `__getenv_int`/`__getenv_briev` | **GHOST** (C deleted; backend adapter `mod.rs:3450`) | `Environ#` intrinsic + direct pure-Briev impl call |
| `node_bridge.bv` `briev_read_file_raw/write_file` | `__read_file__`/`__write_file__` | restored (WRONG) | pure-Briev `posix/io.bv` SysCall# I/O |
| `glue/c.bv` `cstr_to_briev`/`str_to_c`/`cstring_concat` | `briev_cstr_to_briev`/`briev_str_to_c`/`briev_cstring_concat` | C-backed | pure-Briev defns (zero-copy `str_to_c`) |
| `ffi/http.bv` `frgn__http_get/_post` | **none** | ghost, zero users | delete |
| `tamer/*.bv` `BrievHost*` | real | Family J host bridge | **keep** (legit external) |
| `ffi/xxhash.bv`, `web/*` | real | vendored lib / `#Web` | **keep** (legit external) |

## Naming decision (approved 2026-09-23)

`Environ#()` — postpended intrinsic function, because it RETURNS a value (the
environ pointer). Precedent: `CancelRequested#()`, `Now#()`, `GetCwd#()` — all
read compiler-owned globals/state and yield a value with a postpended `#`.
`#Environ` (prepended hashword) would be wrong: hashwords are never value
expressions (type positions, op signatures, stream destinations). `Environ`
(plain) would hide special treatment behind ordinary syntax (Rule 3).

`Args#()` is a DIFFERENT, structured contract (argc + argv are two globals;
"args" = argv, not environ) — deferred (CLI argv retired 2026-09-10, zero
users). `Environ#` and a future `Args#` are orthogonal; do not couple.

## Principles applied

- **Interpreter is reference** (Rule 5): `get_env_int!` in the interpreter reads
  `std::env::var` (eval.rs:1470); `Print#` returns 0 (intrinsics.rs:760). The
  pure-Briev env impls walk the captured `@__briev_environ` — equivalent.
- **Intrinsics before frgn** (Rule 4): env needs the compiler-owned
  `@__briev_environ` global; `Environ#` emits the load (same shape as
  `CancelRequested#` reading `@__briev_cancel_flag`).
- **Stdlib is the extension mechanism** (Rule 14): cstr doors become Briev
  defns; `str_to_c` is a zero-copy view (SSO retired, B4 — every String is a
  heap `[len][bytes][NUL]`, so data region IS a C string).
- **No special-casing in the backend** (Rule 15/19): the declare-guard skip
  pattern already used by the print family (`defn_params.contains_key`) is
  reused; no type-name matching.

## Changes

### 1. `lib/runtime/briev_rt.c` — REVERT the restore

Delete the `__read_file__`/`__write_file__` block added in the previous pass
(they were deleted in `56ef1893` as SysCall#-expressible). Do NOT re-add.

Also delete the now-dead C cstr doors after step 6 lands:
`briev_str_to_c`, `briev_cstr_to_briev`, `briev_cstring_concat`.

### 2. `src/intrinsic_signatures.rs` + `src/backend/llvm/intrinsics.rs` — `Environ#`

New intrinsic `Environ#() -> Int` that emits:

```llvm
%env = load ptr, ptr @__briev_environ
%v = ptrtoint ptr %env to i64
```

Registered in `intrinsic_signatures.rs` (like `CancelRequested#`, observable
false, Native Int) and dispatched in `intrinsics.rs`.

### 3. `lib/std/ffi/env.bv` — DELETE; `lib/std/env.bv` — direct impl calls

Delete `ffi/env.bv` (the two ghost frgns). Rewrite `env.bv`:

```
defn get_env(key: String) -> String {
    term briev_getenv_briev_impl((key as Data) as Int, Environ#());
};
defn get_env_int(key: String) -> Int {
    term briev_getenv_int_impl((key as Data) as Int, Environ#());
};
```

`cast_lanes.bv` already defines both impls (pure-Briev over the environ walk).

### 4. `src/backend/llvm/mod.rs` — delete the env adapter, add `Environ#` support

- Remove the `sig.name == "__getenv_int" || "__getenv_briev"` adapter block
  (mod.rs:3450) — no frgn triggers it anymore; the defn calls the impl directly.
- Keep `@__briev_environ` global + captured-environ `_start` as-is.
- `has_stdout_flush` gate stays (already fixed in previous pass).

### 5. `examples/glue-host/node_bridge.bv` — pure-Briev file I/O

- Delete the two frgns.
- `persist(path: CStr)`: `open(str_to_c(...), O_WRONLY|O_CREAT|O_TRUNC, 0o644)`
  → `write(fd, data, len)` → `close(fd)` via `lib/std/posix/io.bv` (all SysCall#).
- `load(path: CStr)`: `open(O_RDONLY)` → `lseek` to size → `read` into
  `Alloc#` → wrap `[len][bytes][NUL]` → `close`. `str_to_c`/`cstr_to_briev`
  come from the now-pure-Briev `glue/c.bv` (step 6).
- Add `import "std/posix/io.bv";` (verify import path resolves).

### 6. `lib/glue/c.bv` — cstr doors become Briev defns

- `str_to_c(s: String) -> Int` = `term (s as Data) as Int + 8` (zero-copy view;
  the `[len][bytes][NUL]` data region IS the C string). Keep the SSO-safe guard
  comment; SSO retired (B4) so all Strings are heap.
- `cstr_to_briev(p: Ptr<Void>) -> String` = `cstr_len` (cast_lanes) + `Alloc#` +
  `Store#` len + `Copy#` + NUL — the pure-Briev twin of the C function.
- `cstring_concat(a, b)` = `cstr_len` a+b + `Alloc#` + two `Copy#` + NUL.
- The `proto C_String` CastTo/CastFrom bindings keep the same names — the
  casting graph's `ExtCallDyn(fn_name)` already resolves the Briev-side name and
  adds the `%state` prefix when the name is in `defn_params` (emit_expr.rs:6406).
- Delete the three frgn lines.
- Verify `defn_liveness` roots the cstr doors (ProtocolDef cast edges already
  rooted — defn_liveness.rs:278).

### 7. `lib/std/ffi/http.bv` + `lib/std/http.bv` — DELETE (ghost, zero users)

No C symbol, no imports. Delete both files. Confirm zero referencers first.

### 8. `src/analysis/defn_liveness.rs` — update the env row

Replace the `frgn__getenv_briev/frgn__getenv_int` rows (added in the previous
pass) with an `Environ#` row → `["briev_getenv_briev_impl", "briev_getenv_int_impl"]`.

## Docs to update

- `docs/architecture/briev-native-runtime.md`: family table rows for the cstr
  doors (C-backed → Briev), env (adapter → `Environ#` intrinsic), file I/O
  (restored → pure-Briev, never re-restore), http (deleted).
- `docs/architecture/defn-liveness.md`: intrinsic→helper table note if the env
  row key changes shape.
- Preserve all existing rationale comments; rewrite, never delete.

## Verification

1. `cargo test --lib` green.
2. Full `cargo test` (previously-failing targets): `c_driver_needs_state`,
   `c_driver_node`, `c_driver_boundary`, `c_driver_cpp`, `c_driver_csharp`,
   `c_driver_go`, `c_driver_java`, `c_driver_lua`, `c_driver_python`,
   `pp_roundtrip_tests`, `termination_diagnostics_test`,
   `async_compiled_events_test` — all green.
3. `grep -rn 'frgn ' lib/std lib/compiler lib/glue examples/` — only legit
   external FFI remains (tamer, xxhash, web).
4. `git grep '__getenv_int\|__read_file__\|briev_str_to_c' src lib` — zero
   backend/runtime references outside the plan's keep list.
5. All 16 QEMU gates still pass (env/cstr are hosted-only paths, but confirm no
   regression in the embedded lanes).
6. Praetor on changed files.

## Stateless-defn mechanism (added during implementation)

The cstr doors as pure-Briev defns forced `BrievState*` into every GLUE export
that called them (the old blanket rule: ANY call to a regular defn ⇒ stateful).
Resolved with a first-class stateless-defn mechanism:

- `compute_defn_needs_state` (export_abi.rs): LEAST FIXPOINT over the call
  graph — a defn is stateless iff its body (directly + through called defns)
  is stateless. Replaces the DFS-with-`visiting⇒true` cycle rule that wrongly
  marked self-recursive stateless defns (cstr_len) stateful.
- Emission: regular defns emit `%state` ONLY when the fixpoint says so
  (mod.rs); every call-site gate (`defn_takes_state`) passes `%state` only to
  state-taking defns (emit_user_call, ExtCall/ExtCallDyn, helpers.rs free,
  str_next_char, cstr_len, task/event helper sites, BadFn declare).
- Scoping: a defn's PARAMS/LOCALS shadow same-named global state fields
  (`byte_at(s, i)` vs `let i: Int = 0`) — the `Identifier` check respects
  lexical binding.
- `bad fn`/`asm fn` are always stateful (the bad ABI passes `%state` first).
- The Briev needs_state pass + its projection mirror the new semantics (a
  `stateless` section carries the Rust fixpoint verdict; the pass marks a
  `C:name` stateful only when `name` is not in it).

ABI consequence: exports whose bodies are pure (echo/greet/identity/join over
stateless cstr doors) now emit WITHOUT `%state` — the clean C ABI is restored.