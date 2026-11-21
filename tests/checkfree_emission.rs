// checkfree is a pure static obligation — INV-CF-NO-EMISSION-DELTA.
//
// The modifier lives in `Definition.modifiers` and is consumed ONLY by the
// typechecker's `check_checkfree_defns`. It must never influence codegen: the
// same program with and without the annotation has to produce byte-identical
// LLVM IR, or the language surface has smuggled a semantic difference into a
// promise that is supposed to be free.
//
// This is also the guard against the tempting shortcut of lowering `checkfree`
// into a runtime bounds check "just in case" — that would make the annotation
// a pessimization and would hide the fact that the proof failed.

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Compile `src` to LLVM IR and return the emitted `.ll` text.
fn emit_ir(name: &str, src: &str) -> String {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT)
        .join("target/briev_checkfree_emission")
        .join(name);
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();
    let bv = out_dir.join(format!("{name}.bv"));
    std::fs::write(&bv, src).unwrap();
    let build = Command::new(brievc)
        .args([
            "build",
            &bv.to_string_lossy(),
            "--llvm",
            "--out",
            &out_dir.to_string_lossy(),
            "--stdlib-path",
            &format!("{PROJECT_ROOT}/lib"),
        ])
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to spawn brievc build: {e}"));
    assert!(
        build.status.success(),
        "{name}: build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    std::fs::read_to_string(out_dir.join(format!("{name}.ll")))
        .unwrap_or_else(|e| panic!("{name}: no emitted .ll: {e}"))
}

/// The same program, differing only by the `checkfree` prefix.
fn program(prefix: &str) -> String {
    format!(
        r#"import "std/io.bv";

{prefix}defn at(xs: List<Int>, i: Int) -> Int {{
    when 0 <= i {{
        when i < xs.Count#() {{
            term xs[i];
        }};
    }};
    term -1;
}};

node entry [beginprogram][true] {{
    let xs: List<Int> = [7, 8, 9];
    Print#(at(xs, 1) as String);
    endprogram Print#("");
}};
"#
    )
}

#[test]
fn checkfree_does_not_change_emission() {
    let plain = emit_ir("cf_plain", &program(""));
    let annotated = emit_ir("cf_annotated", &program("checkfree "));
    assert_eq!(
        plain, annotated,
        "`checkfree` changed the emitted LLVM IR — the obligation is analysis-only \
         and must have no codegen effect"
    );
}
