// ── Callback Round-Trip Test ───────────────────────────────────────────
// 2026-08-03: host → Briv → host. The C driver passes a function pointer
// into an exported Briv function (`apply(cb: fn(Int) -> Int, x)`); Briv
// calls it back via CallPtr# (call-through-pointer) and returns the result.
// Toolchain-guarded (cc/ar/llc/clang).

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn host_callback_into_briev_roundtrip() {
    for tool in ["cc", "ar", "llc", "clang"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_cb_driver_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let bv = format!("{}/examples/glue-host/callback.bv", PROJECT_ROOT);

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
#include "callback-bindings/briev_types.h"
#include <stdio.h>

static int64_t doubler(int64_t x) { return x * 2; }
static int64_t plus_one(int64_t x) { return x + 1; }

int main(void) {
    BrievState* st = __briev_init_state();
    printf("double:%ld\n", apply(doubler, 21));
    printf("inc:%ld\n", apply(plus_one, 41));
    __glue_release(st);
    return 0;
}
"#).unwrap();

    let header_dir = out_dir.join("callback-bindings");
    let driver = out_dir.join("driver");
    let cc = Command::new("cc")
        .current_dir(&out_dir)
        .args(["-o", driver.to_str().unwrap(), driver_c.to_str().unwrap()])
        .arg(format!("-I{}", header_dir.display()))
        .arg(format!("-L{}", out_dir.display()))
        .arg("-lcallback")
        .output().expect("failed cc");
    assert!(cc.status.success(), "cc failed: {}", String::from_utf8_lossy(&cc.stderr));

    let run = Command::new(&driver).output().expect("failed to run driver");
    assert!(run.status.success(), "driver failed: {}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("double:42"), "doubler(21) should be 42: {}", stdout);
    assert!(stdout.contains("inc:42"), "plus_one(41) should be 42: {}", stdout);

    let _ = std::fs::remove_dir_all(&out_dir);
}
