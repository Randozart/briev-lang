// 2026-10-16 (term-semantics plan step 6): backend E2E for arrowless
// output inference — an arrowless defn/txn whose body's first value
// `term <e>;` or tail resolves to T carries T as its output: the call
// site BINDS the value (before this pass the backend voided the body
// and the bound variable silently stayed 0). Typecheck pins:
// typechecker::output_inference::tests. Harness-exact link flags.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn out_dir(name: &str) -> PathBuf {
    let d = Path::new(PROJECT_ROOT)
        .join("target/briev_inferred_output")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn build_link_run(name: &str, src: &str) -> (String, i32) {
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
    assert!(
        build.status.success(),
        "{name}: build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
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
fn arrowless_value_term_returns_to_call_site() {
    // No `-> Int` anywhere: the term's type IS the output.
    let src = r#"
defn double(x: Int) {
    term x * 2;
};

let n: Int = 0;
node main [n == 0][n == 1] {
    Print#(double(21));
    n = 1;
    endprogram;
};
"#;
    let (out, code) = build_link_run("infer_term", src);
    assert_eq!(code, 0, "exit code");
    assert_eq!(out, "42", "the inferred output reaches the call site");
}

#[test]
fn arrowless_tail_returns_to_call_site() {
    let src = r#"
defn double(x: Int) {
    x * 2
};

let n: Int = 0;
node main [n == 0][n == 1] {
    Print#(double(21));
    n = 1;
    endprogram;
};
"#;
    let (out, code) = build_link_run("infer_tail", src);
    assert_eq!(code, 0, "exit code");
    assert_eq!(out, "42", "the inferred tail output reaches the call site");
}

#[test]
fn arrowless_txn_value_term_returns_to_call_site() {
    let src = r#"
txn double(x: Int) [x >= 0][x >= 0] {
    term x * 2;
};

let n: Int = 0;
node main [n == 0][n == 1] {
    Print#(double(21));
    n = 1;
    endprogram;
};
"#;
    let (out, code) = build_link_run("infer_txn", src);
    assert_eq!(code, 0, "exit code");
    assert_eq!(out, "42", "the inferred txn output reaches the call site");
}

#[test]
fn arrowless_local_fed_term_returns_to_call_site() {
    // The common shape: a typed local feeds the term.
    let src = r#"
defn quad(x: Int) {
    let y: Int = x * 2;
    term y * 2;
};

let n: Int = 0;
node main [n == 0][n == 1] {
    Print#(quad(21));
    n = 1;
    endprogram;
};
"#;
    let (out, code) = build_link_run("infer_local", src);
    assert_eq!(code, 0, "exit code");
    assert_eq!(out, "84", "locals in scope at the term infer through");
}
