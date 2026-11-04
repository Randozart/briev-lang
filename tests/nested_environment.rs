//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Runtime environment expansion in loop bounds, guards and nested arguments.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

#[test]
fn nested_environment_macros_execute_inside_reactive_loops() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tempfile::tempdir().expect("temporary output directory");
    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .env("BRIEV_HOME", root)
        .arg("build").arg(root.join("tests/fixtures/load/nested_environment.bv"))
        .arg("--stdlib-path").arg(root.join("lib"))
        .arg("--config-dir").arg(root.join("config"))
        .arg("--out").arg(output.path())
        .output().expect("build environment fixture");
    assert!(build.status.success(), "{}\n{}",
        String::from_utf8_lossy(&build.stdout), String::from_utf8_lossy(&build.stderr));
    let result = Command::new("timeout").arg("20")
        .arg(output.path().join("nested_environment"))
        .env("BRIEV_ENV_READS", "3").env("BRIEV_ENV_ENABLED", "1")
        .env("BRIEV_ENV_KEY", "BRIEV_ENV_VALUE").env("BRIEV_ENV_VALUE", "7")
        .output().expect("run environment fixture");
    assert!(result.status.success(), "{:?}: {}", result.status,
        String::from_utf8_lossy(&result.stderr));
    assert_eq!(result.stdout, b"nested_env_ok\n");
}
