// ── C# Render Test (P/Invoke bindings) ────────────────────────────────
// 2026-08-04 (plan 2026-08-04-ship-common-language-environments): `briev
// bindings <bridge> csharp` renders a .cs class with DllImport externs against
// the bridge .so (the composite String is an IntPtr handle). No .NET runtime
// here, so this asserts the RENDERED shape; a full round-trip runs where
// `dotnet` exists.

mod common;

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn csharp_bindings_render() {
    for tool in ["cc", "ar", "llc", "clang"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_csharp_test");
    let _claim = common::claim_out_dir(&out_dir);
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let bv = format!("{}/examples/glue-host/boundary.bv", PROJECT_ROOT);
    let bindings = Command::new(brievc)
        .args([
            "bindings",
            &bv,
            "csharp",
            "--out",
            &out_dir.to_string_lossy(),
        ])
        .output()
        .expect("failed brievc bindings csharp");
    assert!(
        bindings.status.success(),
        "csharp bindings failed: {}",
        String::from_utf8_lossy(&bindings.stderr)
    );

    let cs = std::fs::read_to_string(out_dir.join("boundary-bindings/bridge.cs")).unwrap();
    assert!(
        cs.contains(r#"[DllImport("boundary")]"#),
        "DllImport on the bridge lib"
    );
    assert!(
        cs.contains("private static extern IntPtr echo(IntPtr name);"),
        "CStr → IntPtr: {}",
        cs
    );
    assert!(
        cs.contains("private static extern double identity(double x);"),
        "CDouble → double: {}",
        cs
    );
    assert!(
        cs.contains("private static extern IntPtr join(IntPtr a, IntPtr b);"),
        "two CStr params: {}",
        cs
    );
    assert!(
        cs.contains("public static IntPtr Init()"),
        "state init accessor"
    );
}
