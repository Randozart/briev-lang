//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// 2026-11-16 (S3/G4'): the parity gate that makes "the protocol surface is data"
// provable rather than argued.
//
// Compiles one fixture twice. Both variants import a protocol file at the *same*
// import position and depth; the only difference is whether that file was written
// by hand or produced by `brievc render`. Byte-identical IR then means the
// generator's output is the same program as the hand-written one.
//
// Why structure matters: an inline declaration and an imported one are different
// programs to the emitter — `collect_strings` interns literals in item order, and
// splitting a declaration across a module boundary legitimately renumbers them.
// Demanding byte-identity across *that* difference would ask the emitter to be
// invariant under program restructuring, which it neither owes nor should.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Build one committed fixture to LLVM IR and return the module text.
///
/// The fixtures are committed rather than generated into the temp dir because
/// import resolution is project-root relative: a source under `target/tmp/…` has
/// no `lib/` or `tests/` to resolve against.
fn emit_ir(fixture: &str, dir: &Path) -> String {
    let out = dir.join(format!("out_{fixture}"));
    let _ = std::fs::remove_dir_all(&out);
    let output = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args([
            "build",
            &format!("tests/fixtures/glue_gen/{fixture}.bv"),
            "--llvm",
            "--out",
            &out.to_string_lossy(),
        ])
        .output()
        .expect("spawn brievc");
    assert!(
        output.status.success(),
        "build failed for {fixture}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ll = out.join(format!("{fixture}.ll"));
    std::fs::read_to_string(&ll).expect("read .ll")
}

#[test]
fn generated_and_handwritten_protocols_emit_identical_ir() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("glue_gen_parity");
    let generated = emit_ir("parity_generated", &dir);
    let handwritten = emit_ir("parity_handwritten", &dir);
    assert!(
        generated.contains("call i64 @str_to_c"),
        "the fixture must exercise the zero-copy lane, or the gate proves nothing"
    );
    if generated != handwritten {
        let first = generated
            .lines()
            .zip(handwritten.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| format!("line {}:\n  generated: {a}\n  handwritten: {b}", i + 1))
            .unwrap_or_else(|| "(length differs)".to_string());
        panic!("generated and hand-written protocols must emit identical IR; first difference {first}");
    }
}
