// ── Term Termination Diagnostics — end-to-end ────────────────────────
// 2026-08-04: the fixtures in tests/fixtures/term_*.bv are run through the
// real `brievc check` binary. Behavioral tests: a .bv that provably cannot
// run its trailing statement must fail; valid shapes must pass.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn brievc_check(bv: &str) -> (bool, String) {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out = Command::new(brievc)
        .args(["check", bv])
        .output()
        .expect("failed to run brievc check");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out.status.success(), stderr)
}

#[test]
fn check_rejects_unreachable_after_terminating_term() {
    let (ok, err) = brievc_check(&format!(
        "{}/tests/fixtures/term_unreachable.bv",
        PROJECT_ROOT
    ));
    assert!(
        !ok,
        "unreachable code after term! must fail check; stderr: {err}"
    );
    assert!(
        err.contains("unreachable code"),
        "missing diagnostic: {err}"
    );
    assert!(
        err.contains("termination errors"),
        "error must be tagged: {err}"
    );
}

#[test]
fn check_warns_on_bare_term_guard_hint() {
    let (ok, err) = brievc_check(&format!(
        "{}/tests/fixtures/term_guard_hint.bv",
        PROJECT_ROOT
    ));
    assert!(
        ok,
        "bare-term guard is valid; check must pass; stderr: {err}"
    );
    assert!(err.contains("checkpoint"), "hint warning missing: {err}");
}

#[test]
fn check_accepts_valid_swan_song() {
    let (ok, err) = brievc_check(&format!(
        "{}/tests/fixtures/term_valid_swan_song.bv",
        PROJECT_ROOT
    ));
    assert!(ok, "valid swan song must pass; stderr: {err}");
}

#[test]
fn check_rejects_unreachable_after_defn_term_return() {
    let (ok, err) = brievc_check(&format!(
        "{}/tests/fixtures/term_defn_unreachable.bv",
        PROJECT_ROOT
    ));
    assert!(
        !ok,
        "unreachable code after term x; in a defn must fail; stderr: {err}"
    );
    assert!(
        err.contains("unreachable code"),
        "missing diagnostic: {err}"
    );
}

fn has(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .output()
        .is_ok()
}

// 2026-08-04 (ac6aca40): a value-form `term <val>` inside an INLINED collection
// member body (RingBuffer pop via `<- queue`) must NOT emit a void terminator in
// the countdown loop — be934d61 broke queue_drain with a `ret void` inside the
// i32-returning main ("value doesn't match function result type 'i32'"). The
// generated IR must link under clang and print at each guard boundary.
#[test]
fn member_inline_term_links_in_countdown_loop() {
    for tool in ["clang"] {
        if !has(tool) {
            eprintln!("SKIP: {tool} not available");
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_term_member_inline_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let bv = format!(
        "{}/tests/fixtures/term_member_inline_countdown.bv",
        PROJECT_ROOT
    );

    let build = Command::new(brievc)
        .env("BOUND", "5000")
        .args([
            "build",
            &bv,
            "--out",
            &out_dir.to_string_lossy(),
            "--optimize-budget",
            "256",
        ])
        .output()
        .expect("failed brievc build");
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("countdown loop"),
        "fixture must dispatch via countdown loop to exercise the member-inline path: {stderr}"
    );

    let ll = out_dir.join("term_member_inline_countdown.ll");
    let exe = out_dir.join("term_member_inline_countdown");
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
            &format!("{}/lib/runtime/briev_rt.c", PROJECT_ROOT),
            "-o",
            &exe.to_string_lossy(),
        ])
        .output()
        .expect("failed clang link");
    assert!(
        link.status.success(),
        "link failed (inlined member term emitted an invalid terminator?):\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let run = Command::new(&exe)
        .env("BOUND", "5000")
        .output()
        .expect("failed to run linked binary");
    assert!(
        run.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    for boundary in ["1000", "2000", "3000", "4000", "5000"] {
        assert!(
            stdout.contains(boundary),
            "missing boundary print {boundary}: {stdout}"
        );
    }
}

// 2026-08-04 (be934d61): a value-form `term! <val>` inside a `when` guard must
// unwind the WHOLE transaction body (interpreter TermReturn) — Print#(2) after
// the guard must NOT run. Pre-fix the LLVM backend fell through past the guard
// and printed "12"; the fix (void_txn_abort_label + conditional convergence)
// prints only "1". The statement after the guard IS reachable when the guard is
// false, so `brievc check` must pass — this is a codegen-only parity test.
#[test]
fn guard_value_form_term_unwinds_body() {
    for tool in ["clang"] {
        if !has(tool) {
            eprintln!("SKIP: {tool} not available");
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_term_guard_value_form_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let bv = format!("{}/tests/fixtures/term_guard_value_form.bv", PROJECT_ROOT);

    let check = Command::new(brievc)
        .args(["check", &bv])
        .output()
        .expect("failed brievc check");
    assert!(
        check.status.success(),
        "statement after a conditional guard is reachable — check must pass: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    let build = Command::new(brievc)
        .args([
            "build",
            &bv,
            "--out",
            &out_dir.to_string_lossy(),
            "--optimize-budget",
            "256",
        ])
        .output()
        .expect("failed brievc build");
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ll = out_dir.join("term_guard_value_form.ll");
    let exe = out_dir.join("term_guard_value_form");
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
            &format!("{}/lib/runtime/briev_rt.c", PROJECT_ROOT),
            "-o",
            &exe.to_string_lossy(),
        ])
        .output()
        .expect("failed clang link");
    assert!(
        link.status.success(),
        "link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let run = Command::new(&exe)
        .output()
        .expect("failed to run linked binary");
    assert!(
        run.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    assert!(
        stdout.contains('1'),
        "expected Print#(1) in guard body, got: {stdout}"
    );
    assert!(
        !stdout.contains('2'),
        "guard body term! must unwind the whole txn — Print#(2) must NOT run, got: {stdout}"
    );
}
