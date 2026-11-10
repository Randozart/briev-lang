// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-10: the trg dependency graph must assign topo order / bit indices
//! deterministically across processes (HashSet iteration is process-seeded).

use std::process::Command;

const SRC: &str = r#"
trg alpha @ 0x1000;
trg zeta @ 0x1004;
let result: Int = 0;
node main [result == 0][result == 1] { result = alpha; term; };
"#;

#[test]
fn trigger_bit_assignment_is_stable_across_processes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("trg.bv");
    std::fs::write(&src, SRC).unwrap();
    let mut outputs = Vec::new();
    for run in 0..4 {
        let out = dir.path().join(format!("out{run}"));
        let status = Command::new(env!("CARGO_BIN_EXE_brievc"))
            .args(["build"])
            .arg(&src)
            .args(["--llvm", "--out"])
            .arg(&out)
            .env("BRIEV_HOME", env!("CARGO_MANIFEST_DIR"))
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("spawn brievc");
        assert!(
            status.status.success(),
            "build failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        outputs.push(std::fs::read_to_string(out.join("trg.ll")).unwrap());
    }
    for (i, text) in outputs.iter().enumerate().skip(1) {
        assert_eq!(*text, outputs[0], "run {i} differs from run 0");
    }
}
