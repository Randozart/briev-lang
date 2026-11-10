// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-10: plugin pipeline intent.
//! AST stages run on the LIVE AST (`on_ast`); Generated/Optimized run on the
//! live backend IR TEXT (`on_ir`). No serialization in the data path.
//! Regression: `on_ir` used to be a no-op, so `$(Generated)` blocks silently
//! did nothing.

use std::path::Path;
use std::process::Command;

fn compile(src: &str) -> (bool, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plugin_e2e.bv");
    std::fs::write(&path, src).unwrap();
    let out = dir.path().join("out");
    let result = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args(["build"])
        .arg(&path)
        .args(["--llvm", "--out"])
        .arg(&out)
        .env("BRIEV_HOME", env!("CARGO_MANIFEST_DIR"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("spawn brievc");
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let ll = std::fs::read_to_string(out.join("plugin_e2e.ll")).unwrap_or_default();
    (result.status.success(), format!("{stdout}\n{stderr}"), ll)
}

const SOURCE: &str = r#"
$(Typed) {
    EmitInfo$("plugin-typed-ran");
};

$(Generated) {
    Find$("x86_64-unknown-linux-gnu\"").InsertAfter$(" ; plugin-ran");
};

let done: Bool = false;
node main [done == false][done == true] {
    println!(1);
    done = true;
    term;
};
"#;

#[test]
fn ast_and_ir_stage_plugins_reach_emission() {
    let (ok, log, ll) = compile(SOURCE);
    assert!(ok, "build failed:\n{log}");
    assert!(log.contains("plugin-typed-ran"), "live-AST stage ran:\n{log}");
    assert!(ll.contains("; plugin-ran"), "IR stage mutated the emitted text:\n{ll}");
}

#[test]
fn unsupported_ir_statement_is_a_loud_error() {
    let src = r#"
$(Generated) {
    when true { Find$("target").Delete$(); };
};
let done: Bool = false;
node main [done == false][done == true] { done = true; term; };
"#;
    let (ok, log, _) = compile(src);
    assert!(!ok, "unsupported IR plugin statement must fail the build");
    assert!(
        log.contains("IR plugin: unsupported statement"),
        "error names the IR-plugin surface:\n{log}"
    );
}
