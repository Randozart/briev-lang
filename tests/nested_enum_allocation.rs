//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Nested enum images in collections must contain every payload word.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

fn checked(command: &mut Command) {
    let output = command.output().expect("start nested enum regression");
    assert!(output.status.success(), "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
fn nested_enum_collection_images_are_in_bounds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let has_valgrind = Command::new("valgrind").arg("--version")
        .output().is_ok_and(|output| output.status.success());
    for hir in [false, true] {
        check_emission(root, hir, has_valgrind);
    }
}

fn check_emission(root: &Path, hir: bool, has_valgrind: bool) {
    let directory = tempfile::tempdir().unwrap();
    checked(Command::new(env!("CARGO_BIN_EXE_brievc"))
        .current_dir(root).env("BRIEV_HOME", root)
        .env("BRIEV_HIR_EMIT", if hir { "1" } else { "0" })
        .args(["build", "tests/fixtures/nested_enum_allocation.bv",
            "--stdlib-path", "lib", "--config-dir", "config", "--out"])
        .arg(directory.path()));
    for level in ["-O0", "-O3"] {
        let binary = directory.path().join(format!("probe{level}"));
        checked(Command::new("clang").arg(level)
            .arg(directory.path().join("nested_enum_allocation.ll"))
            .arg(root.join("lib/runtime/briev_rt.c"))
            .args(["-lm", "-ldl", "-o"]).arg(&binary));
        checked(Command::new("timeout").arg("20").arg(&binary).env("VALUE", "42"));
        if has_valgrind {
            checked(Command::new("timeout").args(["90", "valgrind",
                "--error-exitcode=91", "--leak-check=no"])
                .arg(&binary).env("VALUE", "42"));
        }
    }
}
