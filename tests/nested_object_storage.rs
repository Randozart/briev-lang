//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Nested object handles must not overlap adjacent fields or allocation ends.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

fn checked(command: &mut Command) {
    let output = command.output().expect("start nested object regression");
    assert!(output.status.success(), "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
fn nested_object_storage_preserves_adjacent_fields() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for hir in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut compiler = Command::new(env!("CARGO_BIN_EXE_brievc"));
        compiler.env("BRIEV_HIR_EMIT", if hir { "1" } else { "0" });
        checked(compiler.current_dir(root).env("BRIEV_HOME", root)
            .arg("build").arg("tests/fixtures/nested_object_storage.bv")
            .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
            .arg(directory.path()));
        for level in ["-O0", "-O3"] {
            let binary = directory.path().join(format!("probe{level}"));
            checked(Command::new("clang").arg(level)
                .arg(directory.path().join("nested_object_storage.ll"))
                .arg(root.join("lib/runtime/briev_rt.c"))
                .args(["-lm", "-ldl", "-o"]).arg(&binary));
            checked(Command::new("timeout").arg("20").arg(&binary));
        }
    }
}
