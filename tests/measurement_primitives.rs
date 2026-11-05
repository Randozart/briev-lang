//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Checked time, process identity, host units and bounded distributions.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{fs, path::Path, process::Command};

#[test]
fn measurement_primitives_execute_native_gates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in ["clock_gate", "proc_stat_gate", "auxv_gate", "histogram_gate"] {
        let output = tempfile::tempdir().unwrap();
        let source = root.join(format!("tests/fixtures/load/{name}.bv"));
        let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
            .env("BRIEV_HOME", root).current_dir(root)
            .arg("build").arg(&source).arg("--stdlib-path").arg(root.join("lib"))
            .arg("--config-dir").arg(root.join("config"))
            .arg("--out").arg(output.path()).output().unwrap();
        assert!(build.status.success(), "{name}: {}", String::from_utf8_lossy(&build.stderr));
        let run = Command::new("timeout").arg("30")
            .arg(output.path().join(name)).output().unwrap();
        assert!(run.status.success(), "{name}: {:?}", run.status);
        let expected = fs::read_to_string(source.with_extension("golden")).unwrap().replace("\r\n", "\n");
        assert_eq!(String::from_utf8_lossy(&run.stdout), expected, "{name}");
    }
}
