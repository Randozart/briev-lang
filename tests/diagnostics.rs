// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! P2: compile-fail corpus + directive runner. Each `.bv` under
//! `tests/compile_fail/` declares the substrings its diagnostic must contain;
//! `tests/directives/` proves the inline stdout/exit directives.

use std::path::Path;
use std::process::Command;

fn run_suite(dir: &str, out: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(out);
    let _ = std::fs::remove_dir_all(&out_dir);
    let output = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args([
            "test",
            &root.join(dir).to_string_lossy(),
            "--out",
            &out_dir.to_string_lossy(),
        ])
        .output()
        .expect("spawn brievc test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{dir} failed:\n{stdout}\n{stderr}"
    );
    assert!(
        !stdout.contains("FAIL "),
        "{dir} reported a failing case:\n{stdout}"
    );
}

#[test]
fn compile_fail_corpus_reports_expected_diagnostics() {
    run_suite("tests/compile_fail", "diagnostics_compile_fail");
}

#[test]
fn directive_corpus_runs_with_inline_expectations() {
    run_suite("tests/directives", "diagnostics_directives");
}
