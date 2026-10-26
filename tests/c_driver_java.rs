// ── Java Round-Trip Test (JNI) ─────────────────────────────────────────
// 2026-08-04 (plan 2026-08-04-ship-common-language-environments): `briev
// extension <bridge> java` renders + builds a JNI shim (lib<bridge>.so; the
// composite String crosses via GetStringUTFChars / NewStringUTF on the
// NUL-invariant data); `briev export <bridge> java` renders the Java class
// with `native` methods. Toolchain-guarded: finds javac/java on PATH or the
// portable ~/briv-tools/jdk-* JDK.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

fn jdk_bin() -> Option<std::path::PathBuf> {
    // A real JDK has javac; the system JRE's `java` alone is not enough.
    if let Ok(out) = Command::new("javac").arg("-version").output() {
        if out.status.success() {
            return Some("".into());
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let tools = std::path::Path::new(&home).join("briv-tools");
    if let Ok(entries) = std::fs::read_dir(&tools) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if dir.is_dir() && dir.file_name().map_or(false, |n| n.to_string_lossy().starts_with("jdk-")) {
                let bin = dir.join("bin");
                if bin.join("javac").exists() && bin.join("java").exists() {
                    return Some(bin);
                }
            }
        }
    }
    None
}

#[test]
fn java_roundtrip() {
    for tool in ["cc", "ar", "llc", "clang"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let Some(jbin) = jdk_bin() else {
        eprintln!("SKIP: JDK not found (PATH or ~/briv-tools/jdk-*)");
        return;
    };
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let base = std::env::temp_dir().join("briev_java_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    let bv = format!("{}/examples/glue-host/boundary.bv", PROJECT_ROOT);
    let build = Command::new(brievc)
        .args(["build", &bv, "--library", "--out", &base.to_string_lossy()])
        .output().expect("failed brievc build --library");
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let ext = Command::new(brievc)
        .args(["extension", &bv, "java", "--out", &base.to_string_lossy()])
        .output().expect("failed brievc extension java");
    if !ext.status.success() {
        let stderr = String::from_utf8_lossy(&ext.stderr);
        if stderr.contains("jni.h") {
            eprintln!("SKIP: JNI headers not installed");
            return;
        }
        panic!("java ext failed: {stderr}");
    }
    assert!(base.join("libboundary.so").exists(), "libboundary.so missing");

    let export = Command::new(brievc)
        .args(["export", &bv, "java", "--out", &base.to_string_lossy()])
        .output().expect("failed brievc export java");
    assert!(export.status.success(), "java export failed: {}", String::from_utf8_lossy(&export.stderr));

    let src = base.join("boundary-bridge");
    std::fs::write(src.join("Main.java"), r#"
public class Main {
    public static void main(String[] args) {
        if (!boundary.Echo("hello").equals("hello")) throw new RuntimeException("echo");
        if (!boundary.Greet("world").equals("world")) throw new RuntimeException("greet");
        if (boundary.Identity(3.5) != 3.5) throw new RuntimeException("identity");
        if (!boundary.Join("foo", "bar").equals("foobar")) throw new RuntimeException("join");
        System.out.println("JAVA OK");
    }
}
"#).unwrap();

    let javac = jbin.join("javac");
    let java = jbin.join("java");
    let compile = Command::new(&javac)
        .current_dir(&src)
        .args(["Bridge.java", "Main.java"])
        .output().expect("failed javac");
    assert!(compile.status.success(), "javac failed: {}", String::from_utf8_lossy(&compile.stderr));

    let run = Command::new(&java)
        .current_dir(&src)
        .args([format!("-Djava.library.path={}", base.to_string_lossy()), "Main".to_string()])
        .output().expect("failed java");
    assert!(run.status.success(), "java failed: {}", String::from_utf8_lossy(&run.stderr));
    assert!(String::from_utf8_lossy(&run.stdout).contains("JAVA OK"));
}
