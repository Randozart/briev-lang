// ── Go Round-Trip Test (cgo) ───────────────────────────────────────────
// 2026-08-04 (plan 2026-08-04-ship-common-language-environments): `briev
// export <bridge> go` renders a cgo Go package — the preamble includes the C
// header, the per-export wrappers convert natively (string ↔ *C.char via the
// composite pointer-as-int64 handle, C.GoString reads the NUL-invariant data
// zero-copy). Toolchain-guarded: finds `go` on PATH or the portable
// ~/briv-tools/go/bin/go.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

fn go_bin() -> Option<std::path::PathBuf> {
    if let Ok(out) = Command::new("go").arg("version").output() {
        if out.status.success() {
            return Some("go".into());
        }
    }
    let portable = std::path::Path::new(&std::env::var("HOME").unwrap_or_default())
        .join("briv-tools/go/bin/go");
    if portable.exists() {
        Some(portable)
    } else {
        None
    }
}

#[test]
fn go_roundtrip() {
    for tool in ["cc", "ar", "llc", "clang"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let Some(go) = go_bin() else {
        eprintln!("SKIP: go not found (PATH or ~/briv-tools/go/bin/go)");
        return;
    };
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let base = std::env::temp_dir().join("briev_go_test");
    let _ = std::fs::remove_dir_all(&base);
    let pkg = base.join("boundary");
    std::fs::create_dir_all(&pkg).unwrap();

    let bv = format!("{}/examples/glue-host/boundary.bv", PROJECT_ROOT);
    let build = Command::new(brievc)
        .args(["build", &bv, "--library", "--out", &base.to_string_lossy()])
        .output().expect("failed brievc build --library");
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));
    std::fs::copy(base.join("libboundary.a"), pkg.join("libboundary.a")).unwrap();

    let bindings = Command::new(brievc)
        .args(["bindings", &bv, "c", "--out", &base.to_string_lossy()])
        .output().expect("failed brievc bindings");
    assert!(bindings.status.success(), "bindings failed: {}", String::from_utf8_lossy(&bindings.stderr));
    std::fs::copy(base.join("boundary-bindings/briev_types.h"), pkg.join("briev_types.h")).unwrap();

    let export = Command::new(brievc)
        .args(["export", &bv, "go", "--out", &base.to_string_lossy()])
        .output().expect("failed brievc export go");
    assert!(export.status.success(), "go export failed: {}", String::from_utf8_lossy(&export.stderr));
    std::fs::copy(base.join("boundary-bridge/bridge.go"), pkg.join("bridge.go")).unwrap();

    std::fs::write(base.join("go.mod"), "module briefgobridge\n\ngo 1.22\n").unwrap();
    std::fs::write(base.join("main.go"), r#"
package main

import (
    "fmt"
    b "briefgobridge/boundary"
)

func main() {
    if b.Echo("hello") != "hello" { panic("echo") }
    if b.Greet("world") != "world" { panic("greet") }
    if b.Identity(3.5) != 3.5 { panic("identity") }
    if b.Join("foo", "bar") != "foobar" { panic("join") }
    fmt.Println("GO OK")
}
"#).unwrap();

    let run = Command::new(&go)
        .env("CGO_ENABLED", "1")
        .current_dir(&base)
        .args(["run", "."])
        .output().expect("failed go run");
    assert!(run.status.success(), "go run failed: {}", String::from_utf8_lossy(&run.stderr));
    assert!(String::from_utf8_lossy(&run.stdout).contains("GO OK"));
}
