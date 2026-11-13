// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Saltpetre library conformance gate: the tiny ONNX corpus runs through the
//! Briev executor via the session API and must match the ORT-generated
//! goldens bit-for-bit. Mirrors the kiosk differential protocol but stays
//! in-repo and fast (no self-hosted runner, no real models).
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURES: &[&str] = &["relu", "sign", "mlp", "conv", "conv_ext", "tail"];

fn checked(command: &mut Command) -> String {
    let output = command.output().expect("start saltpetre conformance step");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn build_conformance(root: &Path, out: &Path) -> PathBuf {
    checked(Command::new(env!("CARGO_BIN_EXE_brievc"))
        .current_dir(root)
        .env("BRIEV_HOME", root)
        .args(["build", "tests/saltpetre/conformance.bv",
            "--stdlib-path", "lib", "--config-dir", "config", "--out"])
        .arg(out));
    out.join("conformance")
}

#[test]
fn saltpetre_tiny_corpus_matches_golden_bits() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures = root.join("tests/fixtures/saltpetre");
    let directory = tempfile::tempdir().unwrap();
    let exe = build_conformance(root, directory.path());

    for name in FIXTURES {
        let golden_path = fixtures.join(format!("{name}.golden.json"));
        let golden: Value =
            serde_json::from_str(&std::fs::read_to_string(&golden_path).unwrap()).unwrap();
        let input: Vec<f64> = golden["input"]
            .as_array().expect("golden input array")
            .iter().map(|v| v.as_f64().unwrap()).collect();
        let expected: Vec<i64> = golden["output"]
            .as_array().expect("golden output array")
            .iter().map(|v| {
                let bits = (v.as_f64().unwrap() as f32).to_bits();
                (bits as i32) as i64
            }).collect();

        let input_file = directory.path().join(format!("{name}.input.txt"));
        let text: Vec<String> = input.iter().map(|v| format!("{v}")).collect();
        std::fs::write(&input_file, text.join("\n")).unwrap();

        let stdout = checked(Command::new(&exe)
            .env("ONNX_MODEL", fixtures.join(format!("{name}.onnx")))
            .env("ONNX_INPUT", &input_file)
            .env("ONNX_OUTPUT", "0"));
        let bits: Vec<i64> = stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.trim().parse::<i64>()
                .unwrap_or_else(|_| panic!("{name}: non-bit line {l:?}")))
            .collect();
        assert_eq!(bits.len(), expected.len(), "{name}: output count");
        // Tolerant parity, kiosk policy (atol/rtol 1e-3): conv-style
        // accumulation order differs from ORT by a few ulps.
        const ATOL: f64 = 1e-3;
        const RTOL: f64 = 1e-3;
        for (i, (got, want)) in bits.iter().zip(expected.iter()).enumerate() {
            let got_f = f32::from_bits((*got as u32) as i32 as u32) as f64;
            let want_f = f32::from_bits((*want as i32) as u32) as f64;
            let delta = (got_f - want_f).abs();
            assert!(
                delta <= ATOL + RTOL * want_f.abs(),
                "{name}: output[{i}] {got_f} vs golden {want_f} (delta {delta})"
            );
        }
    }
}
