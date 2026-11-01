// D7h: the Node bridge must export, compile, load, and call Briev exports.
use std::path::Path;
use std::process::Command;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn node_export_boundary_runs() {
    if !has("node") || !has("llc") {
        eprintln!("SKIP: node/llc not available");
        return;
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out = Path::new(ROOT).join("target/briev_js_export_boundary");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let bridge = Path::new(ROOT).join("examples/glue-host/bench.bv");
    for mode in ["export", "extension"] {
        let build = Command::new(brievc)
            .args([
                mode,
                &bridge.to_string_lossy(),
                "node",
                "--out",
                &out.to_string_lossy(),
            ])
            .output()
            .unwrap_or_else(|e| panic!("spawn brievc {mode} node: {e}"));
        assert!(
            build.status.success(),
            "{mode} node failed:\n{}",
            String::from_utf8_lossy(&build.stderr)
        );
    }
    let gate = Command::new("node")
        .arg(Path::new(ROOT).join("benchmarks/bridge/gate/gate.js"))
        .arg(out.join("bench.node"))
        .arg("42")
        .output()
        .expect("spawn node gate");
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(gate.status.success(), "node gate failed:\n{}", String::from_utf8_lossy(&gate.stderr));
    assert!(stdout.contains("BRIEV_FH") && stdout.contains("BRIEV_ADD"), "node gate output missing: {stdout}");
}
