// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-12 (standalone roadmap C1): `--verify-report` writes a
//! machine-readable verdict, including when the gate fails.

use std::path::Path;
use std::process::Command;

fn run(src: &str) -> (i32, String) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("probe.bv");
    std::fs::write(&file, src).unwrap();
    let report = dir.path().join("report.json");
    let status = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args(["build"])
        .arg(&file)
        .args(["--llvm", "--out"])
        .arg(dir.path().join("out"))
        .args(["--verify-report"])
        .arg(&report)
        .env("BRIEV_HOME", env!("CARGO_MANIFEST_DIR"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("spawn brievc");
    assert!(report.exists(), "report must be written: {}", Path::new(&report).display());
    let json = std::fs::read_to_string(&report).unwrap();
    (status.status.code().unwrap_or(-1), json)
}

#[test]
fn report_is_written_for_a_violation_and_a_pass() {
    let (code, json) = run("defn f(a: Int) [a >= 0][term == 0] -> Int { term 5; };");
    assert_ne!(code, 0, "the gate rejects the lying body");
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    let violated = value["violated"].as_array().expect("violated array");
    assert_eq!(violated[0]["name"], "f");

    let (code, json) = run("defn f(a: Int) [a >= 0][term == a + 1] -> Int { term a + 1; };");
    assert_eq!(code, 0, "the honest body passes");
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert!(
        value["proven"].as_array().is_some_and(|names| !names.is_empty()),
        "{value}"
    );
    assert_eq!(value["violated"].as_array().map(Vec::len), Some(0));
}
