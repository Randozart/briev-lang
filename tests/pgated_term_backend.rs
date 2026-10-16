// 2026-10-16 (term-semantics plan §1, step 4): backend E2E for P-gated
// bare `term;`. The interpreter pins landed in
// interpreter::eval::pgated_term; these pin the LLVM emission end to end —
// build + link + run, same harness flags as the other smokes. The
// behavioural contract: an UNMET post at the term site continues the body
// (the tail prints); a MET post stops the firing (the tail never runs).

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn build_link_run(name: &str, src: &str) -> (String, i32) {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT)
        .join("target/briev_pgated_term_backend")
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
            &format!("{PROJECT_ROOT}/lib/std"),
        ])
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to spawn brievc build: {e}"));
    assert!(
        build.status.success(),
        "{name}: build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ll = out_dir.join(format!("{name}.ll"));
    let exe = out_dir.join(name);
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
        .unwrap_or_else(|e| panic!("{name}: failed to spawn clang: {e}"));
    assert!(
        link.status.success(),
        "{name}: link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let run = Command::new(&exe)
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to run: {e}"));
    (
        String::from_utf8_lossy(&run.stdout).to_string(),
        run.status.code().unwrap_or(-1),
    )
}

#[test]
fn bare_term_unmet_post_continues_the_body() {
    // post [n == 1]; at the term site n is still 0 → the tail runs (42
    // prints) before n advances and endprogram exits.
    let src = r#"
let n: Int = 0;
node main [n == 0][n == 1] {
    when n == 0 { term; };
    Print#(42);
    n = 1;
    endprogram;
};
"#;
    let (out, code) = build_link_run("unmet", src);
    assert_eq!(code, 0, "unmet: exit code");
    assert_eq!(out, "42", "unmet post — the tail must run");
}

#[test]
fn bare_term_met_post_stops_the_firing() {
    // n advances FIRST; the term site's post [n == 1] is met → the firing
    // stops, Print#(99) never runs, the reactor converges (no endprogram
    // needed — equilibrium exits 0).
    let src = r#"
let n: Int = 0;
node main [n == 0][n == 1] {
    n = 1;
    when n == 1 { term; };
    Print#(99);
    endprogram;
};
"#;
    let (out, code) = build_link_run("met", src);
    assert_eq!(code, 0, "met: exit code");
    assert_eq!(out, "", "met post — the tail must never run");
}
