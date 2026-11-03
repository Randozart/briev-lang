//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use std::path::Path;
use std::process::Command;

#[test]
fn optimized_enum_result_units_match_independent_os_queries() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return; }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tempfile::tempdir().expect("fixture directory");
    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .env("BRIEV_HOME", root)
        .arg("build").arg(root.join("tests/fixtures/process_units_probe.bv"))
        .arg("--stdlib-path").arg(root.join("lib"))
        .arg("--config-dir").arg(root.join("config"))
        .arg("--out").arg(output.path())
        .output().expect("build optimized units gate");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let mut expected = Vec::new();
    for key in ["PAGESIZE", "CLK_TCK"] {
        let query = Command::new("getconf").arg(key).output().expect("independent OS query");
        assert!(query.status.success());
        expected.extend(query.stdout);
    }
    let result = Command::new("timeout").arg("20")
        .arg(output.path().join("process_units_probe")).output().expect("execute units gate");
    assert!(result.status.success(), "{:?}", result.status);
    assert_eq!(result.stdout, expected, "returned enum image must survive optimized calls");
}
