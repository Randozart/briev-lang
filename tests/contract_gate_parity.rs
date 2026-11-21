//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! One contract policy governs every acceptance entry (issue #149).
//!
//! `policy: deny` used to be enforced ONLY inside `cargo test`: the gate was
//! installed by `check_source_for` (the conformance sweep) and by nothing else,
//! so `brievc check`, `build`, `run`, `test` and `library` compiled everything
//! under `Allow`. A library author could pass every local command and fail CI
//! twenty-one minutes later on an unprovable call site.
//!
//! These tests take a program whose call site is genuinely unprovable and
//! assert that each entry rejects it, with the diagnostic naming the reason.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// A program whose only defect is a call whose precondition cannot be proven:
/// `need` demands `x >= 1`, and `n` comes from the environment.
const UNPROVABLE: &str = r#"
import "std/io.bv";

defn need(x: Int) [x >= 1][term >= 0] -> Int {
    term x;
};

node entry [beginprogram][true] {
    let n: Int = get_env_int!("N");
    Print#(need(n) as String);
    endprogram Print#("");
};
"#;

fn scratch(tag: &str) -> PathBuf {
    let unique = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "briev_contract_gate_{}_{}_{}",
        std::process::id(),
        unique,
        tag
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Run one CLI entry against the unprovable program and return its stderr.
fn run_entry(tag: &str, args: &[&str]) -> (bool, String) {
    let dir = scratch(tag);
    let source = dir.join("unprovable.bv");
    fs::write(&source, UNPROVABLE).expect("write fixture");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_brievc"));
    command.current_dir(root).env("BRIEV_HOME", root);
    command.args(args);
    command.arg(source.to_str().expect("utf-8 path")).args([
        "--stdlib-path",
        "lib",
        "--config-dir",
        "config",
    ]);
    if tag != "check" && tag != "check_validate" {
        command.args(["--out"]).arg(&dir);
    }
    let output = command.output().expect("start brievc");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn contract_gate_parity_across_entries() {
    // Every acceptance entry must reject the same program for the same reason.
    let entries: [(&str, &[&str]); 3] = [
        ("check", &["check"]),
        ("check_validate", &["check", "--validate"]),
        ("build", &["build"]),
    ];
    for (tag, args) in entries {
        let (ok, stderr) = run_entry(tag, args);
        assert!(
            !ok,
            "`brievc {tag}` accepted a program whose call site cannot be proven — \
             the contract policy is not installed on this entry"
        );
        assert!(
            stderr.contains("is not provable at this call site"),
            "`brievc {tag}` rejected the program but did not name the unprovable \
             call site; stderr was:\n{stderr}"
        );
    }
}

#[test]
fn contract_gate_does_not_exempt_the_fixture_roots() {
    // An exemption is allowed only when a fixture exists to test the ABSENCE
    // of a proof, and it must be visible in the config. Today there is none:
    // the config carries the policy and nothing else. If a root list is ever
    // added, this test must be updated deliberately rather than silently.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let config =
        fs::read_to_string(root.join("config/contracts.dbvl")).expect("read contracts.dbvl");
    let policy_lines: Vec<&str> = config
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .collect();
    assert!(
        policy_lines.iter().all(|l| l.starts_with("policy")),
        "config/contracts.dbvl grew a key beyond `policy` — if it is a per-root \
         exemption, the tests above no longer prove parity across roots. Keys: {policy_lines:?}"
    );
}

#[test]
fn config_deny_is_what_the_cli_reads() {
    // The gate's stage comes from the shipped config; if someone flips it to
    // `allow`, every test above would pass for the wrong reason. Pin the file's
    // contents so the flip is a visible change with a failing test.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let config =
        fs::read_to_string(root.join("config/contracts.dbvl")).expect("read contracts.dbvl");
    assert!(
        config.contains("deny"),
        "config/contracts.dbvl no longer selects the `deny` policy — the contract \
         tests would then prove nothing about enforcement:\n{config}"
    );
}
