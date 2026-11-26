//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Float state/global/const fields initialized from Int literals.
//!
//! 2026-11-26: `let f: Float = 0;` (and the Float64/Float16/negative and
//! `const` spellings) emitted an integer constant in a floating slot —
//! `store float 0`, `@d = global double 0`, `@Z = constant float 0` — which
//! clang/opt reject as invalid IR (BUGS.md 2026-11-06). The literal must be
//! emitted at the slot's declared width. Building the fixture fails if the
//! IR is invalid; the -O0/-O3 run pins the runtime value.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

fn checked(command: &mut Command) -> std::process::Output {
    let output = command.output().expect("start float literal state init regression");
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// No integer constant may be stored into / declared for a floating slot.
fn assert_no_integer_float_constants(ir: &str) {
    for bad in [
        "store float 0,",
        "store double 0,",
        "store half 0,",
        "global float 0\n",
        "global double 0\n",
        "global half 0\n",
        "constant float 0\n",
        "constant double 0\n",
        "constant half 0\n",
    ] {
        assert!(
            !ir.contains(bad),
            "invalid floating constant `{bad}` emitted:\n{ir}"
        );
    }
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
            .arg("tests/fixtures/load/float_int_literal_state_init.bv")
            .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
            .arg(directory.path()),
    );

    let ll = directory.path().join("float_int_literal_state_init.ll");
    let ir = std::fs::read_to_string(&ll).expect("read emitted IR");
    assert_no_integer_float_constants(&ir);

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
        assert_eq!(output.stdout, b"1 -1\n");
    }
}

#[test]
fn float_int_literal_state_init_executes() {
    for hir in [false, true] {
        verify_mode(hir);
    }
}
