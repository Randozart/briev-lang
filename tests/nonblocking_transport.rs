//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Controlled local peers exercise bounded nonblocking operations.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};
use std::os::unix::process::ExitStatusExt;

#[test]
fn transport_progress_connections_and_bounds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    let cases = [
        ("nonblocking_gate", "nonblocking_partial_progress_backpressure_eof_ok\n"),
        ("nonblocking_connect_gate", "nonblocking_connect_success_and_refusal_ok\n"),
        ("nonblocking_limits_gate", "nonblocking_valid_lifetime_ok\n"),
    ];
    for (name, expected) in cases {
        let build = Command::new(env!("CARGO_BIN_EXE_brievc")).current_dir(root)
            .env("BRIEV_HOME", root).arg("build").arg(format!("tests/fixtures/load/{name}.bv"))
            .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
            .arg(directory.path()).output().unwrap();
        assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
        let run = Command::new("timeout").arg("20").arg(directory.path().join(name))
            .env("CASE", "0").output().unwrap();
        assert!(run.status.success(), "{name}: {:?}", run.status);
        assert_eq!(run.stdout, expected.as_bytes());
    }
    for case in 1..=11 {
        let run = Command::new("timeout").arg("20").arg(directory.path().join("nonblocking_limits_gate"))
            .env("CASE", case.to_string()).output().unwrap();
        assert!(run.status.signal() == Some(4) || run.status.code() == Some(132), "case {case}: {:?}", run.status);
        assert!(run.stdout.is_empty());
    }
}
