//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Compile and execute the focused native regression fixture.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

fn checked(command: &mut Command) -> std::process::Output {
    let output = command.output().expect("start tuple test command");
    assert!(output.status.success(), "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    output
}

#[test]
fn native_regression_executes_at_both_optimization_levels() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    checked(Command::new(env!("CARGO_BIN_EXE_brievc")).current_dir(root)
        .env("BRIEV_HOME", root).arg("build").arg("tests/fixtures/load/statement_match_fallthrough.bv")
        .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
        .arg(directory.path()));
    for level in ["-O0", "-O3"] {
        let binary = directory.path().join(format!("probe{level}"));
        checked(Command::new("clang").arg(level).arg(directory.path().join("statement_match_fallthrough.ll"))
            .arg(root.join("lib/runtime/briev_rt.c")).args(["-lm", "-ldl", "-o"]).arg(&binary));
        let output = checked(Command::new("timeout").arg("20").arg(&binary));
        assert_eq!(output.stdout, b"statement_match_fallthrough_ok\n");
    }
}