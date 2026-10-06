# The Folio — Project Manifest, Dependencies, and Lock

**Date:** 2026-10-06 (package/module v0)
**Plan:** `docs/plans/2026-10-06-package-module-v0.md`
**Code:** `src/manifest.rs`, `src/packages.rs`, `src/import_resolver.rs`

A **folio** holds brievs — the project manifest and its lock. The name is a
deliberate pun on Cargo; the file names are `folio.toml` and `folio.lock`.

## `folio.toml`

```toml
[project]
name = "myapp"
version = "0.1.0"
entry = "src/main.bv"

[dependencies]
bar = { path = "../bar" }
foo = { git = "https://github.com/x/foo", tag = "v1.2" }
# foo = { git = "https://github.com/x/foo", rev = "<sha>" }
# foo = { git = "https://github.com/x/foo", branch = "main" }
```

- `[project]` — name, version, entry module. `entry` is documentation for v0.
- `[dependencies]` — each dep is exactly one of `path`, `git`, or `registry`.
  - `path` — a directory relative to the folio; local edits, never locked.
  - `git` — a git URL. Pin with `rev`, `tag`, or `branch`; with none, the
    default branch is used and the resolved commit is recorded in the lock.
  - `registry` — parsed but **unsupported** in v0 (clear error); the registry
    index is a later layer.
- `[target.<name>]` — target profiles (`triple`, `linker_script`, `entry`,
  `isr_mechanism`), consumed by `brievc build --target/--all-targets`.

`folio.toml` replaces the former `briev.toml`; there is no alias.

## `folio.lock`

```toml
version = 1

[package.foo]
source = "https://github.com/x/foo"
rev = "394d92c319dbbb42ca366b21b016b32d36db0017"
hash = "f7a95fa7…"
```

- Only **git** deps are locked (path deps are local edits).
- `rev` is the resolved commit; on the next build it is used in preference to
  the remote head, so builds are reproducible.
- `hash` is `blake3(source + "\n" + rev)` — an integrity record following the
  macro-lock hashing pattern (`src/macros/lockfile.rs`).

## Resolution order

1. Manifest pin (`rev`/`tag`/`branch`) wins.
2. Else the locked commit — reproducible.
3. Else the remote default branch (`HEAD`).

`brievc update` ignores (2), refreshing unpinned deps to head and rewriting the
lock. `brievc build` writes the lock; `brievc check` is **strictly read-only**
(it never writes `folio.lock`).

## Cache and offline

Git deps clone once per URL into `~/.briev/cache/git/<blake3(url)>/` and are
checked out at the pin. Once the cache is warm and the lock exists, builds are
offline. Transport is the `git` CLI (like clang for codegen) — no Rust
dependency. A missing `git` is a clear error.

## Import roots

Each dependency contributes its **search root**: `<dep>/src` when that directory
exists, else `<dep>`. Roots are canonical **absolute** paths — the import
resolver joins each search path onto the source directory, so a relative root
would be mis-appended. Convention: a dep named `foo` exposes `src/foo.bv`, so

```
import <foo>;              // -> <dep>/src/foo.bv
import <foo/sub>;          // -> <dep>/src/foo/sub.bv
```

## CLI

| Command | Effect |
|---|---|
| `brievc add <name> --git <url> [--tag\|--rev\|--branch <r>]` | add a git dep, resolve, lock |
| `brievc add <name> --path <dir>` | add a path dep |
| `brievc remove <name>` | remove a dep and rewrite the lock |
| `brievc update` | refresh unpinned git deps, rewrite `folio.lock` |

## Entry convention

A project entry is a `node` with the `beginprogram` marker (SPEC §11.5.1) —
never a compiler-special-cased `Main`:

```
node entry [beginprogram][true] {
    println!("Hello, Briev!");
    term;
};
```

`brievc init <name>` scaffolds exactly this plus a `folio.toml`.
