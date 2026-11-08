//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! P0 regression: a folded counted node must keep its nested `foreach`
//! (`emit_countable_body` used to drop it via `_ => {}`).
//!
//! Plan: `docs/plans/2026-11-08-general-workstream-countable-body-and-folds.md`.
//! The fixture's only `Max#` use is inside the counted node's nested loop,
//! so `llvm.smax.i64` in the emitted IR can ONLY exist if the loop survived
//! the countable-body emission. (Before the fix the fold emitted
//! `y[i] = a[i*K]` and the call vanished.)

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn folded_counted_node_keeps_nested_foreach() {
    let root = Path::new(PROJECT_ROOT);
    let fixture = root.join("tests/fixtures/load/countable_body_foreach.bv");
    assert!(fixture.exists(), "missing fixture {}", fixture.display());

    let out_dir = root.join("target/countable_body_foreach");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).expect("create out dir");

    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .env("BRIEV_HOME", root)
        .current_dir(root)
        .args(["build", &fixture.to_string_lossy(), "--llvm"])
        .args(["--stdlib-path", &root.join("lib").to_string_lossy()])
        .args(["--config-dir", &root.join("config").to_string_lossy()])
        .args(["--out", &out_dir.to_string_lossy()])
        .output()
        .expect("spawn brievc build");
    assert!(
        build.status.success(),
        "fixture build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ir = std::fs::read_to_string(out_dir.join("countable_body_foreach.ll"))
        .expect("read emitted IR");

    // The nested max fold survived (the P0 bug dropped it).
    assert!(
        ir.contains("llvm.smax.i64"),
        "nested foreach was dropped from the folded counted node — \
         `emit_countable_body` lost the loop again"
    );
    // The folded body still carries the loop label family emitted by the
    // standard statement emitter.
    let loop_labels = ir.matches("foreach.hdr").count();
    assert!(
        loop_labels >= 2,
        "expected the init chunk loop AND the fold loop; found {loop_labels} \
         foreach headers"
    );
}
