// `endprogram` and the process exit status (2026-11-21).
//
// WHY THIS TEST EXISTS. `endprogram <int>` sets the exit status, but
// `endprogram Print#(x)` prints x and exits 0 — the printing works and the
// failure signal does not. Nothing documented that, and a downstream service
// (knooppunt's Briev kernel) used the Print# form to report a failed boot, so
// systemd recorded a lost bind as a clean run. The reference now states the
// rule; this test is what keeps the statement true.
//
// Both halves matter and fail differently:
//   * making the Print# form inherit a non-zero status would break callers
//     that print and then exit successfully;
//   * making a plain operand non-fatal (or truncated differently) would break
//     every supervisor that trusts an exit status.
//
// Exit statuses are observed through the OS, so they are 8-bit: `endprogram
// 301` exits 45 (301 & 0xFF) and `endprogram 0 - 3` exits 253 (255 - 2).
use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Build `src` with brievc and run it, returning (stdout, exit code).
///
/// `brievc build` links a runnable program on its own (the cli_args helper
/// needs an explicit clang link only because Argc#/Args# programs carry their
/// own `_start`); these fixtures have a plain `node entry`, so the direct path
/// keeps the test about exit status rather than about linking.
fn build_and_run(name: &str, src: &str) -> (String, i32) {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT)
        .join("target/briev_endprogram")
        .join(name);
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();
    let bv = out_dir.join(format!("{name}.bv"));
    std::fs::write(&bv, src).unwrap();

    let build = Command::new(brievc)
        .args([
            "build",
            &bv.to_string_lossy(),
            "--out",
            &out_dir.to_string_lossy(),
            "--stdlib-path",
            &format!("{PROJECT_ROOT}/lib"),
        ])
        .current_dir(PROJECT_ROOT)
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to spawn brievc build: {e}"));
    assert!(
        build.status.success(),
        "{name}: build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(out_dir.join(name))
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to run the built program: {e}"));
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        run.status.code().unwrap_or(-1),
    )
}

/// A tiny node body: `endprogram <operand>;` with no output of its own.
fn program(operand: &str) -> String {
    format!("node entry [beginprogram][true] {{\n    endprogram {operand};\n}};\n")
}

#[test]
fn endprogram_status_is_the_operand() {
    let (out, code) = build_and_run("ep_seven", &program("7"));
    assert_eq!(code, 7, "endprogram 7 must exit 7 (printed {out:?})");
    assert_eq!(out, "", "endprogram 7 must print nothing, got {out:?}");
}

#[test]
fn endprogram_status_is_truncated_to_eight_bits() {
    let (_, code) = build_and_run("ep_301", &program("301"));
    assert_eq!(code, 45, "endprogram 301 must exit 301 & 0xFF = 45");
}

#[test]
fn endprogram_accepts_a_negative_status() {
    // A subtraction rather than a literal: the lexer has no negative literal
    // in this position, and callers express errno-shaped failures this way.
    let (_, code) = build_and_run("ep_neg", &program("0 - 3"));
    assert_eq!(code, 253, "endprogram 0 - 3 must exit 253 (255 - 2)");
}

#[test]
fn endprogram_without_operand_exits_zero() {
    let (out, code) = build_and_run(
        "ep_none",
        "node entry [beginprogram][true] {\n    endprogram;\n};\n",
    );
    assert_eq!(code, 0, "bare endprogram must exit 0 (printed {out:?})");
}

/// The trap, pinned as a documented behaviour rather than a wish: the printed
/// value is NOT the status. If this ever changes, the reference table changes
/// with it — and every service that prints on a failure path gets a different
/// exit code, which is why it must be a deliberate decision.
#[test]
fn endprogram_print_form_exits_zero() {
    let (out, code) = build_and_run("ep_print", &program("Print#(9)"));
    assert_eq!(out, "9", "the Print# operand must still print its value");
    assert_eq!(
        code, 0,
        "endprogram Print#(x) exits 0 — the operand is void, not a status"
    );
}

#[test]
fn endprogram_print_form_status_is_not_the_byte_count() {
    // 6 bytes printed, still 0: rules out "Print# returns bytes written" as
    // the accidental explanation for the case above.
    let (out, code) = build_and_run("ep_print_long", &program("Print#(123456)"));
    assert_eq!(out, "123456");
    assert_eq!(code, 0, "the Print# status is 0, not the byte count");
}
