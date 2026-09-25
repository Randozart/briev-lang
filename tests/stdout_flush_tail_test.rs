// ── Buffered stdout: sub-CAP output must reach the terminal ──────────
// 2026-09-25 (bug 12): the buffered stdout lane flushed only when the
// buffer filled (@__STDOUT_CAP) or at __exit (endprogram). Host mains
// that return through the reactor loop (loop_engine/ssa.rs) never flushed,
// so any program whose total output was under CAP printed nothing —
// bit_clear lost its "0\n", deep_recursion its "15\n" (both harness
// MISMATCHes). The counter-engine mains already flushed; the reactor,
// modulo-switch and prealloc mains now do too.
//
// Behavioral test: the observable is the printed value, not any internal.
// A single firing prints 42 (2 bytes ≪ CAP); the program then reaches
// equilibrium and exits — the flush tail is the only thing that can make
// the output observable.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

fn build_and_run(src: &str, name: &str) -> String {
    for tool in ["clang"] {
        if !has(tool) {
            eprintln!("SKIP: {tool} not available");
            return String::new();
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    // Under target/ (repo-rooted, gitignored): the import resolver walks up
    // from the .bv's directory, so a repo-relative fixture finds lib/std.
    let out_dir = std::path::Path::new(PROJECT_ROOT)
        .join("target/briev_stdout_flush_tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let bv = out_dir.join(format!("{name}.bv"));
    std::fs::write(&bv, src).unwrap();

    let build = Command::new(brievc)
        .args(["build", &bv.to_string_lossy(), "--llvm", "--out", &out_dir.to_string_lossy(),
               "--stdlib-path", &format!("{}/lib/std", PROJECT_ROOT)])
        .output()
        .expect("failed brievc build");
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let ll = out_dir.join(format!("{name}.ll"));
    let exe = out_dir.join(name);
    let link = Command::new("clang")
        .args(["-O3", "-flto", "-march=native", "-ffast-math", "-fdata-sections", "-ffunction-sections",
               "-Wl,--gc-sections", &ll.to_string_lossy(),
               &format!("{}/lib/runtime/briev_rt.c", PROJECT_ROOT), "-o", &exe.to_string_lossy()])
        .output()
        .expect("failed clang link");
    assert!(link.status.success(), "link failed:\n{}",
        String::from_utf8_lossy(&link.stderr));

    let run = Command::new(&exe).output().expect("failed to run linked binary");
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    String::from_utf8_lossy(&run.stdout).to_string()
}

#[test]
fn reactor_loop_main_flushes_sub_cap_output() {
    // Counter-entry loop over the reactor main (loop_engine/ssa.rs .ss_main_loop
    // exit): prints once, far under CAP, then reaches equilibrium.
    let src = "\
let i: Int = 0;

node tick [i < 1][i == 1] {
    println!(42);
    i = i + 1;
    term;
};
";
    let out = build_and_run(src, "reactor_flush");
    if out.is_empty() && !has("clang") {
        return; // SKIP path: clang missing, build_and_run bailed
    }
    assert_eq!(out.trim(), "42", "sub-CAP output lost at exit (no flush tail): {out:?}");
}

#[test]
fn modulo_switch_main_flushes_sub_cap_output() {
    // Modulo-rotated dispatch main (emit_modulo_rotated .mr_end exit): two
    // txns partitioned on count % 2, total output 10 bytes ≪ CAP.
    let src = "\
let count: Int = 0;
let total: Int = 4;

node even [count < total && count % 2 == 0][count == total] {
    println!(count);
    count = count + 1;
    term;
};

node odd [count < total && count % 2 == 1][count == total] {
    println!(count);
    count = count + 1;
    term;
};
";
    let out = build_and_run(src, "modulo_flush");
    if out.is_empty() && !has("clang") {
        return;
    }
    assert_eq!(
        out.trim(),
        "0\n1\n2\n3",
        "modulo-main sub-CAP output lost: {out:?}"
    );
}
