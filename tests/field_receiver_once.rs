//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Compile and execute the focused native regression fixture.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

fn checked(command: &mut Command) -> std::process::Output {
    let output = command.output().expect("start field receiver test command");
    assert!(output.status.success(), "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    output
}

fn verify_emission_mode(hir: bool) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    let mut compiler = Command::new(env!("CARGO_BIN_EXE_brievc"));
    compiler.env("BRIEV_HIR_EMIT", if hir { "1" } else { "0" });
    checked(compiler.current_dir(root)
        .env("BRIEV_HOME", root)
        .arg("build").arg("tests/fixtures/load/field_receiver_once.bv")
        .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
        .arg(directory.path()));
    for level in ["-O0", "-O3"] {
        let binary = directory.path().join(format!("probe{level}"));
        checked(Command::new("clang").arg(level).arg(directory.path().join("field_receiver_once.ll"))
            .arg(root.join("lib/runtime/briev_rt.c")).args(["-lm", "-ldl", "-o"]).arg(&binary));
        let output = checked(Command::new("timeout").arg("20").arg(&binary));
        assert_eq!(output.stdout, b"field_receiver_once_ok\n");
    }
}

#[test]
fn native_regression_executes_at_both_optimization_levels() {
    for hir in [false, true] { verify_emission_mode(hir); }
}
