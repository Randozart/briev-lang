//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! A folded transaction's successful term must publish state before returning.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn checked(command: &mut Command) -> Output {
    let output = command
        .output()
        .expect("start counted-term regression command");
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn compile(fixture: &str, shape: &str, directory: &Path) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    checked(
        Command::new(env!("CARGO_BIN_EXE_brievc"))
            .current_dir(root)
            .env("BRIEV_HOME", root)
            .env("BRIEV_DIAG_EFFECT", shape)
            .args([
                "build",
                fixture,
                "--llvm",
                "--stdlib-path",
                "lib",
                "--config-dir",
                "config",
                "--out",
            ])
            .arg(directory),
    );
    directory
        .join(Path::new(fixture).file_stem().unwrap())
        .with_extension("ll")
}

fn link(ir: &Path, level: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = ir.with_extension(level);
    checked(
        Command::new("clang")
            .arg(level)
            .arg(ir)
            .arg(root.join("lib/runtime/briev_rt.c"))
            .args(["-lm", "-ldl", "-o"])
            .arg(&binary),
    );
    binary
}

fn run(binary: &Path, bound: &str) -> Vec<u8> {
    checked(
        Command::new("timeout")
            .args(["--kill-after=2s", "10s"])
            .arg(binary)
            .env("BOUND", bound),
    )
    .stdout
}

fn check_bounds(binary: &Path, cases: &[(&str, &[u8])]) {
    for (bound, expected) in cases {
        assert_eq!(run(binary, bound), *expected, "BOUND={bound}");
    }
}

fn check_shape(fixture: &str, shape: &str, cases: &[(&str, &[u8])]) {
    let directory = tempfile::tempdir().unwrap();
    let ir = compile(fixture, shape, directory.path());
    for level in ["-O0", "-O3"] {
        check_bounds(&link(&ir, level), cases);
    }
}

#[test]
fn mixed_effect_shapes_terminate_and_print_exact_counters() {
    for shape in ["auto", "stateless"] {
        check_shape(
            "tests/tier1/mixed_effect_shape.bv",
            shape,
            &[
                ("0", b""),
                ("1", b""),
                ("1000000", b"1000000\n1000000\n"),
                ("1000001", b"1000000\n1000000\n"),
            ],
        );
    }
}

#[test]
fn conditional_term_publishes_all_carriers_and_skips_only_successful_tail() {
    for shape in ["escaped", "stateless"] {
        check_shape(
            "tests/fixtures/load/counted_term_publication.bv",
            shape,
            &[
                ("0", b"0\n10\n1\n"),
                ("1", b"1\n14\n1\n"),
                ("4", b"4\n32\n3\n"),
            ],
        );
    }
}
