//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use std::path::Path;
use std::process::{Command, Output};

fn run_program(source: &str) -> Output {
    let directory = tempfile::tempdir().expect("temporary native fixture");
    let input = directory.path().join("bounds.bv");
    std::fs::write(&input, source).expect("write fixture");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let built = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .env("BRIEV_HOME", root)
        .args(["build", input.to_str().unwrap(), "--stdlib-path"])
        .arg(root.join("lib"))
        .arg("--config-dir").arg(root.join("config"))
        .arg("--out").arg(directory.path())
        .output().expect("compile fixture");
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    Command::new(directory.path().join("bounds")).output().expect("execute fixture")
}

#[test]
fn indexed_collection_bounds_and_completion() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return; }
    for index in [-1, 0, 1, 2] {
        let source = format!(r#"
            let done: Bool = false;
            node main [!done][done] {{
                let values: List<Int> = [3,5];
                values[{index}] = 9;
                when values[{index}] != 9 {{ endprogram 7; }};
                println!("completed");
                done = true;
                endprogram;
            }};
        "#);
        let outcome = run_program(&source);
        if (0..2).contains(&index) {
            assert!(outcome.status.success(), "index {index}: {:?}", outcome.status);
            assert_eq!(outcome.stdout, b"completed\n");
        } else {
            assert!(!outcome.status.success(), "invalid index {index} was accepted");
            assert!(outcome.stdout.is_empty(), "invalid index reached completion");
        }
    }
}

#[test]
fn collection_storage_and_member_boundaries_execute() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return; }
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/load");
    for name in ["collection_index_store_gate", "fixed_collection_elements_gate",
                 "index_effects_gate", "inline_member_term_gate"] {
        let source = std::fs::read_to_string(fixtures.join(format!("{name}.bv"))).unwrap();
        let expected = std::fs::read(fixtures.join(format!("{name}.golden"))).unwrap();
        let result = run_program(&source);
        assert!(result.status.success(), "{name}: {:?}", result.status);
        assert_eq!(result.stdout, expected, "{name}");
    }
}
