//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Whole-file reads release descriptors after both EOF and read errors.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{fs, path::Path, process::Command};

#[test]
fn whole_file_reads_preserve_descriptor_availability() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tempfile::tempdir().expect("test directory");
    fs::write(output.path().join("growth"), vec![b'x'; 20000]).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .env("BRIEV_HOME", root).current_dir(root)
        .arg("build").arg(root.join("tests/fixtures/load/file_descriptor_lifetime.bv"))
        .arg("--stdlib-path").arg(root.join("lib"))
        .arg("--config-dir").arg(root.join("config"))
        .arg("--out").arg(output.path()).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new("timeout").arg("30")
        .arg(output.path().join("file_descriptor_lifetime"))
        .current_dir(output.path()).output().unwrap();
    assert!(run.status.success(), "{:?}: {}", run.status, String::from_utf8_lossy(&run.stderr));
    assert_eq!(run.stdout, b"file_descriptors_stable\n");
}
