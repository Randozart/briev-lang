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

    let build = Command::new(Path::new(root).join("target/release/brievc"))
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap()])
        .env("BOUND", "1000")
        .output()
        .expect("run brievc");
    assert!(build.status.success(), "build failed:\n{}", String::from_utf8_lossy(&build.stderr));

    let binary = out.join("enum_list_lifetime");
    let run = Command::new(&binary).env("BOUND", "1000").output().expect("run fixture");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.success(), "fixture trapped:\n{stdout}");
    assert!(stdout.contains("enum_list_lifetime=true"), "wrong element contents: {stdout}");
}
