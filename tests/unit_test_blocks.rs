// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-10 (SPEC §8.11): `test domain { expect …; }` end-to-end surface.
//! The reference interpreter is the v1 engine; blocks are typechecked but
//! never emitted.

use std::process::Command;

fn suite(files: &[(&str, &str)], extra: &[&str]) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    for (name, src) in files {
        std::fs::write(dir.path().join(name), src).unwrap();
    }
    let out = dir.path().join("out");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brievc"));
    cmd.arg("test").arg(dir.path()).arg("--out").arg(&out).arg("--tests-only");
    cmd.args(extra);
    cmd.env("BRIEV_HOME", env!("CARGO_MANIFEST_DIR"))
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    let output = cmd.output().expect("spawn brievc test");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

#[test]
fn passing_and_failing_blocks_report_and_exit() {
    let src = r#"
defn add(a: Int, b: Int) -> Int { term a + b; };
test arithmetic { expect add(1, 2) == 3; };
test wrong { expect add(1, 2) == 4; };
"#;
    let (ok, log) = suite(&[("math.bv", src)], &["--domain", "arithmetic"]);
    assert!(ok, "--domain filter selects only the passing test:\n{log}");
    assert!(log.contains("TEST arithmetic#1 PASS"), "{log}");
    assert!(log.contains("1/1 tests passed"), "{log}");

    let (ok, log) = suite(&[("math.bv", src)], &[]);
    assert!(!ok, "a failing expectation exits nonzero:\n{log}");
    assert!(log.contains("TEST arithmetic#1 PASS"), "{log}");
    assert!(log.contains("TEST wrong#1 FAIL"), "{log}");
    assert!(log.contains("expect (add(1, 2) == 4)"), "{log}");
    assert!(log.contains("1/2 tests passed"), "{log}");

    let (ok, log) = suite(&[("math.bv", src)], &["--domain", "missing"]);
    assert!(!ok, "an unmatched domain is an error:\n{log}");
    assert!(log.contains("no test blocks in domain 'missing'"), "{log}");
    assert!(log.contains("arithmetic, wrong"), "lists available domains:\n{log}");
}

#[test]
fn tests_are_isolated_and_globals_are_reseeded() {
    let src = r#"
let counter: Int = 0;
test one { counter = 5; expect counter == 5; };
test two { expect counter == 0; };
"#;
    let (ok, log) = suite(&[("iso.bv", src)], &[]);
    assert!(ok, "each test gets a fresh seed:\n{log}");
    assert!(log.contains("TEST one#1 PASS"), "{log}");
    assert!(log.contains("TEST two#1 PASS"), "{log}");
}

#[test]
fn list_tests_prints_domain_labels() {
    let src = "test alpha { expect true; };\ntest beta { expect true; };\n";
    let (ok, log) = suite(&[("labels.bv", src)], &["--list-tests"]);
    assert!(ok, "{log}");
    assert!(log.contains("alpha#1"), "{log}");
    assert!(log.contains("beta#1"), "{log}");
}

#[test]
fn expect_requires_bool_and_reactive_forms_are_rejected() {
    let (ok, log) = suite(&[("nonbool.bv", "test bad { expect 1; };")], &[]);
    assert!(!ok, "non-Bool expectation is a type error:\n{log}");
    assert!(log.contains("expected Bool for expect assertion"), "{log}");

    let (ok, log) = suite(&[("term.bv", "test bad { term 1; };")], &[]);
    assert!(!ok, "term inside a test is rejected:\n{log}");
    assert!(log.contains("`term` is not allowed inside a `test` block"), "{log}");
}

#[test]
fn test_blocks_do_not_change_emitted_ir() {
    let plain = r#"
let done: Bool = false;
defn add(a: Int, b: Int) -> Int { term a + b; };
node main [done == false][done] { Print#(add(1, 2)); done = true; term; };
"#;
    let with_test = format!(
        "{plain}test math {{ expect add(1, 2) == 3; }};\n"
    );
    let dir = tempfile::tempdir().unwrap();
    let mut lls = Vec::new();
    for (name, src) in [("plain", plain), ("withtest", &with_test)] {
        let src_path = dir.path().join(format!("{name}.bv"));
        std::fs::write(&src_path, src).unwrap();
        let out = dir.path().join(format!("out_{name}"));
        let status = Command::new(env!("CARGO_BIN_EXE_brievc"))
            .args(["build"])
            .arg(&src_path)
            .args(["--llvm", "--out"])
            .arg(&out)
            .env("BRIEV_HOME", env!("CARGO_MANIFEST_DIR"))
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("spawn build");
        assert!(
            status.status.success(),
            "build failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        lls.push(std::fs::read_to_string(out.join(format!("{name}.ll"))).unwrap());
    }
    assert_eq!(lls[0], lls[1], "test blocks must not change the emitted IR");
}

#[test]
fn derivation_operator_is_unchanged() {
    // `:=` still means compile-time derivation/reference implementation.
    let src = r#"
defn twice_ref(x: Int) -> Int { term x + x; };
defn twice(x: Int) -> Int := twice_ref;
test math { expect true; };
"#;
    let (ok, log) = suite(&[("derivation.bv", src)], &[]);
    assert!(ok, "the derivation suffix still parses/typechecks:\n{log}");
}
