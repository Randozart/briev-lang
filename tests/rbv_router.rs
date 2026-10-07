// ── Web-surface regression gate (2026-10-07) ───────────────────────────
// Runs benchmarks/rbv_gate.sh against tests/fixtures/router.rbv: build the
// router smoke, check the emitted IR for call/declare agreement, and run the
// wasm32 runtime gate. Guards the pointer-width / void-frgn / view-liveness
// fixes (plan 2026-10-07-web-surface-completion.md, W1).
//
// Toolchain-guarded: needs clang + llc to build a .rbv. node is optional
// (the script skips the runtime half when absent).

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn router_smoke_gate() {
    for tool in ["clang", "llc"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let gate = format!("{}/benchmarks/rbv_gate.sh", PROJECT_ROOT);
    let run = Command::new("bash")
        .arg(&gate)
        .env("BRIEVC", env!("CARGO_BIN_EXE_brievc"))
        .output()
        .expect("failed to run benchmarks/rbv_gate.sh");
    assert!(
        run.status.success(),
        "rbv gate failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
    eprintln!("{}", String::from_utf8_lossy(&run.stdout));
}
