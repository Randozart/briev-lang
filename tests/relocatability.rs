// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// Acceptance test for compiler relocatability (2026-11-12, C2c).
//
// A `brievc` shipped outside its build tree must compile with the resource
// tree beside it, from any working directory. The bug this guards against was
// subtle: `discover_system_plugins` preferred `./plugins` and silently skipped
// discovery when the directory was absent, so a package without it lost the
// prelude and the *type system* degraded — a `-> Int` contract started
// reporting `expected Float for term value ... found Int`. Nothing crashed;
// the compiler simply became quietly wrong.
//
// Two phases, both cheap (symlinks stand in for a shipped resource tree):
//   1. with `plugins/` present  -> compiles from an unrelated cwd;
//   2. with `plugins/` removed  -> fails loudly, and never with a type error.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Build the shipped-layout root: the compiler binary plus one symlink per
/// resource directory. Symlinks keep the test fast while still exercising the
/// real resolution path (`resource_root()` + `BRIEV_HOME`).
fn shipped_root(base: &Path) -> PathBuf {
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("create root");
    let exe = std::env::current_exe().expect("test exe path");
    let brievc_src = std::env::var_os("CARGO_BIN_EXE_brievc")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // Integration tests set CARGO_BIN_EXE_<name>; fall back to the
            // debug build beside the test binary for direct `cargo test` runs.
            exe.parent()
                .and_then(|p| p.parent())
                .map(|p| p.join("brievc"))
                .unwrap_or_else(|| PathBuf::from("brievc"))
        });
    std::fs::copy(&brievc_src, root.join("brievc")).expect("copy brievc");
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    for dir in ["lib", "config", "glue", "std", "spec", "plugins"] {
        let target = repo.join(dir);
        if !target.is_dir() {
            continue;
        }
        let link = root.join(dir);
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("symlink resource dir");
    }
    root
}

/// Compile the demo contract from `cwd`, with `BRIEV_HOME` pinned to `root` and
/// the cargo-provided manifest dir removed so no dev fallback can mask a
/// missing resource.
fn compile_from(root: &Path, cwd: &Path) -> std::process::Output {
    Command::new(root.join("brievc"))
        .current_dir(cwd)
        .env("BRIEV_HOME", root)
        .env_remove("CARGO_MANIFEST_DIR")
        .args(["build", "demo.bv", "--library", "--out", "out"])
        .output()
        .expect("run brievc")
}

#[test]
fn a_shipped_compiler_finds_its_prelude_from_any_working_directory() {
    let base = std::env::temp_dir().join(format!("briev-reloc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = shipped_root(&base);
    let elsewhere = base.join("elsewhere");
    std::fs::create_dir_all(elsewhere.join("out")).expect("create cwd");
    std::fs::write(
        elsewhere.join("demo.bv"),
        "defn demo(a: Int) [a >= 0][term == a + 1] -> Int { term a + 1; };\n",
    )
    .expect("write demo");

    let out = compile_from(&root, &elsewhere);
    assert!(
        out.status.success(),
        "relocated build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        elsewhere.join("out").join("demo.briev_rt.o").exists()
            || std::fs::read_dir(elsewhere.join("out"))
                .map(|d| d.count() > 0)
                .unwrap_or(false),
        "expected build artifacts in ./out"
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_missing_prelude_fails_loudly_instead_of_degrading_the_type_system() {
    let base = std::env::temp_dir().join(format!("briev-reloc-miss-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = shipped_root(&base);
    std::fs::remove_file(root.join("plugins")).expect("drop plugins link");
    let elsewhere = base.join("elsewhere");
    std::fs::create_dir_all(elsewhere.join("out")).expect("create cwd");
    std::fs::write(
        elsewhere.join("demo.bv"),
        "defn demo(a: Int) [a >= 0][term == a + 1] -> Int { term a + 1; };\n",
    )
    .expect("write demo");

    let out = compile_from(&root, &elsewhere);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "an incomplete resource tree must not compile silently:\n{stderr}"
    );
    assert!(
        stderr.contains("no system plugin directory"),
        "expected the missing-prelude diagnostic, got:\n{stderr}"
    );
    // The regression signature: losing the prelude used to surface as a type
    // mismatch rather than an installation error.
    assert!(
        !stderr.contains("type mismatch"),
        "the failure must be an installation error, not a typing degeneration:\n{stderr}"
    );
    let _ = std::fs::remove_dir_all(&base);
}
