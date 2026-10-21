//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//

use super::LlvmBackend;
use std::process::Command;

fn match_ir(source: &str, call: &str) -> String {
    // Root the definition through a node: an uncalled defn is correctly
    // removed by liveness and would never exercise match lowering.
    let source = format!("{source}\nlet tick: Int = 0; node run [tick < 1][tick == 1] {{ {call}; tick = 1; endprogram; }};");
    let (items, universe) = crate::library::parse_and_check("scratch/match_termination_fixture.bv", &source)
        .expect("fixture must typecheck through the production import/type pipeline");
    LlvmBackend::new().with_type_universe(universe).generate(&items, None)
}

fn verify_ir(ir: &str) {
    // Follow the backend's installed-toolchain convention: always assert the
    // emitted value merge, and additionally run LLVM's verifier when available.
    if Command::new("clang").arg("--version").output().is_err() { return; }
    let dir = tempfile::TempDir::new().expect("temporary IR directory");
    let input = dir.path().join("match.ll");
    std::fs::write(&input, ir).expect("write emitted IR");
    let result = Command::new("clang")
        .args(["-Wno-override-module", "-x", "ir", "-c"])
        .arg(&input).arg("-o").arg(dir.path().join("match.o"))
        .output().expect("run installed clang");
    assert!(result.status.success(), "LLVM rejected match IR: {}\n{ir}",
        String::from_utf8_lossy(&result.stderr));
}

#[test]
fn trapping_arm_preserves_integer_result_in_either_order() {
    for arms in ["true => 42, false => { trap; },", "true => { trap; }, false => 42,"] {
        let source = format!("defn choose(flag: Bool) -> Int {{ term match flag {{ {arms} }}; }};");
        let ir = match_ir(&source, "choose(false)");
        assert!(ir.contains("phi i64"), "continuing arm must supply the merge value:\n{ir}");
        assert!(ir.contains("call void @llvm.trap()"), "trap must remain observable:\n{ir}");
        verify_ir(&ir);
    }
}

#[test]
fn trapping_arm_preserves_pointer_result() {
    let ir = match_ir(r#"defn choose(flag: Bool) -> String {
        term match flag { true => { trap; }, false => "kept", };
    };"#, "choose(false)");
    assert!(ir.contains("phi ptr"), "string arm must retain its pointer merge:\n{ir}");
    verify_ir(&ir);
}

#[test]
fn nested_terminating_match_does_not_erase_outer_value() {
    let ir = match_ir("defn choose(flag: Bool, nested: Bool) -> Int {
        term match flag {
            true => match nested { true => { trap; }, false => { trap; }, },
            false => 42,
        };
    };", "choose(false, false)");
    assert!(ir.contains("phi i64"), "outer continuing arm must retain its value:\n{ir}");
    verify_ir(&ir);
}

#[test]
fn continuing_void_match_stays_statement_like() {
    let ir = match_ir("defn choose(flag: Bool) -> Int {
        match flag { true => {}, false => {}, };
        term 42;
    };", "choose(false)");
    assert!(!ir.contains("phi void"), "void match must not manufacture a value:\n{ir}");
    verify_ir(&ir);
}
