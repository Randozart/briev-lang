// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-19 (issue #132): the generated glue protocols file must be
//! self-contained. Its axioms name `str_to_c`/`cstr_to_briev`, which live in
//! the descriptor's `types_module` (`glue/c/types.bv`); `brievc render c` now
//! emits `import "glue/c/types.bv";` into `protocols.bv`, so a program importing
//! ONLY `glue/c/protocols.bv` and crossing the String boundary builds. Before
//! the fix, `--llvm` rejected the module with
//! "use of undefined value '@str_to_c'".
//!
//! This is the end-to-end half of the fix: `glue::generator` unit tests pin that
//! the import line is emitted; this test pins that the emitted import actually
//! resolves the ops for a real consumer (no `glue/c.bv`, no fixture hand-edit).

use std::process::Command;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn importing_only_the_generated_protocols_resolves_the_lane_ops() {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_protocols_only_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let fixture = ROOT.to_string() + "/tests/fixtures/glue_gen/protocols_only_boundary.bv";
    let build = Command::new(brievc)
        .args([
            "build",
            &fixture,
            "--llvm",
            "--out",
            &out_dir.to_string_lossy(),
        ])
        .args(["--stdlib-path", "lib", "--config-dir", "config"])
        .current_dir(ROOT)
        .output()
        .expect("spawn brievc");

    assert!(
        build.status.success(),
        "protocols-only import must resolve the ops; stderr:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    // The lane fired at the call site: `str_to_c` must be defined and called.
    let ll = std::fs::read_to_string(out_dir.join("protocols_only_boundary.ll")).expect("read .ll");
    assert!(
        ll.contains("@str_to_c"),
        "the str_to_c op must be emitted, not left undefined:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
}
