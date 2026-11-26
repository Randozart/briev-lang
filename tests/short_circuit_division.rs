//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Short-circuit division/modulo guards (`e != 0 && s / e > 0`).
//!
//! 2026-11-26 (BUGS.md 2026-11-09): the `&&`/`||` register fast path evaluated
//! its right operand eagerly, so `e != 0 && s / e > 0` emitted `sdiv` before
//! the branch. With `e == 0` that is a division the left operand was meant to
//! skip — trapping/UB, and observed as corrupted output. A `Div`/`Mod` on the
//! right is not speculation-safe and must stay on the lazy CFG edge. The
//! fixture builds and runs at O0/O3 in both AST and HIR modes.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

fn checked(command: &mut Command) -> std::process::Output {
    let output = command.output().expect("start short-circuit division regression");
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
            .arg("tests/fixtures/load/short_circuit_division_guard.bv")
            .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
            .arg(directory.path()),
    );

    let ll = directory.path().join("short_circuit_division_guard.ll");
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
        assert_eq!(output.stdout, b"0 1 1 1 0 1 0 1 0 1\n");
    }
}

#[test]
fn short_circuit_division_guard_executes() {
    for hir in [false, true] {
        verify_mode(hir);
    }
}
