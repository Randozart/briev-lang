// ── Boundary-Type Round-Trip Test ──────────────────────────────────────
// 2026-08-03 (plan 2026-08-03-protocol-driven-glue-boundary): the export
// signature IS the boundary contract. `CStr` is a String<C_String> sub-type
// (ptr ABI, marshalled via the casting graph's cstr_to_briev/str_to_c
// bindings); `CDouble` is Float<C_Double> (double ABI — the Float fix);
// `CStr + CStr` uses the variant's own Concat cross-op (cstring_concat).
// Toolchain-guarded.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn boundary_types_roundtrip() {
    for tool in ["cc", "ar", "llc", "clang"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_boundary_test");
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

    // The generated header must resolve the boundary types to C ABI names.
    let header = std::fs::read_to_string(out_dir.join("boundary-bindings").join("briev_types.h")).unwrap();
    assert!(header.contains("int64_t echo(int64_t name)"), "CStr → int64_t: {}", header);
    assert!(header.contains("int64_t join(int64_t a, int64_t b)"), "CStr params: {}", header);
    assert!(header.contains("double identity(double x)"), "CDouble → double: {}", header);

    let driver_c = out_dir.join("driver.c");
    std::fs::write(&driver_c, r#"
#include "boundary-bindings/briev_types.h"
#include <stdio.h>

int main(void) {
    BrievState* st = __briev_init_state();
    int64_t echoed = echo((int64_t)"hello");
    int64_t greeted = greet((int64_t)"hello");
    int64_t joined = join((int64_t)"foo", (int64_t)"bar");
    double d = identity(3.14);
    printf("echo:%s\n", (char*)(uintptr_t)echoed);
    printf("greet:%s\n", (char*)(uintptr_t)greeted);
    printf("join:%s\n", (char*)(uintptr_t)joined);
    printf("ident:%f\n", d);
    __glue_release(st);
    return 0;
}
"#).unwrap();

    let header_dir = out_dir.join("boundary-bindings");
    let driver = out_dir.join("driver");
    let cc = Command::new("cc")
        .current_dir(&out_dir)
        .args(["-o", driver.to_str().unwrap(), driver_c.to_str().unwrap()])
        .arg(format!("-I{}", header_dir.display()))
        .arg(format!("-L{}", out_dir.display()))
        .arg("-lboundary")
        .output().expect("failed cc");
    assert!(cc.status.success(), "cc failed: {}", String::from_utf8_lossy(&cc.stderr));

    let run = Command::new(&driver).output().expect("failed to run driver");
    assert!(run.status.success(), "driver failed: {}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("echo:hello"), "echo: {}", stdout);
    assert!(stdout.contains("greet:hello"), "greet (marshalled): {}", stdout);
    assert!(stdout.contains("join:foobar"), "join (cstring_concat): {}", stdout);
    assert!(stdout.contains("ident:3.140000"), "identity (CDouble → double): {}", stdout);

    let _ = std::fs::remove_dir_all(&out_dir);
}
