// ── Async Phase D — compiled port events, end-to-end ─────────────────
// 2026-08-26: examples/async-events-compiled.bv builds via `brievc build
// --llvm`, links with lib/runtime/briev_rt.c using the harness-exact clang
// command, and must print 17 (= consume(7) + produce(1)*10) — reachable ONLY
// if the producer's fire woke the blocked consumer through the round-robin.
// Behavioral test: the contract is the observable output, not any internal.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn compiled_port_wake_prints_17() {
    for tool in ["clang"] {
        if !has(tool) {
            eprintln!("SKIP: {tool} not available");
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_async_compiled_events_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let bv = format!("{}/examples/async-events-compiled.bv", PROJECT_ROOT);

    let build = Command::new(brievc)
        .args(["build", &bv, "--llvm", "--out", &out_dir.to_string_lossy()])
        .output()
        .expect("failed brievc build");
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ll = out_dir.join("async-events-compiled.ll");
    let exe = out_dir.join("async-events-compiled");
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
    assert_eq!(
        stdout.trim(),
        "17",
        "wake parity broken — acc must be 17 (consume 7 + produce 1 * 10): {stdout}"
    );
}

// SPEC §9.5 Phase D acceptance: top-level `^Ready` reflection observes the
// wake transition — false directly after spawn (no segment scheduled yet),
// true after `await cc` completes. acc + produced*100 = 111.
#[test]
fn compiled_ready_gate_observes_false_to_true() {
    for tool in ["clang"] {
        if !has(tool) {
            eprintln!("SKIP: {tool} not available");
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_async_ready_gate_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let bv = format!("{}/examples/async-ready-gate.bv", PROJECT_ROOT);

    let build = Command::new(brievc)
        .args(["build", &bv, "--llvm", "--out", &out_dir.to_string_lossy()])
        .output()
        .expect("failed brievc build");
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ll = out_dir.join("async-ready-gate.ll");
    let exe = out_dir.join("async-ready-gate");
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
    assert_eq!(
        stdout.trim(),
        "111",
        "^Ready must observe false→true across the wake: {stdout}"
    );
}
