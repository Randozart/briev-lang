// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-17 (issue #5): `brievc check --validate` runs the backend
//! normalizer + codegen + `opt -passes=verify` without linking, so a program
//! whose frontend typechecks but whose emitted IR is invalid is caught by
//! `check`, not just `build`.
//!
//! These tests drive the real `brievc` binary end-to-end:
//!
//!   * `brievc check <file>` (default) is frontend-only and cheap — it never
//!     runs the backend normalizer or codegen. The conformance sweep depends
//!     on that.
//!   * `brievc check --validate <file>` runs the full pipeline (frontend,
//!     normalizer, codegen) and validates the emitted `.ll` with
//!     `opt -passes=verify`. On a good program it exits 0 and prints
//!     "emitted IR verified".
//!
//! The `--validate` seam is the fix for the divergence where the frontend
//! accepts a program whose emitted IR fails LLVM verification (missing sret
//! slot, width mismatch, undefined SSA) — `check` returned `OK` because it
//! never generated the IR, while `build` died in `opt`/`clang`.

use std::process::Command;

const GOOD_PROGRAM: &str = r#"
let done: Bool = false;
node main [!done][done] {
    done = true;
    endprogram;
};
"#;

/// Write the program to a temp `.bv` file and return its path.
fn write_tmp(src: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("checkval_{}_.bv", std::process::id()));
    std::fs::write(&p, src).expect("write temp .bv");
    p
}

/// Run `brievc <args...>` and capture stdout+stderr.
fn run_brievc(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args(args)
        .output()
        .expect("run brievc");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap_or(-1), combined)
}

/// 2026-11-17: default `check` is frontend-only — it typechecks and prints
/// `OK` without running the backend normalizer or codegen. No `.ll` is
/// written (the default check path returns at `parse_and_check`).
#[test]
fn check_default_is_frontend_only() {
    let path = write_tmp(GOOD_PROGRAM);
    let rel = path.to_str().unwrap();
    let (code, out) = run_brievc(&["check", rel]);
    assert_eq!(code, 0, "default check must pass on a good program:\n{out}");
    assert!(out.contains("OK"), "default check must print OK:\n{out}");
    // The frontend-only path never writes a `.ll` file.
    let ll = path.with_extension("ll");
    assert!(!ll.exists(), "default check must not emit .ll:\n{out}");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&ll);
}

/// 2026-11-17: `check --validate` runs the full pipeline (frontend +
/// normalizer + codegen) and validates the emitted `.ll` with
/// `opt -passes=verify`. On a good program it exits 0, prints
/// "emitted IR verified", and the `.ll` is on disk (proof the codegen ran).
#[test]
fn check_validate_runs_codegen_and_verifies() {
    let path = write_tmp(GOOD_PROGRAM);
    let rel = path.to_str().unwrap();
    let (code, out) = run_brievc(&["check", "--validate", rel]);
    assert_eq!(
        code, 0,
        "check --validate must pass on a good program:\n{out}"
    );
    assert!(
        out.contains("emitted IR verified"),
        "check --validate must report IR verification:\n{out}"
    );
    // The validate path writes the `.ll` before validating it.
    let ll = path.with_extension("ll");
    assert!(ll.exists(), "check --validate must emit the .ll:\n{out}");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&ll);
}
