// `brievc test` runner smoke — tier2 phase 3
// (docs/plans/2026-10-14-daily-driver-tier2.md).
//
// Builds a throwaway suite (passing + golden-pinned + failing) and drives
// the subcommand end-to-end. Pins the runner contract: PASS = the entry
// node fired + exit 0 (+ byte-equal stdout when a .golden exists); a
// violated postcondition traps (232) → FAIL; any failure → runner exit 1.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn test_runner_pass_fail_golden() {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let suite = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tier2_test_suite");
    let _ = fs::remove_dir_all(&suite);
    fs::create_dir_all(&suite).unwrap();

    // PASS: fires, exits 0, stdout matches the golden.
    fs::write(
        suite.join("ok_two.bv"),
        r#"
let done: Bool = false;
node main [done == false][done == true] {
    println!(1 + 1);
    done = true;
    term;
};
"#,
    )
    .unwrap();
    fs::write(suite.join("ok_two.golden"), "2\n").unwrap();

    // FAIL: the entry fires but violates its postcondition → trap (232).
    fs::write(
        suite.join("bad_postcondition.bv"),
        r#"
let done: Bool = false;
node main [done == false][done == true] {
    term;
};
"#,
    )
    .unwrap();

    let out = Command::new(brievc)
        .args(["test", &suite.to_string_lossy()])
        .output()
        .expect("spawn brievc test");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("PASS ok_two"), "{stdout}");
    assert!(
        stdout.contains("FAIL bad_postcondition"),
        "the failing test must FAIL: {stdout}"
    );
    assert!(stdout.contains("1/2 passed"), "{stdout}");
    assert!(!out.status.success(), "any failure → non-zero runner exit");

    // A golden mismatch is also a FAIL (fix the program to print 3, keep
    // the golden at 2).
    fs::write(
        suite.join("ok_two.bv"),
        r#"
let done: Bool = false;
node main [done == false][done == true] {
    println!(2 + 1);
    done = true;
    term;
};
"#,
    )
    .unwrap();
    let out = Command::new(brievc)
        .args(["test", &suite.to_string_lossy()])
        .output()
        .expect("spawn brievc test");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("FAIL ok_two") && stdout.contains("golden mismatch"),
        "golden mismatch must FAIL: {stdout}"
    );
}
