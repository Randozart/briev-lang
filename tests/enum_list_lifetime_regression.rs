// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Regression pin for enum elements stored in a growing list.
//!
//! Regression for the enum-value ownership fix: each pushed enum image must be
//! owned by the collection, not left pointing at a caller-owned `%esrt` stack
//! image that later constructors reuse.

use std::path::Path;
use std::process::Command;

#[test]
fn enum_list_elements_survive_later_calls() {
    let root = env!("CARGO_MANIFEST_DIR");
    let fixture = Path::new(root).join("tests/tier1/enum_list_lifetime.bv");
    let out = Path::new("/tmp/opencode/briev-enum_list_lifetime-regression");
    let _ = std::fs::remove_dir_all(out);

    // 2026-11-18: use the binary cargo just built, not a hardcoded
    // `target/release/brievc`. The nightly's AST opt-out suite builds only the
    // debug profile, so the hardcoded path did not exist there and this test
    // failed with `build failed:` + an empty stderr — which reads like a compiler
    // bug rather than a missing file. Locally it "passed" only because a stale
    // release binary from a previous session happened to be there.
    // `CARGO_BIN_EXE_brievc` is exported by cargo for integration tests, so the
    // path and the profile are always the ones under test (dir_smoke.rs and
    // wasm32_parity.rs already do it this way).
    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args([
            "build",
            fixture.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .env("BOUND", "1000")
        .output()
        .expect("run brievc");
    assert!(
        build.status.success(),
        "build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let binary = out.join("enum_list_lifetime");
    let run = Command::new(&binary)
        .env("BOUND", "1000")
        .output()
        .expect("run fixture");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.success(), "fixture trapped:\n{stdout}");
    assert!(
        stdout.contains("enum_list_lifetime=true"),
        "wrong element contents: {stdout}"
    );
}
