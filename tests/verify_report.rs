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
    assert!(
        report.exists(),
        "report must be written: {}",
        Path::new(&report).display()
    );
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
        value["proven"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry["name"] == "f")),
        "{value}"
    );
    assert_eq!(value["violated"].as_array().map(Vec::len), Some(0));
}

#[test]
fn saved_values_and_current_guard_facts_cannot_authorize_false_proofs() {
    for source in [
        "defn f(x: Int64) [true][term == 0] -> Int64 { let y: Int64 = x; x = 0; term y; };",
        "defn f(x: Int64, y: Int64) [x == 1][term == y] -> Int64 { x = 2; when x == 2 { term x; }; term y; };",
        "defn helper(x: Int64) -> Int64 { let y: Int64 = x; x = 0; term y; }; defn f(x: Int64) [true][term == 0] -> Int64 { term helper(x); };",
    ] {
        let (code, report) = run(source);
        let value: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            code, 0,
            "input-dependent counterexamples remain advisory: {report}"
        );
        assert!(
            value["proven"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["name"] != "f"),
            "{report}"
        );
        assert!(
            value["unproven"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["name"] == "f"),
            "{report}"
        );
    }
}

#[test]
fn endpoint_guards_are_safe_and_reversed_false_contracts_are_rejected() {
    let (code, report) = run(
        "defn f(x: Int) [true][term == 0] -> Int { when x > 9223372036854775807 { term 5; }; term 0; };",
    );
    let value: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(code, 0, "{report}");
    assert!(
        value["proven"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "f"),
        "{report}"
    );
    let (code, report) = run("defn f(x: Int) [0 < x][term < 0] -> Int { term x; };");
    let value: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(code, 1, "{report}");
    assert!(
        value["violated"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "f"),
        "{report}"
    );
}
