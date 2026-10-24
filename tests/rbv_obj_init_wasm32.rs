// wasm32 obj-value seed regression gate (2026-10-23).
//
// `tests/fixtures/obj_init.rbv` previously emitted a native-i64 boxed-word
// seed on wasm32:
//   `%h = ptrtoint ptr %obj to i32`
//   `%p = inttoptr i64 %h to ptr`   // invalid: i32 value, i64 conversion
// and loaded a 4-byte pointer member through `load i64`.
//
// This test pins the target-width ABI: obj handles and pointer members use
// `ctx.int_bits`, and the fixture must compile through `llc` to wasm32.
use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn wasm32_obj_init_uses_pointer_width_handles() {
    for tool in ["clang", "llc"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let fixture = Path::new(PROJECT_ROOT).join("tests/fixtures/obj_init.rbv");
    let out = Path::new(PROJECT_ROOT).join("target/briev_obj_init_wasm32");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();

    let build = Command::new(brievc)
        .args([
            "build",
            &fixture.to_string_lossy(),
            "--split",
            "--out",
            &out.to_string_lossy(),
        ])
        .output()
        .expect("failed to spawn brievc build");
    assert!(
        build.status.success(),
        "wasm32 obj_init build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ll = out.join("obj_init.ll");
    let ir = std::fs::read_to_string(&ll).expect("obj_init.ll missing");
    assert!(
        ir.contains("target triple = \"wasm32-"),
        "fixture must target wasm32; got:\n{ir}"
    );
    assert!(
        ir.contains("inttoptr i32"),
        "wasm32 obj handle must use i32; got:\n{ir}"
    );
    assert!(
        !ir.contains("inttoptr i64 %t9"),
        "stale native-i64 obj handle conversion remains; got:\n{ir}"
    );
    assert!(
        ir.contains("load ptr, ptr"),
        "pointer-sized obj member must load ptr; got:\n{ir}"
    );

    let obj = out.join("obj_init.o");
    let llc = Command::new("llc")
        .args([
            "-filetype=obj",
            "-relocation-model=pic",
            "-o",
            &obj.to_string_lossy(),
            &ll.to_string_lossy(),
        ])
        .output()
        .expect("failed to spawn llc");
    assert!(
        llc.status.success(),
        "llc rejected wasm32 obj_init IR:\n{}",
        String::from_utf8_lossy(&llc.stderr)
    );
}
