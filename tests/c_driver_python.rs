// ── Native Python Extension Round-Trip Test ─────────────────────────────
// 2026-08-03 (plan 2026-08-03-native-python-meld-composite): `briev extension
// <bridge.bv> python` generates a CPython C-extension module that calls the
// Briv exports directly — no ctypes. The shim accepts native Python str/int/
// float and returns native Python values. The CStr <-> String meld makes the
// boundary functions cast-free; the shim's per-category parse/build snippets
// (config/glue.dbvl native.*) marshal natively. Toolchain-guarded.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn python_native_extension_roundtrip() {
    for tool in ["cc", "ar", "llc", "clang", "python3-config", "python3"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_python_native_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let bv = format!("{}/examples/glue-host/boundary.bv", PROJECT_ROOT);
    let ext = Command::new(brievc)
        .args(["extension", &bv, "python", "--out", &out_dir.to_string_lossy()])
        .output().expect("failed brievc extension python");
    assert!(ext.status.success(), "extension failed: {}", String::from_utf8_lossy(&ext.stderr));

    let py = out_dir.join("check.py");
    std::fs::write(&py, r#"
import boundary
assert boundary.echo("hello") == "hello"
assert boundary.greet("world") == "world"
assert boundary.identity(3.5) == 3.5
assert boundary.join("foo", "bar") == "foobar"
print("OK")
"#).unwrap();

    let run = Command::new("python3")
        .current_dir(&out_dir)
        .arg(&py)
        .output().expect("failed python3");
    assert!(run.status.success(), "python failed: {}", String::from_utf8_lossy(&run.stderr));
    assert!(String::from_utf8_lossy(&run.stdout).contains("OK"));
}
