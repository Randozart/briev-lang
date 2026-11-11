// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! P6 (2026-11-11): reduction harness predicates.

use std::path::Path;
use std::process::Command;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn predicate(pred: &str, file: &Path) -> i32 {
    Command::new("bash")
        .arg(root().join("scripts/interesting.sh"))
        .arg(pred)
        .arg(file)
        .env("BRIEVC", env!("CARGO_BIN_EXE_brievc"))
        .current_dir(root())
        .status()
        .expect("run predicate")
        .code()
        .unwrap_or(-1)
}

#[test]
fn scripts_parse() {
    for script in ["scripts/interesting.sh", "scripts/reduce.sh"] {
        let ok = Command::new("bash")
            .args(["-n"])
            .arg(root().join(script))
            .status()
            .expect("bash -n");
        assert!(ok.success(), "{script} has a syntax error");
    }
}

#[test]
fn predicates_distinguish_crash_from_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.bv");
    std::fs::write(&bad, "defn f() -> Int { term missing; };\n").unwrap();
    assert_eq!(predicate("diagnostic", &bad), 0, "compile error is diagnostic-interesting");
    assert_eq!(predicate("ice", &bad), 1, "a normal diagnostic is not an ICE");

    let good = dir.path().join("good.bv");
    std::fs::write(
        &good,
        "let done: Bool = false;\nnode main [done == false][done] { done = true; term; };\n",
    )
    .unwrap();
    assert_eq!(predicate("ice", &good), 1);
    assert_eq!(predicate("diagnostic", &good), 1);
    assert_eq!(predicate("opt", &good), 1, "no divergence on a clean program");
}
