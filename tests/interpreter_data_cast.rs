//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Data retains its representation category across bindings and call boundaries.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{path::Path, process::Command};

#[test]
fn data_words_keep_their_category_and_ffi_bits() {
    use briev_compiler::ast::Type;
    use briev_compiler::interpreter::{Marshalled, Value, casts::eval_is_type, marshal_value};
    let word = Value::Data(u64::MAX - 41);
    assert_eq!(word.clone(), word);
    assert_ne!(word, Value::int(-42));
    assert_eq!(word.as_i64(), Some(-42));
    assert_eq!(marshal_value(&word), Marshalled::Int(-42));
    assert_eq!(
        eval_is_type(&word, &Type::Custom("Data".into()))
            .unwrap()
            .as_bool(),
        Some(true)
    );
    assert_eq!(
        eval_is_type(&word, &Type::int()).unwrap().as_bool(),
        Some(false)
    );
    assert_eq!(
        eval_is_type(&word, &Type::Custom("Unregistered".into()))
            .unwrap()
            .as_bool(),
        Some(false)
    );
}

fn checked(command: &mut Command) -> Vec<u8> {
    let result = command.output().expect("run Data cast regression");
    assert!(
        result.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

#[test]
fn data_casts_and_json_roundtrip_terminate_and_preserve_values() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tempfile::tempdir().unwrap();
    for fixture in ["data_cast_interpreter", "json_roundtrip_interpreter"] {
        let interpreted = checked(
            Command::new("timeout")
                .arg("120")
                .arg(env!("CARGO_BIN_EXE_brievc"))
                .current_dir(root)
                .env("BRIEV_HOME", root)
                .arg("test")
                .arg(format!("tests/fixtures/{fixture}.bv"))
                .arg("--tests-only")
                .arg("--out")
                .arg(output.path().join(fixture)),
        );
        assert!(String::from_utf8_lossy(&interpreted).contains("1/1 tests passed"));
    }
    for hir in ["0", "1"] {
        let destination = output.path().join(hir);
        checked(
            Command::new(env!("CARGO_BIN_EXE_brievc"))
                .current_dir(root)
                .env("BRIEV_HOME", root)
                .env("BRIEV_HIR_EMIT", hir)
                .args([
                    "build",
                    "tests/fixtures/data_cast_interpreter.bv",
                    "--stdlib-path",
                    "lib",
                    "--config-dir",
                    "config",
                    "--out",
                ])
                .arg(&destination),
        );
        assert_eq!(
            checked(
                Command::new("timeout")
                    .arg("20")
                    .arg(destination.join("data_cast_interpreter"))
            ),
            b"data_cast_ok\n"
        );
    }
}
