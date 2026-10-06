# Package/Module v0 + Install Story — the Folio

**Date:** 2026-10-06
**Status:** Active
**Umbrella:** `docs/plans/2026-10-04-three-surfaces-functional.md` Phase 1.2/1.3
**Predecessors:** Phase 1.1 json.bv (`734a8dac`, `cf38ced2`, `deacdf79`); ledger
sweep (`docs/plans/2026-10-06-bugs-ledger-sweep.md`).

## Goal

A stranger can, on a clean box: install the compiler, scaffold a project, add a
dependency, import its modules, and run — with a deterministic lock.

Naming (decided 2026-10-06): the project manifest is **`folio.toml`** and the
lock is **`folio.lock`** (a folio holds brievs; the Cargo pun is intended).
`briev.toml` is renamed, not aliased.

Entry naming (decided 2026-10-06): scaffolds use **`beginprogram`**
(`node entry [beginprogram][true] { … };`, SPEC §11.5.1) — never a
compiler-special-cased `Main`. `Main` is not special in the compiler today; the
rule is to keep it that way.

## Decisions taken

1. **Dependency model:** Cargo-style git + path deps, pinned in `folio.lock`.
   `foo = { git = "https://…", tag = "v1.2" }`, `bar = { path = "../bar" }`.
   Registry-index deps remain parsed but unsupported (clear error).
2. **Install scope:** installer + `init` + static-build docs, no CI/release
   pipeline (hosting decisions deferred).
3. **Order:** package v0 first, then install story.
4. **Folio scope:** `folio.toml` + `folio.lock` only. `.briev/` project dirs,
   `BRIEV_*` env vars, and the local registry keep their names for v0.
5. `folio.toml` fully replaces `briev.toml` (it already carries `[project]`,
   `[dependencies]`, `[target.*]`).

## Phase 1.2 — package/module v0

### Manifest (`src/manifest.rs`)

- Add `Dependency::Git(GitDependency { git, rev, tag, branch })` beside `Path`.
- `find_manifest` searches **`folio.toml`**; `create_default_manifest` writes it.
- `Registry` stays parsed; resolving it is an explicit unsupported error.

### New lib module `src/packages.rs`

Name avoids the existing binary-side `src/deps.rs` (z3/dwarfdump installer).

- `pub enum ResolveMode { Build, Check }`.
- `pub struct ResolvedDep { name, source, root: PathBuf, rev: Option<String> }`.
- `resolve_dependencies(&Manifest, project_root, ResolveMode) -> Result<Vec<ResolvedDep>, PackageError>`:
  - `path` → `project_root.join(path)` (exists-checked).
  - `git` → cache `~/.briev/cache/git/<blake3(url)>/<rev>`; clone/checkout with
    the `git` CLI (consistent with `install-deps`' curl/wget — no new Rust
    deps); resolved SHA from `git rev-parse HEAD`.
  - `ResolveMode::Build` may write `folio.lock`; `Check` never mutates.
- `folio.lock` (TOML): `{ source, rev, hash }` per dep; hashing follows the
  macro-lock pattern (`src/macros/lockfile.rs`). Deterministic; offline reuse
  when lock + cache exist.
- `dependency_search_roots(deps)`: `<deproot>/src` if it exists, else
  `<deproot>` — so `import <dep>` finds `<root>/<dep>.bv` and
  `import "<dep>/sub.bv"` finds submodules.

### Wiring

`ImportResolver` already supports this (`add_search_path`, absolute paths pass
through `Path::join`).

- `src/compile.rs` (build): after parse, `find_manifest(file_dir)` → resolve
  deps (Build) → add roots → write lock if changed.
- `src/pipeline.rs::parse_and_check` (check): same in Check mode.
- `src/library.rs::parse_and_check` (tests): accept a project root so dep-import
  tests run offline against path deps.

### CLI

- `brievc update` — refresh git deps and rewrite `folio.lock`.
- `brievc add <name> --git <url> [--tag/--rev/--branch] | --path <p>` and
  `brievc remove <name>`, reusing `Manifest::add_dependency`/`remove_dependency`.

### Tests

- Manifest parse: git + path deps.
- Path-dep import end-to-end (temp project).
- Git dep: a local `file://` repo fixture (no network) resolves + locks; a
  second resolve is offline; lock determinism; `Check` writes nothing.
- Import by name from a dep (`import <dep>` → `<root>/<dep>.bv`).

## Phase 1.3 — install story

- `scripts/briev-install`: install **`brievc`** (currently copies the wrong
  artifact name `briev-compiler`) to `$PREFIX/brievc` + a `briev` alias; verify.
- Add `brievc --version` / `-V` (`CARGO_PKG_VERSION`).
- `brievc init`: write `folio.toml` + a runnable `src/main.bv`:
  `node entry [beginprogram][true] { … };` (today's `defn main` is not an entry —
  `llvm/mod.rs:5380` — so the scaffold builds to an empty program).
- Static build: document/verify musl or `+crt-static`. `brievc` has no LLVM link
  dependency (it emits IR text), so the binary can be static; producing
  executables shells out to `clang`/`llc`/`llvm-objcopy`/`cc`
  (`compile.rs:2544,2588,2796`), so the installer checks for a system LLVM
  toolchain and messages clearly.
- Clean-box smoke script: `build --release → install → init → run`, timed.

## Documentation maintenance (Rule 3)

- Rename `briev.toml` → `folio.toml` in: `spec/SPEC.md:2442,3292`,
  `docs/architecture/macro-system.md:474`,
  `docs/architecture/bad-dialect.md:359`, `examples/README.md:65`, and the
  renamed `examples/bad/folio.toml`.
- **Leave historical records untouched** (AGENTS Rule 13): `docs/plans/*.md`
  that mention `briev.toml`, and `spec/archived/old_docs/**`.
- New architecture note: this plan + a short `docs/architecture/folio.md`
  (manifest schema, lock semantics, dependency resolution order, offline rule).

## Gates

- `cargo test --lib` green per landing (baseline 2896 at plan time).
- Praetor no new diagnostics on changed files.
- `brievc run` on a scaffolded project prints the expected output.
- Installer works from a scratch prefix; clean-box smoke documented.

## Risks

- `brievc` shells out to clang/llc — "single binary" describes the compiler,
  not a self-contained toolchain; the installer must check the prerequisite.
- `src/deps.rs` name collision — new logic goes in `src/packages.rs`.
- `check` must not mutate the filesystem — enforced by `ResolveMode::Check`.
- Git transport requires `git` on PATH (like clang); offline only with
  lock + cache.

## Sequencing

1. Rename `briev.toml` → `folio.toml` (code, SPEC, architecture, examples/bad).
2. Manifest git dep + `src/packages.rs` (resolve path/git, lock) + tests.
3. Wire into build + check + library.
4. CLI (`update`/`add`/`remove`).
5. Install story (installer, `init`, `--version`, docs, smoke).
