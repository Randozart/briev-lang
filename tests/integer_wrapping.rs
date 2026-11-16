// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Wrapping values and comparisons must survive native optimization.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::{
    path::Path,
    process::{Command, Output},
};

fn checked(command: &mut Command) -> Output {
    let output = command.output().expect("start integer wrapping regression");
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn native_integer_arithmetic_wraps_at_o0_and_o3() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for hir in ["0", "1"] {
        let directory = tempfile::tempdir().unwrap();
        checked(
            Command::new(env!("CARGO_BIN_EXE_brievc"))
                .current_dir(root)
                .env("BRIEV_HOME", root)
                .env("BRIEV_HIR_EMIT", hir)
                .args([
                    "build",
                    "tests/fixtures/integer_wrapping.bv",
                    "--llvm",
                    "--stdlib-path",
                    "lib",
                    "--config-dir",
                    "config",
                    "--out",
                ])
                .arg(directory.path()),
        );
        for level in ["-O0", "-O3"] {
            let binary = directory.path().join(format!("wrapping{level}"));
            checked(
                Command::new("clang")
                    .arg(level)
                    .arg(directory.path().join("integer_wrapping.ll"))
                    .arg(root.join("lib/runtime/briev_rt.c"))
                    .args(["-lm", "-ldl", "-o"])
                    .arg(&binary),
            );
            check_cases(&binary, hir, level);
        }
    }
}

fn check_cases(binary: &Path, hir: &str, level: &str) {
    for (a, b) in [
        (0_i64, 0_i64),
        (7, -3),
        (i64::MAX, 1),
        (i64::MIN, -1),
        (i64::MAX, 2),
        (i64::MIN, 1),
        (-1, i64::MAX),
    ] {
        let output = checked(
            Command::new("timeout")
                .args(["--kill-after=2s", "10s"])
                .arg(binary)
                .env("WRAP_A", a.to_string())
                .env("WRAP_B", b.to_string()),
        );
        let expected = format!(
            "{}|{}|{}|{}|{}|{}|{}\n",
            a.wrapping_add(b),
            a.wrapping_sub(b),
            a.wrapping_mul(b),
            a.wrapping_neg(),
            i32::from(a.wrapping_add(1) > a),
            i32::from(a.wrapping_sub(1) < a),
            i32::from(a.wrapping_mul(2) > a)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected,
            "HIR={hir}, {level}, a={a}, b={b}"
        );
    }
}
