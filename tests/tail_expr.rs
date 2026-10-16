// 2026-10-16 (tails plan §2, step 5): backend E2E for defn-body tails —
// a final bare expression (no `;`) is the body's implicit value, an
// UNCONDITIONAL return. Interpreter pins: interpreter::eval::tail_expr;
// typecheck pins: typechecker::tests::tail_*. These pin the LLVM emission
// end to end: build + link + run, harness-exact flags.

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn out_dir(name: &str) -> PathBuf {
    let d = Path::new(PROJECT_ROOT)
        .join("target/briev_tail_expr")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

use std::path::PathBuf;

fn compile(name: &str, src: &str) -> Result<PathBuf, String> {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let dir = out_dir(name);
    let bv = dir.join(format!("{name}.bv"));
    std::fs::write(&bv, src).unwrap();
    let build = Command::new(brievc)
        .args([
            "build",
            &bv.to_string_lossy(),
            "--llvm",
            "--out",
            &dir.to_string_lossy(),
            "--stdlib-path",
            &format!("{PROJECT_ROOT}/lib/std"),
        ])
        .output()
        .unwrap();
    if !build.status.success() {
        return Err(String::from_utf8_lossy(&build.stderr).to_string());
    }
    Ok(dir)
}

fn build_link_run(name: &str, src: &str) -> (String, i32) {
    let dir = compile(name, src).unwrap_or_else(|e| panic!("{name}: build failed:\n{e}"));
    let ll = dir.join(format!("{name}.ll"));
    let exe = dir.join(name);
    let link = Command::new("clang")
        .args([
            "-O3",
            "-flto",
            "-march=native",
            "-ffast-math",
            "-fdata-sections",
            "-ffunction-sections",
            "-Wl,--gc-sections",
            &ll.to_string_lossy(),
            &format!("{PROJECT_ROOT}/lib/runtime/briev_rt.c"),
            "-o",
            &exe.to_string_lossy(),
        ])
        .output()
        .unwrap();
    assert!(
        link.status.success(),
        "{name}: link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );
    let run = Command::new(&exe).output().unwrap();
    (
        String::from_utf8_lossy(&run.stdout).to_string(),
        run.status.code().unwrap_or(-1),
    )
}

#[test]
fn value_tail_returns_from_defn() {
    // The tail `x * 2` is the defn's value — no `term`, no `;`.
    let src = r#"
defn double(x: Int) -> Int {
    x * 2
};

let n: Int = 0;
node main [n == 0][n == 1] {
    Print#(double(21));
    n = 1;
    endprogram;
};
"#;
    let (out, code) = build_link_run("value_tail", src);
    assert_eq!(code, 0, "exit code");
    assert_eq!(out, "42", "the tail's value reaches the call site");
}

#[test]
fn match_tail_returns_arm_value() {
    // The json-shaped body — a final match arms the return.
    let src = r#"
defn pick(x: Int) -> Int {
    match x { 0 => 7, _ => 9 }
};

let n: Int = 0;
node main [n == 0][n == 1] {
    Print#(pick(0));
    Print#(pick(5));
    n = 1;
    endprogram;
};
"#;
    let (out, code) = build_link_run("match_tail", src);
    assert_eq!(code, 0, "exit code");
    assert_eq!(out, "79", "each arm's value is the defn's value");
}

#[test]
fn value_tail_in_void_defn_is_a_build_error() {
    // Typecheck: a VALUE tail in a `-> Void` body is rejected — the fix is
    // an explicit `term` (or a value output type).
    let src = r#"
defn f(x: Int) -> Void {
    x * 2
};
node main [beginprogram][true] {
    f(2);
    endprogram;
};
"#;
    let err = compile("void_value_tail", src)
        .expect_err("value tail in a Void body must fail the build");
    assert!(
        err.contains("tail value vs declared return type")
            || err.contains("TypeMismatch")
            || err.contains("Void"),
        "expected the tail-vs-Void diagnostic, got:\n{err}"
    );
}
