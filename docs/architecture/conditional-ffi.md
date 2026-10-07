# Conditional FFI Architecture
## 2026-07-25

## The Four FFI Levels

Briev provides four FFI declaration levels:

| Level | Keyword | Must link? | Blocking? | Return | Guard required? |
|-------|---------|-----------|-----------|--------|-----------------|
| Required | `frgn` | Yes | Yes | Normal | No (guaranteed) |
| Optional | `frgn?` | No | Yes | Normal | **Yes**: `fn?` |
| Fire-forget | `frgn!` | No | **No** | `Void` | No (no-op on miss) |
| Delivery | `frgn?!` | No | **No** | `Bool` | No (returns false on miss) |

### `frgn` — Required FFI

The original declaration. The function must link against the target, or
compilation fails. `fn?` always returns `true`. No guard needed.

### `frgn?` — Optional FFI

Links if available. The compiler **requires** a `fn?` guard before any call:

```briev
frgn? gpu_kernel(n: Int) -> Int from #System fallback -1;

export defn run(n: Int) -> Int {
    when gpu_kernel? {
        term gpu_kernel(n);     // OK — guarded
    };
    term 0;
};
```

If a function body calls a `frgn?` symbol without a `fn?` guard on every
path, the compiler emits:

```
error: frgn? 'gpu_kernel' not guarded by gpu_kernel?
```

The narrowing pass proves `gpu_kernel?` at compile time per target:
- On Linux with the library: `true`, guard eliminated, direct call.
- On WASM without the library: `false`, entire branch eliminated, fallback runs.

### `frgn!` — Fire-and-Forget

Returns immediately. The return type is always `Void`. If the symbol doesn't
link, the call is silently skipped at the codegen level — no LLVM IR emitted.

```briev
frgn! emit_log(msg: String) -> Void from #System fallback;

export defn process() {
    emit_log("processing started");  // non-blocking, no guard needed
    // ... do work ...
};
```

### `frgn?!` — Fire-and-Forget with Delivery

Returns `Bool(true)` if the call was dispatched, `Bool(false)` if the symbol
doesn't link or the dispatch failed. Never blocks.

```briev
frgn?! send_telemetry(value: Int) -> Bool from #System fallback false;

export defn report(v: Int) -> Bool {
    term send_telemetry(v);  // returns true if dispatched, false otherwise
};
```

## The `fn?` Existence Expression

`fn_name?` evaluates to a `Bool`:

- **`frgn` / `defn`:** always `true` (guaranteed to exist)
- **`frgn?` / `frgn!` / `frgn?!`:** `true` if the symbol linked on this
  target, `false` otherwise

At compile time, `fn?` is a constant — the linker resolution happens during
compilation. The narrowing pass const-folds it:

```rust
// For frgn? bindings where the linker resolved the symbol:
fn_exists = NavValue::Bool(true);   // max=1, min=1
// For frgn? bindings where the linker could not resolve (or frgn! without link):
fn_exists = NavValue::Bool(false);  // max=0, min=0
```

This means `when fn_name? { ... }` is fully eliminated on targets where the
function doesn't exist. Zero runtime cost.

### `term expr?` — Conditional Term

`term expr?` tries to `term expr`. If `expr` can't be resolved (e.g., it calls
a `frgn?` that doesn't exist), execution continues to the next statement:

```briev
export defn handle(x: Int) -> Int {
    term posix_fn(x)?;         // try POSIX
    term fallback(x);          // fallback
};
```

Desugars to:

```briev
export defn handle(x: Int) -> Int {
    when posix_fn? { term posix_fn(x); };
    term fallback(x);
};
```

The narrowing pass const-folds `posix_fn?` per target, eliminating the dead
branch and unwrapping the `term expr?` into a direct `term`.

## Hashword Sources

`from "c"` is replaced by `from #System`.

**`#System` is the base protocol.** It abstracts "the platform's standard
system library" — it maps to different libraries per target but means the same
thing to the compiler. (2026-10-06: any other `#<Name>` protocol is a GLUE
target resolved by name — `lib/glue/<name>/glue.dbv` — and links no system
library.)

`#System` resolves per target:

| Target | `#System` resolves to |
|--------|----------------------|
| `x86_64-linux` | `c` (libc, via `-lc`) |
| `aarch64-linux` | `c` (libc, via `-lc`) |
| `x86_64-macos` | `System` (libSystem, via `-lSystem`) |
| `aarch64-macos` | `System` (libSystem, via `-lSystem`) |
| `wasm32-wasi` | `wasi_snapshot_preview1` |

The compiler maps `#System` to the appropriate library per target in
`config/protocols.dbvl`:

```toml
[x86_64-linux]
"#System" = "c"

[wasm32-wasi]
"#System" = "wasi_snapshot_preview1"
```

Any other `#<Name>` protocol is a GLUE target resolved by name
(`lib/glue/<name>/glue.dbv`) and links no system library — it is not an
entry in `config/protocols.dbvl`. (2026-10-07: `#Web` is being retired as a
protocol; see `web-host-boundary-decision-record.md`.)

## `#Link<name>` — Direct System Library Linking

`from #Link<name>` links a system library directly by name — no per-target
config, no registry lookup. The canonical briev form is:

```briev
frgn MessageBoxW(h: Int, text: Int, caption: Int, type: Int) -> Int from #Link<user32>;
```

The compiler passes `-l<name>` to the linker. This works on all platforms
clang/gcc support — `-lz` for zlib, `-luser32` for Windows USER32,
`-lwasi_snapshot_preview1` for WASI, etc.

### How it differs from `from #System`

| Feature | `from #System` | `from #Link<z>` |
|---------|---------------|-----------------|
| Per-target resolution | Yes — different lib per target | No — always `-l<name>` |
| Config file | `config/protocols.dbvl` | None |
| Example | Linux: `-lc`, macOS: `-lSystem` | `-lz` on any platform |

### How it differs from `from "path"`

`#Link<name>` is a *linker directive*, not a file path. The symbol is assumed
to be in a system-installed library that the linker can resolve via `-l`.
Use `from "path/file.c"` for source files you control.

### Canonical patterns

| Pattern | Use case |
|---------|----------|
| `from #System` | CRT functions: `printf`, `malloc`, `memcpy` |
| `from #Link<z>` | Common system libs: zlib, libpng, libjpeg |
| `from #Link<user32>` | Windows platform APIs |
| `from #Link<wasi_snapshot_preview1>` | WASI imports |
| `from "path/file.c"` | Your own source files |
| `from <registry_entry>` | Registry-installed (via `brievc registry add`) |
| `from <stdlib/file.bv>` | Compiler stdlib |

See `docs/plans/2026-07-26-tamer-zero-c-and-static-memory.md` §1f for the
registry design (`brievc registry add`, lookup order, platform paths).

## Compile-Time Guard Safety

The safety pass (`src/analysis/frgn_guard.rs`) walks each function body after
typechecking, checking that every `frgn?` call is guarded:

```rust
fn check_frgn_guards(body: &[Statement], universe: &TypeUniverse) -> Result<(), String> {
    // Collect all Expr::Call(name) sites
    // For each, look up the ForeignBinding
    // If binding is frgn?/frgn!/frgn?!, check for Expr::Exists(name) guard
    // on all paths leading to the call
}
```

This is integrated into `compile.rs` after typechecking, before codegen.
It's a linear-time walk — no deep analysis needed. The guard pattern is
always `when fn? { term fn(...); }`.

## Pipeline

```
Parser
  → frgn?/frgn!/frgn?! parsed with flags
  → fn? parsed as Expr::Exists
  → from #System parsed as FromSpec::Protocol("#System")
  ↓
Type checker
  → ForeignBinding flags resolved
  → fn? typed as Bool
  ↓
FRGN guard check (NEW)
  → verify frgn? guarded by fn?
  → Error if unguarded
  ↓
Narrowing pass
  → const-fold fn? based on linker resolution
  → term expr? desugared + dead branch eliminated
  ↓
Normalizer → Codegen
  → ProtoSource → linker directives
  → frgn! emits non-blocking call IR
  → frgn?! emits non-blocking call with delivery check IR
```
