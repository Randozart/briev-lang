// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-10: emitted artifact shapes.
//! `.beast` = AST S-expression snapshots; `.ir` = REAL backend IR at
//! codegen/optimize; `.hir` = canonical HIR at Typed/Provenanced.

use std::path::{Path, PathBuf};
use std::process::Command;

fn build(src: &Path, out: &Path, extra: &[&str]) -> (bool, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brievc"));
    cmd.args(["build"])
        .arg(src)
        .args(["--llvm", "--out"])
        .arg(out)
        .args(extra)
        .env("BRIEV_HOME", env!("CARGO_MANIFEST_DIR"))
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    let output = cmd.output().expect("spawn brievc");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

fn artifact(dir: &Path, suffix: &str) -> PathBuf {
    let hit = std::fs::read_dir(dir)
        .expect("read artifacts")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.file_name().unwrap().to_string_lossy().ends_with(suffix))
        .unwrap_or_else(|| panic!("no *{suffix} artifact in {}", dir.display()));
    hit
}

#[test]
fn ast_ir_and_hir_artifacts_have_their_own_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("artifacts.bv");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal.bv"),
        &src,
    )
    .unwrap();
    let out = dir.path().join("out");
    let (ok, log) = build(
        &src,
        &out,
        &["--emit-beast", "parse", "--emit-beast", "codegen", "--emit-hir"],
    );
    assert!(ok, "build failed:\n{log}");

    let beast = std::fs::read_to_string(artifact(dir.path(), ".beast")).unwrap();
    assert!(beast.trim_start().starts_with('('), "AST snapshot is S-expression text");
    assert!(!beast.contains("target triple"), "AST snapshot must not carry IR");

    let ir = std::fs::read_to_string(artifact(dir.path(), ".ir")).unwrap();
    assert!(ir.contains("target triple"), "IR snapshot is the backend text:\n{ir}");
    assert!(ir.contains("define"), "IR snapshot has function definitions");
    assert!(!ir.contains("(universe"), "IR snapshot must not be AST S-expressions");

    let hir = std::fs::read_to_string(artifact(dir.path(), ".hir")).unwrap();
    assert!(hir.starts_with("program"), "HIR snapshot starts with the program header");
    assert!(hir.contains("callable "), "HIR snapshot lists callables");
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "hir"))
            .count(),
        2,
        "Typed and Provenanced HIR snapshots"
    );
}

#[test]
fn emit_beast_ir_stage_requires_the_backend_text() {
    // Sanity: the IR snapshot appears only at codegen/optimize, not for AST
    // stages. Requesting only `parse` must not write `.ir`.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("astonly.bv");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal.bv"),
        &src,
    )
    .unwrap();
    let (ok, log) = build(&src, &dir.path().join("out"), &["--emit-beast", "parse"]);
    assert!(ok, "build failed:\n{log}");
    assert!(
        !std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.path().extension().is_some_and(|x| x == "ir")),
        "AST-only snapshot must not emit .ir"
    );
}
