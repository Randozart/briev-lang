//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Regression for the publish-into-incoming-parameter leak (BUGS.md 2026-11-09).
//!
//! Before the fix, storing an allocation into a callee's incoming object
//! parameter disabled region reclamation, so the storage leaked. The reference
//! `reclamation_runtime::native_aliases_...` guard covers the aliasing shape;
//! this covers the direct leak shape with Valgrind.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::process::Command;

#[test]
fn published_storage_into_a_local_receiver_is_reclaimed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .env("BRIEV_HOME", root)
        .current_dir(root)
        .arg("build")
        .arg(root.join("tests/fixtures/memory/publish_into_parameter_leak.bv"))
        .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let binary = directory.path().join("publish_into_parameter_leak");
    let run = Command::new("timeout").arg("30").arg(&binary).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    if Command::new("valgrind").arg("--version").output().is_ok() {
        let checked = Command::new("valgrind")
            .args(["--leak-check=full", "--errors-for-leak-kinds=definite,indirect", "--error-exitcode=91"])
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            checked.status.success(),
            "published storage into a local receiver must be reclaimed:\n{}",
            String::from_utf8_lossy(&checked.stderr)
        );
    }
}
