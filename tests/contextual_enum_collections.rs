//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Declared collection context survives enum, tuple and nested-list boundaries.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

fn checked(command: &mut Command) -> Vec<u8> {
    let output = command
        .output()
        .expect("start contextual collection regression");
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn enum_collection_literals_match_the_interpreter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    let fixture = root.join("tests/fixtures/contextual_enum_collections.bv");
    let interpreted = checked(
        Command::new(env!("CARGO_BIN_EXE_brievc"))
            .current_dir(root)
            .env("BRIEV_HOME", root)
            .arg("test")
            .arg(&fixture)
            .arg("--tests-only")
            .arg("--out")
            .arg(directory.path().join("interpreter")),
    );
    assert!(String::from_utf8_lossy(&interpreted).contains("1/1 tests passed"));
    for hir in ["0", "1"] {
        check_emission(root, &fixture, hir);
    }
}

fn check_emission(root: &Path, fixture: &Path, hir: &str) {
    let directory = tempfile::tempdir().unwrap();
    checked(
        Command::new(env!("CARGO_BIN_EXE_brievc"))
            .current_dir(root)
            .env("BRIEV_HOME", root)
            .env("BRIEV_HIR_EMIT", hir)
            .arg("build")
            .arg(fixture)
            .args([
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
        .is_ok_and(|result| result.status.success());
    for level in ["-O0", "-O3"] {
        let binary = directory.path().join(format!("probe{level}"));
        checked(
            Command::new("clang")
                .args([level, "-flto"])
                .arg(directory.path().join("contextual_enum_collections.ll"))
                .arg(root.join("lib/runtime/briev_rt.c"))
                .args(["-lm", "-ldl", "-o"])
                .arg(&binary),
        );
        assert_eq!(
            checked(Command::new("timeout").arg("20").arg(&binary)),
            b"contextual_enum_collections_ok\n"
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
