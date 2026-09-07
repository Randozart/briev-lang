use std::process::Command;

/// Verify that all vendored C libraries exist and can be compiled to object files.
#[test]
fn test_xxhash_compiles() {
    let out = Command::new("clang")
        .args(["-c", "-x", "c", "-Ilib/std/c/xxhash", "-o", "/dev/null", "lib/std/c/xxhash/xxhash.c"])
        .output()
        .expect("clang must be in PATH");
    assert!(out.status.success(), "xxhash compilation failed: {}",
        String::from_utf8_lossy(&out.stderr));
}

#[test]
fn test_yyjson_compiles() {
    let out = Command::new("clang")
        .args(["-c", "-x", "c", "-Ilib/std/c/json", "-o", "/dev/null", "lib/std/c/json/yyjson.c"])
        .output()
        .expect("clang must be in PATH");
    assert!(out.status.success(), "yyjson compilation failed: {}",
        String::from_utf8_lossy(&out.stderr));
}

#[test]
fn test_briv_json_compiles() {
    let out = Command::new("clang")
        .args(["-c", "-x", "c", "-Ilib/std/c/json", "-o", "/dev/null", "lib/std/c/json/briv_json.c"])
        .output()
        .expect("clang must be in PATH");
    assert!(out.status.success(), "briv_json compilation failed: {}",
        String::from_utf8_lossy(&out.stderr));
}

#[test]
fn test_stb_image_compiles() {
    let out = Command::new("clang")
        .args(["-c", "-x", "c", "-Ilib/std/c/stb_image", "-o", "/dev/null", "lib/std/c/stb_image/stb_image.c"])
        .output()
        .expect("clang must be in PATH");
    assert!(out.status.success(), "stb_image compilation failed: {}",
        String::from_utf8_lossy(&out.stderr));
}

#[test]
fn test_lz4_compiles() {
    let out = Command::new("clang")
        .args(["-c", "-x", "c", "-Ilib/std/c/lz4", "-o", "/dev/null", "lib/std/c/lz4/lz4.c"])
        .output()
        .expect("clang must be in PATH");
    assert!(out.status.success(), "lz4 compilation failed: {}",
        String::from_utf8_lossy(&out.stderr));
}
