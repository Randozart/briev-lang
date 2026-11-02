// ── C++ Round-Trip Test ────────────────────────────────────────────────
// 2026-08-04 (plan 2026-08-04-ship-common-language-environments): the C
// bindings are C/C++-compatible (the header's `extern "C"` guards). A C++
// driver compiled with g++ exercises the boundary types + meld path.
// Toolchain-guarded on g++.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn cpp_roundtrip() {
    for tool in ["cc", "ar", "llc", "clang", "g++"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_cpp_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let bv = format!("{}/examples/glue-host/boundary.bv", PROJECT_ROOT);
    let build = Command::new(brievc)
        .args(["build", &bv, "--library", "--out", &out_dir.to_string_lossy()])
        .output().expect("failed brievc build --library");
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let bindings = Command::new(brievc)
        .args(["bindings", &bv, "c", "--out", &out_dir.to_string_lossy()])
        .output().expect("failed brievc bindings");
    assert!(bindings.status.success(), "bindings failed: {}", String::from_utf8_lossy(&bindings.stderr));

    let inc = out_dir.join("boundary-bindings");
    let driver_cpp = out_dir.join("driver.cpp");
    std::fs::write(&driver_cpp, r#"
#include "boundary-bindings/briev_types.h"
#include <cstdio>
#include <cstring>

int main() {
    BrievState* st = __briev_init_state();
    int64_t echoed = echo((int64_t)"hello");
    int64_t greeted = greet((int64_t)"hello");
    int64_t joined = join((int64_t)"foo", (int64_t)"bar");
    double scaled = identity(3.5);
    if (std::strcmp((const char*)echoed, "hello") != 0) return 1;
    if (std::strcmp((const char*)greeted, "hello") != 0) return 2;
    if (std::strcmp((const char*)joined, "foobar") != 0) return 3;
    if (scaled != 3.5) return 4;
    std::printf("CPP OK\n");
    __glue_release(st);
    return 0;
}
"#).unwrap();

    let exe = out_dir.join("driver");
    let compile = Command::new("g++")
        .arg("-std=c++17").arg("-o").arg(&exe).arg(&driver_cpp)
        .arg(out_dir.join("libboundary.a"))
        .current_dir(&out_dir)
        .output().expect("failed g++");
    assert!(compile.status.success(), "g++ failed: {}", String::from_utf8_lossy(&compile.stderr));

    let run = Command::new(&exe).current_dir(&out_dir)
        .output().expect("failed driver");
    assert!(run.status.success(), "driver failed: {}", String::from_utf8_lossy(&run.stderr));
    assert!(String::from_utf8_lossy(&run.stdout).contains("CPP OK"));
}
