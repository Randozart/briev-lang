// ── Package/Dependency Resolution — the Folio (2026-10-06) ──────────────
// Phase 1.2 of docs/plans/2026-10-04-three-surfaces-functional.md: git + path
// dependencies declared in `folio.toml`, pinned in `folio.lock`, exposed to the
// import resolver as extra search roots.
//
// Model (decided 2026-10-06): Cargo-style. `foo = { git = "url", tag = "v1" }`
// or `bar = { path = "../bar" }`. Git deps clone once per URL into
// ~/.briev/cache/git/<blake3(url)>/ and are checked out at the manifest pin,
// else the locked commit, else the remote default branch. `folio.lock` records
// the resolved commit for every git dep so builds are reproducible and offline
// once the cache is warm. Path deps are never locked (they are local edits).
//
// Registry-name deps remain parsed but unsupported (clear error) — the
// registry index is a later layer.
//
// `brievc` emits LLVM IR text and shells out to git for transport (like it
// shells out to clang), so this adds no Rust dependencies.

use crate::manifest::{Dependency, GitDependency, Manifest};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PackageError {
    #[error("dependency '{0}': path '{1}' does not exist")]
    MissingPath(String, PathBuf),
    #[error("dependency '{0}': registry deps are not supported yet — use `path` or `git`")]
    RegistryUnsupported(String),
    #[error("dependency '{0}': `git` was not found on PATH (required to fetch '{1}')")]
    GitMissing(String, String),
    #[error("dependency '{0}': git failed for '{1}': {2}")]
    GitFailed(String, String, String),
    #[error("cannot read folio.toml: {0}")]
    Manifest(String),
    #[error("cannot write folio.lock: {0}")]
    Lock(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Whether resolution may mutate the filesystem. `Build` writes `folio.lock`;
/// `Check` is strictly read-only; `Update` ignores the locked commit (refreshes
/// unpinned deps to the remote head) and writes `folio.lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveMode {
    Build,
    Check,
    Update,
}

/// A dependency resolved to a local directory.
#[derive(Debug, Clone)]
pub struct ResolvedDep {
    pub name: String,
    /// Git URL or path, as written in the manifest.
    pub source: String,
    pub root: PathBuf,
    /// Resolved commit SHA for git deps; `None` for path deps.
    pub rev: Option<String>,
}

impl ResolvedDep {
    /// The import search root: `<root>/src` when it exists (the conventional
    /// layout — `src/<name>.bv`), else `<root>`.
    pub fn search_root(&self) -> PathBuf {
        let src = self.root.join("src");
        if src.is_dir() {
            src
        } else {
            self.root.clone()
        }
    }
}

/// One `folio.lock` entry — git deps only.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LockEntry {
    pub source: String,
    pub rev: String,
    /// Integrity hash of the lock record (`blake3(source + rev)`), following
    /// the macro-lock hashing pattern.
    pub hash: String,
}

/// The whole `folio.lock`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FolioLock {
    pub version: u32,
    #[serde(default)]
    pub package: BTreeMap<String, LockEntry>,
}

impl Default for FolioLock {
    fn default() -> Self {
        Self { version: 1, package: BTreeMap::new() }
    }
}

impl FolioLock {
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        toml::from_str(&text).ok()
    }

    fn entry_hash(source: &str, rev: &str) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(source.as_bytes());
        hasher.update(b"\n");
        hasher.update(rev.as_bytes());
        hasher.finalize().to_hex().to_string()
    }
}

/// `~/.briev/cache/git` — one checkout dir per URL.
pub fn cache_root() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".briev").join("cache").join("git")
}

fn checkout_dir(url: &str) -> PathBuf {
    let key = blake3::hash(url.as_bytes()).to_hex().to_string();
    cache_root().join(key)
}

/// Resolve every dependency of `manifest`. With `ResolveMode::Build`, rewrite
/// `folio.lock` (project_root/folio.lock) when the git pins changed.
pub fn resolve_dependencies(
    manifest: &Manifest,
    project_root: &Path,
    mode: ResolveMode,
) -> Result<Vec<ResolvedDep>, PackageError> {
    let lock_path = project_root.join("folio.lock");
    let lock = FolioLock::load(&lock_path).unwrap_or_default();

    let mut names: Vec<&String> = manifest.dependencies.keys().collect();
    names.sort();
    let mut deps = Vec::with_capacity(names.len());
    for name in names {
        let dep = &manifest.dependencies[name];
        deps.push(resolve_one(name, dep, project_root, lock.package.get(name), mode)?);
    }

    if mode != ResolveMode::Check {
        write_lock(&lock_path, &deps)?;
    }
    Ok(deps)
}

fn resolve_one(
    name: &str,
    dep: &Dependency,
    project_root: &Path,
    locked: Option<&LockEntry>,
    mode: ResolveMode,
) -> Result<ResolvedDep, PackageError> {
    match dep {
        Dependency::Path(p) => {
            let root = project_root.join(&p.path);
            if !root.exists() {
                return Err(PackageError::MissingPath(name.to_string(), root));
            }
            Ok(ResolvedDep {
                name: name.to_string(),
                source: p.path.display().to_string(),
                root,
                rev: None,
            })
        }
        Dependency::Registry(_) => Err(PackageError::RegistryUnsupported(name.to_string())),
        Dependency::Git(g) => resolve_git(name, g, locked, mode),
    }
}

fn resolve_git(
    name: &str,
    g: &GitDependency,
    locked: Option<&LockEntry>,
    mode: ResolveMode,
) -> Result<ResolvedDep, PackageError> {
    // Manifest pin wins; else the locked commit (reproducible) unless this is
    // an explicit update; else the remote default branch.
    let pin = g.pin().map(str::to_string).or_else(|| match mode {
        ResolveMode::Update => None,
        _ => locked.map(|l| l.rev.clone()),
    });
    let dir = checkout_dir(&g.git);
    ensure_checkout(name, &g.git, pin.as_deref(), &dir)?;
    let rev = git_head(name, &g.git, &dir)?;
    Ok(ResolvedDep {
        name: name.to_string(),
        source: g.git.clone(),
        root: dir,
        rev: Some(rev),
    })
}

fn ensure_checkout(
    name: &str,
    url: &str,
    pin: Option<&str>,
    dir: &Path,
) -> Result<(), PackageError> {
    if !dir.join(".git").exists() {
        std::fs::create_dir_all(dir)?;
        run_git(name, url, &["clone", "--quiet", url, &dir.to_string_lossy()])?;
    }
    if let Some(pin) = pin {
        run_git(name, url, &["-C", &dir.to_string_lossy(), "checkout", "--quiet", pin])?;
    }
    Ok(())
}

fn git_head(name: &str, url: &str, dir: &Path) -> Result<String, PackageError> {
    run_git(name, url, &["-C", &dir.to_string_lossy(), "rev-parse", "HEAD"])
}

fn run_git(name: &str, url: &str, args: &[&str]) -> Result<String, PackageError> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|_| PackageError::GitMissing(name.to_string(), url.to_string()))?;
    if !output.status.success() {
        return Err(PackageError::GitFailed(
            name.to_string(),
            url.to_string(),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write_lock(path: &Path, deps: &[ResolvedDep]) -> Result<(), PackageError> {
    let mut lock = FolioLock::default();
    for dep in deps {
        let Some(rev) = &dep.rev else { continue };
        lock.package.insert(
            dep.name.clone(),
            LockEntry {
                source: dep.source.clone(),
                rev: rev.clone(),
                hash: FolioLock::entry_hash(&dep.source, rev),
            },
        );
    }
    // Path-only projects write no lock (path deps are local edits, never
    // pinned — Cargo's rule).
    if lock.package.is_empty() {
        return Ok(());
    }
    let text = toml::to_string_pretty(&lock).map_err(|e| PackageError::Lock(e.to_string()))?;
    std::fs::write(path, text)?;
    Ok(())
}

/// Resolve the project's dependencies for `file_path` and return the import
/// search roots. Empty when there is no `folio.toml` or no dependencies.
pub fn project_dependency_roots(
    file_path: &str,
    mode: ResolveMode,
) -> Result<Vec<PathBuf>, PackageError> {
    let start = Path::new(file_path).parent().unwrap_or_else(|| Path::new("."));
    let Some(manifest_path) = crate::manifest::find_manifest(start) else {
        return Ok(Vec::new());
    };
    let manifest = Manifest::load(&manifest_path).map_err(|e| PackageError::Manifest(e.to_string()))?;
    if manifest.dependencies.is_empty() {
        return Ok(Vec::new());
    }
    let project_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let deps = resolve_dependencies(&manifest, project_root, mode)?;
    // The import resolver joins each search path onto the source dir, so a
    // RELATIVE path would be appended (`src/tmp_folio/...`). Return absolute
    // roots — canonicalized so `../bar` collapses.
    Ok(deps
        .iter()
        .map(|d| {
            let root = d.search_root();
            std::fs::canonicalize(&root).unwrap_or(root)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, deps: &str) {
        let text = format!(
            "[project]\nname = \"t\"\nversion = \"0.1.0\"\nentry = \"src/main.bv\"\n\n[dependencies]\n{deps}"
        );
        std::fs::write(dir.join("folio.toml"), text).unwrap();
    }

    fn init_git_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let run = |args: &[&str]| {
            let st = Command::new("git").args(args).current_dir(dir).output().unwrap();
            assert!(st.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&st.stderr));
        };
        run(&["init", "--quiet"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("foo.bv"), "defn foo() -> Int { term 1; };\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "--quiet", "-m", "init"]);
    }

    #[test]
    fn path_dependency_resolves_to_its_src() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path();
        std::fs::create_dir_all(proj.join("deps/bar/src")).unwrap();
        std::fs::write(proj.join("deps/bar/src/bar.bv"), "defn bar() -> Int { term 2; };\n").unwrap();
        write_manifest(proj, "bar = { path = \"deps/bar\" }\n");
        let manifest = Manifest::load(&proj.join("folio.toml")).unwrap();
        let deps = resolve_dependencies(&manifest, proj, ResolveMode::Build).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].search_root(), proj.join("deps/bar/src"));
        // Path deps are not locked.
        assert!(!proj.join("folio.lock").exists(), "path-only projects write no lock");
    }

    #[test]
    fn git_dependency_clones_and_locks_its_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("upstream");
        init_git_repo(&repo);
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let url = format!("file://{}", repo.display());
        write_manifest(&proj, &format!("foo = {{ git = \"{url}\" }}\n"));

        let manifest = Manifest::load(&proj.join("folio.toml")).unwrap();
        let deps = resolve_dependencies(&manifest, &proj, ResolveMode::Build).unwrap();
        assert_eq!(deps.len(), 1);
        let rev = deps[0].rev.clone().expect("git dep records a commit");

        let lock = FolioLock::load(&proj.join("folio.lock")).expect("lock written");
        assert_eq!(lock.package["foo"].rev, rev);
        assert!(!lock.package["foo"].hash.is_empty());

        // A second resolve reuses the checkout and the locked commit (offline).
        let deps2 = resolve_dependencies(&manifest, &proj, ResolveMode::Build).unwrap();
        assert_eq!(deps2[0].rev.as_deref(), Some(rev.as_str()));
    }

    #[test]
    fn check_mode_does_not_write_a_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("upstream");
        init_git_repo(&repo);
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let url = format!("file://{}", repo.display());
        write_manifest(&proj, &format!("foo = {{ git = \"{url}\" }}\n"));
        let manifest = Manifest::load(&proj.join("folio.toml")).unwrap();
        resolve_dependencies(&manifest, &proj, ResolveMode::Check).unwrap();
        assert!(!proj.join("folio.lock").exists(), "check must not mutate");
    }

    #[test]
    fn registry_dependency_is_a_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path();
        write_manifest(proj, "baz = { registry = \"baz\", version = \"0.1\" }\n");
        let manifest = Manifest::load(&proj.join("folio.toml")).unwrap();
        let err = resolve_dependencies(&manifest, proj, ResolveMode::Check).unwrap_err();
        assert!(format!("{err}").contains("registry deps are not supported"), "{err}");
    }

    /// 2026-10-06: a path dependency's module is importable by name
    /// (`import <bar>;` → `<dep>/src/bar.bv`). Built inside the repo so stdlib
    /// discovery works from the temp project.
    #[test]
    fn dependency_module_is_importable_by_name() {
        let tmp = tempfile::Builder::new()
            .prefix(".folio-dep-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let proj = tmp.path().join("proj");
        let bar = tmp.path().join("bar");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::create_dir_all(bar.join("src")).unwrap();
        std::fs::write(bar.join("src/bar.bv"), "defn bar_value() -> Int { term 42; };\n").unwrap();
        write_manifest(&proj, "bar = { path = \"../bar\" }\n");
        let src = "import <bar>;\nnode entry [beginprogram][true] { let x: Int = bar_value(); term; };\n";
        std::fs::write(proj.join("src/main.bv"), src).unwrap();
        crate::pipeline::check_source_for(proj.join("src/main.bv").to_str().unwrap(), src, None)
            .expect("the dependency module must resolve");
    }
}
