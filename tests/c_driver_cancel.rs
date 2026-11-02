// ── Host Cancellation Round-Trip Test ──────────────────────────────────
// 2026-08-03: a C driver spawns a thread that raises __briev_set_cancel;
// the exported Briv loop polls CancelRequested#() explicitly and stops
// early (partial result << the uncancelled full result).

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn host_cancellation_stops_briev_loop() {
    for tool in ["cc", "ar", "llc", "clang", "pkg-config"] {
        if !has(tool) && tool != "pkg-config" {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_cancel_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let bv = format!("{}/examples/glue-host/cancel.bv", PROJECT_ROOT);
    let build = Command::new(brievc)
        .args(["build", &bv, "--library", "--out", &out_dir.to_string_lossy()])
        .output().expect("failed brievc build --library");
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let bindings = Command::new(brievc)
        .args(["bindings", &bv, "c", "--out", &out_dir.to_string_lossy()])
        .output().expect("failed brievc bindings");
    assert!(bindings.status.success(), "bindings failed: {}", String::from_utf8_lossy(&bindings.stderr));

    let driver_c = out_dir.join("driver.c");
    std::fs::write(&driver_c, r#"
#include "cancel-bindings/briev_types.h"
#include <stdio.h>
#include <pthread.h>
#include <unistd.h>

static BrievState* g_st;

static void* canceller(void* arg) {
    (void)arg;
    usleep(20000);
    __briev_set_cancel(g_st, 1);
    return NULL;
}

int main(void) {
    g_st = __briev_init_state();
    int64_t small = cancellable_sum(g_st, 100000);      // full, small
    int64_t mid   = cancellable_sum(g_st, 50000000);    // full, computable (~0.1s)
    __briev_clear_cancel(g_st);
    pthread_t th;
    pthread_create(&th, NULL, canceller, NULL);
    int64_t partial = cancellable_sum(g_st, 2000000000LL); // stops early (~20ms)
    pthread_join(th, NULL);
    printf("small:%ld\n", small);
    printf("mid:%ld\n", mid);
    printf("partial:%ld\n", partial);
    __glue_release(g_st);
    return 0;
}
"#).unwrap();

    let header_dir = out_dir.join("cancel-bindings");
    let driver = out_dir.join("driver");
    let cc = Command::new("cc")
        .current_dir(&out_dir)
        .args(["-o", driver.to_str().unwrap(), driver_c.to_str().unwrap()])
        .arg(format!("-I{}", header_dir.display()))
        .arg(format!("-L{}", out_dir.display()))
        .arg("-lcancel")
        .arg("-lpthread")
        .output().expect("failed cc");
    assert!(cc.status.success(), "cc failed: {}", String::from_utf8_lossy(&cc.stderr));

    let run = Command::new(&driver).output().expect("failed to run driver");
    assert!(run.status.success(), "driver failed: {}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);
    let read = |name: &str| -> i64 {
        stdout.lines().find(|l| l.starts_with(name)).unwrap()
            .trim_start_matches(name).parse().unwrap()
    };
    let small = read("small:");
    let mid = read("mid:");
    let partial = read("partial:");
    assert_eq!(small, 14999850000, "sum(100000) mismatch: {}", stdout);
    assert!(mid > small, "sum(2000000) should exceed sum(100000): {}", stdout);
    // The 2e9 run is cancelled after ~20ms (~10M iterations at ~500M/s) —
    // it must stop well before the 50M full run, let alone 2e9.
    assert!(partial < mid,
        "cancelled 2e9 run must stop before the 2M full run (partial {partial} < mid {mid}): {stdout}");

    let _ = std::fs::remove_dir_all(&out_dir);
}
