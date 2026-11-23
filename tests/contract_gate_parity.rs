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

/// Read the config's `exempt:` row as the list of roots it names.
fn exempt_roots() -> Vec<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = fs::read_to_string(root.join("config/contracts.dbvl")).expect("read contracts.dbvl");
    config
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("exempt:"))
        .flat_map(|rest| rest.trim_end_matches(';').split(','))
        .map(|root| root.trim().trim_matches('"').to_string())
        .filter(|root| !root.is_empty())
        .collect()
}

#[test]
fn exemptions_name_real_directories_and_never_acceptance_roots() {
    // An exemption is allowed only when the root is real and is NOT acceptance
    // code: the stdlib, the examples, and the tier corpora stay under `deny`.
    // This is the mechanical half of "no silent exemption" — the other half is
    // the announcement tested below.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = exempt_roots();
    for exempt in &roots {
        assert!(
            root.join(exempt).is_dir(),
            "config/contracts.dbvl exempts `{exempt}`, which is not a directory"
        );
        for acceptance in ["lib", "examples", "src", "tests/tier1", "tests/tier2"] {
            assert!(
                exempt != acceptance && !exempt.starts_with(&format!("{acceptance}/")),
                "`{exempt}` is acceptance code and must not be exempt from the gate"
            );
        }
    }
}

#[test]
fn an_exempt_root_announces_itself() {
    // An exemption that runs silently is indistinguishable from a gate that is
    // not installed. Every skipped file says so.
    let roots = exempt_roots();
    let Some(exempt) = roots.first() else {
        return; // no exemptions configured; nothing to announce
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(source) = first_bv_under(&root.join(exempt)) else {
        return; // the exempt root holds no fixture; nothing to announce
    };
    let dir = scratch("exempt_announce");
    let output = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .current_dir(root)
        .env("BRIEV_HOME", root)
        .arg("build")
        .arg(&source)
        .args(["--stdlib-path", "lib", "--config-dir", "config", "--llvm", "--out"])
        .arg(&dir)
        .output()
        .expect("start brievc");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("the contract gate is not enforced for"),
        "`{}` is under the exempt root `{exempt}` but the run said nothing:\n{stderr}",
        source.display()
    );
}

/// The first `*.bv` under `dir`, depth-first.
fn first_bv_under(dir: &std::path::Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for entry in &entries {
        if entry.is_dir() {
            if let Some(found) = first_bv_under(entry) {
                return Some(found);
            }
        } else if entry.extension().is_some_and(|ext| ext == "bv") {
            return Some(entry.clone());
        }
    }
    None
}

#[test]
fn node_term_is_a_checkpoint_not_a_return() {
    // A node's bare `term;` is a convergence checkpoint: it may continue, so it
    // establishes nothing for the code after it. The same guard in a defn DOES
    // license its negation, because a defn's bare term exits when the post is
    // vacuous (`lower_bare_terms` folds a `true` post into a return).
    //
    // Checked through the CLI rather than a unit test: the licence is a
    // typechecker-context decision, and the CLI is where the policy, the
    // context and the call gate are all installed together.
    let src = r#"
import "std/io.bv";

defn needs(x: Int) [x >= 1][term >= 0] -> Int { term x; };

node n [beginprogram][true] {
    let y: Int = get_env_int!("N");
    when y < 1 { term; };
    Print#(needs(y) as String);
    endprogram Print#("");
};
"#;
    let dir = scratch("node_term");
    let source = dir.join("node_term.bv");
    fs::write(&source, src).expect("write fixture");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .current_dir(root)
        .env("BRIEV_HOME", root)
        .arg("check")
        .arg(&source)
        .args(["--stdlib-path", "lib", "--config-dir", "config"])
        .output()
        .expect("start brievc");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("not provable"),
        "a node's bare term must not license `y >= 1`:\n{stderr}"
    );
}

#[test]
fn node_bare_term_is_a_checkpoint_not_a_return() {
    // The wave-10 boundary: `endprogram`/`trap` are unconditional, so their
    // guards refine. A bare `term;` in a node is a CONVERGENCE CHECKPOINT —
    // it may continue — so it licenses nothing. (The same guard in a defn
    // with a vacuous post DOES license, via `lower_bare_terms`.)
    let src = r#"
import "std/io.bv";

defn needs(x: Int) [x >= 1][term >= 0] -> Int { term x; };

node n [beginprogram][true] {
    let y: Int = get_env_int!("N");
    when y < 1 { term; };
    Print#(needs(y) as String);
    endprogram Print#("");
};
"#;
    let dir = scratch("node_bare_term");
    let source = dir.join("node_bare_term.bv");
    fs::write(&source, src).expect("write fixture");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .current_dir(root)
        .env("BRIEV_HOME", root)
        .arg("check")
        .arg(&source)
        .args(["--stdlib-path", "lib", "--config-dir", "config"])
        .output()
        .expect("start brievc");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("not provable"),
        "a node's bare term must not license `y >= 1`:\n{stderr}"
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
