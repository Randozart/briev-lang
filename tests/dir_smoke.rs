// Directory listing E2E — Phase 4 tail of
// docs/plans/2026-10-15-bool-width-canonicalization.md.
//
// lib/std/os/dir.bv `entries()` through the compiled surface: the FIRST
// callable txn with a generic-Result contract (`[is_ok(term) ||
// is_err(term)] -> Result<List<String>, Int>`) to run end to end — the
// convergence gate monomorphizes is_ok/is_err over `Result<List<String>,
// Int>` (gate term binding + import-contract closure, 2026-10-15) and the
// getdents64 loop accumulates a real List<String>. Golden asserts the
// entry COUNT (2 dot entries + created files — readdir order is
// filesystem-dependent, so never assert order), a `found` probe via
// recursive traversal with a variable index + string equality, and the
// Err branch (-ENOENT for a missing dir). The scratch dir lives under
// target/ (unique per test slot — cargo runs tests in parallel).

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn build_link_run(name: &str, src: &str, dir: &str, missing: &str) -> (String, i32) {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT)
        .join("target/briev_dir_smoke")
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
        .args([dir, missing])
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to run: {e}"));
    (
        String::from_utf8_lossy(&run.stdout).to_string(),
        run.status.code().unwrap_or(-1),
    )
}

/// entries() round trip: real dir → count + found probe; missing dir →
/// -ENOENT through the same monomorphized contract gate.
#[test]
fn dir_entries_listing_and_err_branch() {
    let scratch = Path::new(PROJECT_ROOT).join("target/briev_dir_smoke_scratch");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("alpha.txt"), "a").unwrap();
    std::fs::write(scratch.join("beta.txt"), "b").unwrap();

    let src = r#"
import "std/cli.bv";
import "std/os/dir.bv";

defn print_matches(names: List<String>, i: Int, want: String) -> Void {
    when i < names.Count#() {
        when names[i] == want {
            println!("found");
        };
        print_matches(names, i + 1, want);
    };
};

let n: Int = 0;
node main [n == 0][n == 1] {
    let d: String = arg(1);
    let missing: String = arg(2);
    let r: Result<List<String>, Int> = entries(d);
    when is_ok(r) {
        let names: List<String> = unwrap(r);
        println!(names.Count#());
        print_matches(names, 0, "alpha.txt");
    };
    when is_err(r) {
        println!(unwrap_err(r));
    };
    let r2: Result<List<String>, Int> = entries(missing);
    when is_err(r2) {
        println!(unwrap_err(r2));
    };
    n = 1;
    term;
};
"#;

    let (out, code) = build_link_run(
        "dir_entries",
        src,
        &scratch.to_string_lossy(),
        "/nonexistent-briev-dir-probe",
    );
    assert_eq!(code, 0, "dir smoke must exit 0");
    // Count: "." + ".." + alpha.txt + beta.txt = 4, in any readdir order.
    // "found" pins the traversal + string-eq path; the -2 pins the Err
    // branch (-ENOENT for the missing dir) through the monomorphized gate.
    assert_eq!(
        out, "4\nfound\n-2\n",
        "dir listing golden drifted: {out:?}"
    );
}
