// D7h: primitives and a public export must compile with no prelude/stdlib.
use std::path::Path;
use std::process::Command;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn no_prelude_export_boundary_compiles() {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out = Path::new(ROOT).join("target/briev_no_prelude_boundary");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let fixture = Path::new(ROOT).join("tests/fixtures/no_prelude_boundary.bv");
    let build = Command::new(brievc)
        .args([
            "build",
            &fixture.to_string_lossy(),
            "--disable-plugin",
            "prelude",
            "--llvm",
            "--out",
            &out.to_string_lossy(),
        ])
        .output()
        .expect("spawn brievc");
    assert!(
        build.status.success(),
        "no-prelude build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let ir = std::fs::read_to_string(out.join("no_prelude_boundary.ll")).unwrap();
    assert!(ir.contains("@add("), "public export missing:\n{ir}");
    assert!(
        ir.contains("@add__briev_internal("),
        "export wrapper/internal body missing:\n{ir}"
    );
}
