//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Execute inferred tuple construction, returns, projections and destructuring.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

fn checked(command: &mut Command) -> std::process::Output {
    let output = command.output().expect("start tuple test command");
    assert!(output.status.success(), "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    output
}

#[test]
fn inferred_tuple_values_execute_at_both_optimization_levels() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    checked(Command::new(env!("CARGO_BIN_EXE_brievc")).current_dir(root)
        .env("BRIEV_HOME", root).arg("build").arg("tests/fixtures/load/tuple_values.bv")
        .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
        .arg(directory.path()));
    for level in ["-O0", "-O3"] {
        let binary = directory.path().join(format!("tuple{level}"));
        checked(Command::new("clang").arg(level).arg(directory.path().join("tuple_values.ll"))
            .arg(root.join("lib/runtime/briev_rt.c")).args(["-lm", "-ldl", "-o"]).arg(&binary));
        for seed in ["17", "999"] {
            let output = checked(Command::new("timeout").arg("20").arg(&binary).env("TUPLE_SEED", seed));
            assert_eq!(output.stdout, b"tuple_values_ok\n");
        }
    }
}

#[test]
fn nested_tuple_handles_verify_on_wasm32() {
    use briev_compiler::backend::llvm::LlvmBackend;
    let source = r#"
        obj Item { value: Int; };
        defn make(value: Int) -> (Item,Bool) { term (Item { value: value },true); };
        defn probe(value: Int) -> Int {
            let pair = make(value);
            let nested = (pair,value);
            let (inner,number) = nested;
            let (item,flag) = inner;
            when !flag || inner.0.value != value { term 0; };
            term item.value + number;
        };
        let done: Bool = false;
        node main [!done][done] { let answer = probe(17); done = true; endprogram; };
    "#;
    let (items, universe) = briev_compiler::library::parse_and_check("scratch/tuple_wasm.bv", source).unwrap();
    let ir = LlvmBackend::new().with_type_universe(universe).with_int_bits(32)
        .with_target_triple("wasm32-unknown-unknown").generate(&items, None);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tuple.ll");
    std::fs::write(&path, ir).unwrap();
    checked(Command::new("opt").args(["-passes=verify", "-disable-output"]).arg(&path));
    // Keep an opt-in artifact for the independent WebAssembly execution gate.
    if let Some(destination) = std::env::var_os("TUPLE_WASM_IR") {
        std::fs::copy(path, destination).unwrap();
    }
}
