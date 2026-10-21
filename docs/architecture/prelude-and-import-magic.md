# Prelude, Import#, and Stdlib Auto-Import

**Date**: 2026-06-14 — Phase 14
**Author**: Design discussion between maintainer and OpenCode

## Background

Previously, every Briev program had to manually import stdlib modules:

```briev
import { Option, Some, None } from "std/option.bv";
import { Result, Ok, Err } from "std/result.bv";
```

This was unwieldy for new projects and violated the principle that common
building blocks should be readily available. The solution had to satisfy
the **No Magic** philosophy — nothing hidden, nothing hardcoded, everything
traceable through transparent paths.

## Design Summary

Three-tier system:

| Tier | Mechanism | What it provides | Control |
|------|-----------|------------------|---------|
| Built-in | Compiler-injected | `Option<T>`, `Some`, `None`, `Result<T,E>`, `Ok`, `Err` | Always present. Required for FFI error handling. |
| Auto-core | Compiler scans `{BRIEV_STDLIB_PATH}/std/core/` | All "safe" modules (pure `defn`/`enum`, no `frgn`/`link`/`trg`) | \`--disable-plugin prelude\` disables |
| `import#` | Compiler-relative path resolution | Anything in `BRIEV_STDLIB_PATH` | Explicit opt-in per module/glob |

## Directory Structure

The stdlib is split into two directories under `{BRIEV_STDLIB_PATH}/std/`:

```
std/
  core/          # Safe — pure Briev, no frgn/link/trg — AUTO-IMPORTED
    option.bv
    result.bv
    collections.bv
    char.bv
    bits.bv
    hashmap.bv
    hashset.bv
    stack.bv
    queue.bv
    iterator.bv
    ptr.bv
    string_builder.bv

  ffi/           # Has frgn/link/trg — requires explicit import#
    io.bv
    string.bv
    process.bv
    time.bv
    json.bv
    http.bv
    tty.bv
    out.bv
    shm.bv
    xxhash.bv
    env.bv
    encoding.bv
    system.bv
    metro_bridge.bv
    briev_rt.bv

  ext/           # Pure Briev but niche — requires explicit import#
    metropolitan_ffi.bv
    metrod_registry.bv
```

## `import#` Syntax

The `#` suffix on `import` changes the search root from project-relative to
compiler-relative:

| Syntax | Search root | Use case |
|--------|-------------|----------|
| `import "foo.bar"` | Project paths (`lib/`, `imports/`, `./`) | Project-local modules |
| `import# "std/ffi/string.bv"` | `BRIEV_STDLIB_PATH` | Compiler's stdlib |

The `#` suffix is parsed in the parser: after consuming `import`, check for
a trailing `Hash` token.

### Glob Support

The path may end with `*` (single-directory glob) or `**` (recursive glob):

| Pattern | Meaning |
|---------|---------|
| `import# "std/ffi/*"` | All `.bv` files in `std/ffi/` |
| `import# "std/**"` | All `.bv` files in all subdirectories of `std/` |
| `import "core/*"` | Project-relative glob (searches project paths) |

Glob expansion is handled by the import resolver by listing the matched
directory and generating individual import nodes for each `.bv` file.

## Hardcoded Option + Result

Two enum types are always available in the compiler's built-in namespace,
even under `--disable-plugin prelude`:

- `Option<T>` with variants `Some(T)` and `None`
- `Result<T, E>` with variants `Ok(T)` and `Err(E)`

These are required because the FFI system uses them for error handling
(foreign functions that can fail return `Result`, optional foreign bindings
use `Option`). The definitions in `lib/std/core/option.bv` and
`lib/std/core/result.bv` remain as documentation and source of truth, but
the compiler guarantees their availability without imports.

## Auto-Import of `core/`

By default (without `--disable-plugin prelude`), the compiler:

1. Locates `{BRIEV_STDLIB_PATH}` (from `--stdlib-path`, `BRIEV_STDLIB_PATH`
   env var, or executable-relative default)
2. Lists all `.bv` files in `{BRIEV_STDLIB_PATH}/std/core/`
3. Injects a synthetic `import# "std/core/<file>"` for each file
4. Resolves through normal import resolution (recursive, dedup'd)

The user never sees these imports in their source, but they can inspect
what's available by reading the `core/` directory. The `--disable-plugin prelude` flag
disables this entirely.

## `--disable-plugin prelude` Flag

Replaces the existing `--no-std` / `--no-stdlib` flags. The `--disable-plugin prelude` flag:

1. Disables auto-import of `core/`
2. Disables FFI binding resolution for std paths
3. Does NOT remove built-in `Option`/`Result` (they're required for FFI)
4. Does NOT prevent `import#` (compiler-relative imports still work)

## Bootstrap Type Universe

**Added 2026-06-30 (Phase C):** 14 primitive types (`Int`, `UInt`, `Int8`,
`UInt8`, `Int16`, `UInt16`, `Int32`, `UInt32`, `Float`, `Float64`, `Bool`,
`Char`, `String`, `Data`) are defined in `lib/std/types/bootstrap.bv` using
the `<~` Annotation Arrow syntax.

The compiler auto-injects `import# "std/types/bootstrap.bv"` for every `.bv`
file, ensuring all built-in types are available without explicit imports.
This happens alongside the `core/` auto-import and is also gated by `--disable-plugin prelude`.

Previously these types were hardcoded as Rust `Vec<ResolvedType>` literals in
`init_primitives()`. Now they are single-source-of-truth `.bv` declarations.

## Implementation Phases

### Phase 1: Directory Restructure
- Create `lib/std/core/` and `lib/std/ffi/` and `lib/std/ext/`
- Move files and update any internal import paths

### Phase 2: AST + Parser
- Add `is_magic: bool` field to `ast::Import`
- In `parse_import()`, detect trailing `#` after `import`
- Keep `import#` and `import` paths unified through parse

### Phase 3: Import Resolver — Glob Support
- In `resolve_import()`, detect `*` or `**` in last path segment
- For `*`: list directory, collect `.bv` files
- For `**`: list recursively, collect `.bv` files
- Generate and resolve individual imports per file
- Cache results as with normal imports

### Phase 4: Import Resolver — Magic Path Resolution
- When `is_magic` is true, resolve path relative to `BRIEV_STDLIB_PATH`
- Add `stdlib_path: Option<PathBuf>` field to `ImportResolver`
- Add builder method `.with_stdlib_path(path)`

### Phase 5: Import Resolver — Auto-Core Import
- Add `use_stdlib: bool` field to `ImportResolver` (default: true)
- In `resolve_imports()`, if `use_stdlib`, inject auto-core imports first
- Also handle when `BRIEV_STDLIB_PATH` env var is set

### Phase 6: CLI
- Add `--disable-plugin prelude` flag (replaces `--no-std` / `--no-stdlib` internally)
- Thread through to import resolver at all call sites
- Keep `--stdlib-path` for custom stdlib location

### Phase 7: Option + Result Hardcoding
- Add built-in type definitions for `Option` and `Result` in the
  compiler's type universe / initial state
- These are always available, no import required

### Phase 8: Tests (Complete)
- ✅ Parser tests for `import#` syntax (`test_parse_import_magic`, `test_parse_import_magic_with_items`, `test_parse_import_magic_glob`, `test_parse_import_normal`)
- ✅ Import resolver tests for glob expansion (`test_glob_import_non_recursive`)
- ✅ Import resolver tests for magic path resolution (`test_magic_import_resolution`)
- ✅ Auto-core import tests (`test_auto_core_injection`, `test_auto_core_disabled`)
- ✅ Built-in Option/Result tests (`test_synthesize_builtin_types_basic`, `test_synthesize_builtin_types_no_duplicate`, `test_option_variants_correct`, `test_result_variants_correct`)

## Implementation Notes

### Auto-Core Whitelist
Currently only `ptr.bv` is auto-imported. Most other `std/core/*.bv` files use Briev syntax
features (unification, collection mutation) that the Rust parser doesn't fully support.
These are documented in BUGS.md (see 2026-06-14 entries).

### Option + Result Injection
Injected via `Program::synthesize_builtin_types()` in `src/ast.rs` after import resolution,
before `synthesize_init_txn()`. The injection checks for existing definitions to avoid
duplication. Called from both `run_llvm_compile` and `run_compile_unified` in `src/main.rs`.

### Glob Expansion
Implemented in `ImportResolver::resolve_glob()`. Non-recursive `*` lists direct children of
the matched directory. Recursive `**` walks all subdirectories via `walkdir`. Magic globs
resolve relative to `BRIEV_STDLIB_PATH`; non-magic globs resolve relative to project paths.

## No Magic Compliance

Each mechanism is explicitly traceable:

| Mechanism | Why it's not magic |
|-----------|-------------------|
| Built-in Option/Result | Required for FFI; definitions in stdlib are the source of truth |
| Auto-core import | Uses `BRIEV_STDLIB_PATH` — a configurable path. Scans a real directory. Resolves through normal imports. |
| `import#` | `#` suffix explicitly signals compiler-relative path. Same resolution as normal imports, different root. |
| Glob `*`/`**` | Standard glob semantics. No hidden allowlists. List directory and import everything found. |

## Backward Compatibility

Existing `import "std/option.bv"` will still work because the import
resolver's project-relative search paths (`lib/`, `imports/`, `./`) still
apply. The `core/`, `ffi/`, `ext/` split is internal to `BRIEV_STDLIB_PATH`;
project-relative imports can still resolve old paths if a project has its
own `lib/std/` mirror.

The `--disable-plugin prelude` flag replaces `--no-std` / `--no-stdlib`
(kept as aliases for backward compatibility at the CLI level).

### Relative code paths (2026-10-21)

Code specifiers starting with `./` or `../` are filesystem paths relative to
the importing source file: directory traversal and filename dots are preserved
after removing a recognized code extension. Bare specifiers retain dotted
namespace expansion. Import-resolver regression tests cover parent/current
paths, extensionless imports, dotted filenames, and bare dotted namespaces.
