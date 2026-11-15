//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! A0: required gate controls must detect false success in both runner adapters.

#[test]
fn gate_controls_reject_false_success() {
    let output = std::process::Command::new("python3")
        .args(["-B", "scripts/tests/test_gate.py"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("gate controls require Python 3; install python3 and rerun the gate");
    assert!(
        output.status.success(),
        "gate controls failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
