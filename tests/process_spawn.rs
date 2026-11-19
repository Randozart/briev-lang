//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Execute shell status and byte-capture contracts through both engines.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

fn checked(command: &mut Command) -> Vec<u8> {
    let output = command
        .output()
        .expect("start process intrinsic regression");
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn process_intrinsics_match_reference_execution() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    let interpreted = checked(
        Command::new(env!("CARGO_BIN_EXE_brievc"))
            .current_dir(root)
            .env("BRIEV_HOME", root)
            .args([
                "test",
                "tests/fixtures/process_spawn.bv",
                "--tests-only",
                "--out",
            ])
            .arg(directory.path()),
    );
    assert!(String::from_utf8_lossy(&interpreted).contains("1/1 tests passed"));
    for hir in ["0", "1"] {
        check_native(root, hir);
    }
}

fn check_native(root: &Path, hir: &str) {
    let directory = tempfile::tempdir().unwrap();
    checked(
        Command::new(env!("CARGO_BIN_EXE_brievc"))
            .current_dir(root)
            .env("BRIEV_HOME", root)
            .env("BRIEV_HIR_EMIT", hir)
            .args([
                "build",
                "tests/fixtures/process_spawn.bv",
                "--llvm",
                "--stdlib-path",
                "lib",
                "--config-dir",
                "config",
                "--out",
            ])
            .arg(directory.path()),
    );
    let has_valgrind = Command::new("valgrind")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    for level in ["-O0", "-O3"] {
        let binary = directory.path().join(format!("probe{level}"));
        checked(
            Command::new("clang")
                .args([level, "-flto"])
                .arg(directory.path().join("process_spawn.ll"))
                .arg(root.join("lib/runtime/briev_rt.c"))
                .args(["-lm", "-ldl", "-o"])
                .arg(&binary),
        );
        assert_eq!(
            checked(Command::new("timeout").arg("20").arg(&binary)),
            b"process_spawn_ok\n"
        );
        if has_valgrind {
            checked(
                Command::new("timeout")
                    .args(["90", "valgrind", "--error-exitcode=91", "--leak-check=no"])
                    .arg(&binary),
            );
        }
    }
}

#[test]
fn capture_runtime_handles_growth_boundaries_and_legacy_callers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("capture-runtime");
    checked(
        Command::new("clang")
            .arg("-O2")
            .arg(root.join("tests/fixtures/process_capture_runtime.c"))
            .arg(root.join("lib/runtime/briev_rt.c"))
            .args(["-lm", "-ldl", "-o"])
            .arg(&binary),
    );
    checked(Command::new("timeout").arg("30").arg(&binary));
    if Command::new("valgrind")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        checked(
            Command::new("timeout")
                .args([
                    "90",
                    "valgrind",
                    "--error-exitcode=91",
                    "--leak-check=full",
                    "--errors-for-leak-kinds=definite",
                ])
                .arg(&binary),
        );
    }
}
