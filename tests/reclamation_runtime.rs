//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Native region bookkeeping, including explicit release across a barrier.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{fs, process::Command};

#[test]
fn region_records_follow_explicit_free_growth_and_suspension() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("regions.ll");
    let runtime = include_str!("../src/backend/llvm/reclamation.ll").replace("define internal ", "define ");
    fs::write(&source, format!("declare ptr @malloc(i64)\ndeclare void @free(ptr)\ndeclare ptr @realloc(ptr, i64)\n{runtime}")).unwrap();
    let harness = directory.path().join("probe.c");
    fs::write(&harness, include_str!("fixtures/load/reclamation_runtime.c")).unwrap();
    for optimization in ["-O0", "-O3"] {
        let binary = directory.path().join("probe");
        let build = Command::new("clang").arg(optimization).arg("-flto")
            .arg(&source).arg(&harness).args(["-Wl,--wrap=malloc", "-Wl,--wrap=free", "-Wl,--wrap=realloc"]).arg("-o").arg(&binary).output().unwrap();
        assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
        let run = Command::new(&binary).output().unwrap();
        assert!(run.status.success(), "region allocation counts: {:?}", run.status);
        if Command::new("valgrind").arg("--version").output().is_ok() {
            let checked = Command::new("valgrind").args(["--leak-check=full", "--errors-for-leak-kinds=definite,indirect", "--error-exitcode=91"])
                .arg(&binary).output().unwrap();
            assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stderr));
        }
    }
}

#[test]
fn native_aliases_borrowed_payloads_and_nested_returns_are_memory_safe() {
    let (_directory, binary) = build_native_fixture("reclamation_aliases");
    assert_native_output(&binary, b"reclamation_ok\n");
    if Command::new("valgrind").arg("--version").output().is_ok() {
        let checked = Command::new("valgrind").args(["--leak-check=full", "--errors-for-leak-kinds=definite,indirect", "--error-exitcode=91"])
            .arg(binary).output().unwrap();
        assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stderr));
    }
}

#[test]
fn repeated_scopes_preserve_bounded_tail_execution() {
    let (_directory, binary) = build_native_fixture("reclamation_bounded");
    assert_native_output(&binary, b"bounded_regions_ok\n");
}

fn build_native_fixture(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .env("BRIEV_HOME", root).current_dir(root)
        .arg("build").arg(root.join(format!("tests/fixtures/load/{name}.bv")))
        .arg("--stdlib-path").arg(root.join("lib"))
        .arg("--config-dir").arg(root.join("config"))
        .arg("--out").arg(directory.path()).output().unwrap();
    assert!(build.status.success(), "{}\n{}", String::from_utf8_lossy(&build.stdout), String::from_utf8_lossy(&build.stderr));
    let binary = directory.path().join(name);
    (directory, binary)
}

fn assert_native_output(binary: &std::path::Path, expected: &[u8]) {
    let result = Command::new("timeout").arg("30").arg(binary).output().unwrap();
    assert!(result.status.success(), "{:?}: {}", result.status, String::from_utf8_lossy(&result.stderr));
    assert_eq!(result.stdout, expected);
}