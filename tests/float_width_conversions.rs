//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Floating-width conversions across the Float16 / Float32 / Float64 family.
//!
//! 2026-11-26: codegen paths that only recognized `float`/`double` emitted
//! invalid IR for Float16 — `store half <floatreg>` on assignment, `ret half
//! <floatreg>` on return, and `fadd float %half, …` on a Float64 `+ Float`
//! literal. The conversions (fpext/fptrunc) must be inserted at the slot,
//! return, and cast boundaries. Building the fixture fails if the IR is
//! invalid; the -O0/-O3 run pins the values.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

fn checked(command: &mut Command) -> std::process::Output {
    let output = command.output().expect("start float width conversion regression");
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn verify_mode(hir: bool) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    let mut compiler = Command::new(env!("CARGO_BIN_EXE_brievc"));
    compiler.env("BRIEV_HIR_EMIT", if hir { "1" } else { "0" });
    checked(
        compiler
            .current_dir(root)
            .env("BRIEV_HOME", root)
            .arg("build")
            .arg("tests/fixtures/load/float_width_conversions.bv")
            .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
            .arg(directory.path()),
    );

    let ll = directory.path().join("float_width_conversions.ll");
    for level in ["-O0", "-O3"] {
        let binary = directory.path().join(format!("probe{level}"));
        checked(
            Command::new("clang")
                .arg(level)
                .arg(&ll)
                .arg(root.join("lib/runtime/briev_rt.c"))
                .args(["-lm", "-ldl", "-o"])
                .arg(&binary),
        );
        let output = checked(Command::new("timeout").arg("20").arg(&binary));
        assert_eq!(output.stdout, b"4.5 4 2.5\n");
    }
}

#[test]
fn float_width_conversions_execute() {
    for hir in [false, true] {
        verify_mode(hir);
    }
}
