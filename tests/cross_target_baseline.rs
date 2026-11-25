// ===----------------------------------------------------------------------=== //
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
// ===----------------------------------------------------------------------=== //
//! The cross-target baseline record must stay true.
//!
//! `scripts/cross_target_ir.sh` compares each shipped triple's failure count
//! against `config/cross-target-baseline.toml`. The script enforces "no worse
//! than declared". This test enforces the other half of the ratchet — the record
//! may not claim more debt than actually exists — because a baseline that lags
//! behind reality is a number nobody can trust, and the next person to read it
//! will plan work that is already done.
//!
//! Two layers, because they cost very different amounts:
//!
//! * always: the file parses, every shipped triple has an entry, and a count is
//!   a plain non-negative integer. Cheap, runs everywhere.
//! * when `BRIEV_CROSS_TARGET_CHECK=1`: run the real sweep and require the
//!   measured count to EQUAL the declared one. That is ~4 minutes for two
//!   triples, so CI's cross-target job sets the variable rather than every
//!   local `cargo test`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Triples the project ships and therefore gates. wasm32 is the web build
/// (`*.rbv` → wasm32); aarch64 is the Linux/ABI sibling of the native triple.
const SHIPPED_TRIPLES: [&str; 2] = ["aarch64-unknown-linux-gnu", "wasm32-unknown-wasi"];

/// The baseline file, relative to the repository root.
const BASELINE_PATH: &str = "config/cross-target-baseline.toml";

/// Repository root: the test binary runs from the crate root.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Parse `[baseline]` entries as `triple -> count`.
///
/// Deliberately a hand parse, not a TOML dependency: the file has one table, two
/// keys, and this test must not be the reason the build gets another crate. A
/// line without `=` is prose (the rationale block at the top) and is skipped.
fn parse_baseline(text: &str) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let mut in_table = false;
    for (n, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('[') {
            in_table = trimmed == "[baseline]";
            continue;
        }
        if !in_table {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            panic!("{BASELINE_PATH}:{}: not a `triple = count` entry", n + 1);
        };
        let triple = key.trim().trim_matches('"').to_string();
        let count: u64 = value.trim().parse().unwrap_or_else(|_| {
            panic!(
                "{BASELINE_PATH}:{}: `{}` is not a count",
                n + 1,
                value.trim()
            )
        });
        assert!(
            out.insert(triple.clone(), count).is_none(),
            "{BASELINE_PATH}: `{triple}` is declared twice; the later entry would silently win"
        );
    }
    out
}

/// Read and parse the checked-in baseline.
fn declared_baseline() -> BTreeMap<String, u64> {
    let path = repo_root().join(BASELINE_PATH);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    parse_baseline(&text)
}

/// The compiler binary under test.
fn brievc() -> PathBuf {
    if let Ok(explicit) = std::env::var("BRIEVC") {
        return PathBuf::from(explicit);
    }
    for candidate in ["target/debug/brievc", "target/release/brievc"] {
        let path = repo_root().join(candidate);
        if path.is_file() {
            return path;
        }
    }
    panic!("no brievc binary found; build it first or set BRIEVC");
}

/// Run the real sweep for one triple and return how many programs failed.
fn measured_failures(triple: &str) -> u64 {
    let script = repo_root().join("scripts/cross_target_ir.sh");
    assert!(script.is_file(), "missing {}", script.display());
    let output = Command::new("bash")
        .arg(&script)
        .arg(triple)
        .env("BRIEVC", brievc())
        // A stale baseline makes the script exit non-zero, which is exactly the
        // signal this test was written to produce; the count is parsed out of
        // stdout either way.
        .output()
        .expect("failed to run scripts/cross_target_ir.sh");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // The summary line is `<triple> — N checked, M failed`.
    let line = text
        .lines()
        .rfind(|l| l.contains(triple) && l.contains("failed"))
        .unwrap_or_else(|| panic!("no summary line for {triple} in:\n{text}"));
    let failed = line
        .split("failed")
        .next()
        .and_then(|head| head.rsplit(|c: char| c == ',' || c == '—').next())
        .and_then(|count| count.trim().parse::<u64>().ok())
        .unwrap_or_else(|| panic!("cannot read the failure count out of: {line:?}"));
    failed
}

/// Classify one synthetic compiler log through the sweep's own rule.
fn classify(log_body: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "briev-classify-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, log_body).expect("write synthetic log");
    let output = Command::new("bash")
        .arg(repo_root().join("scripts/cross_target_ir.sh"))
        .arg("--classify")
        .arg(&path)
        .output()
        .expect("run the classifier");
    std::fs::remove_file(&path).ok();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A log carrying BOTH a liveness warning and an invalid-module error is DEBT.
///
/// This is the bug that put 12 in the baseline when the truth was 27. The compiler
/// prints `walked the live program ... no endprogram, halt, or trap is reachable` as
/// a WARNING and then emits anyway; a classifier that tests the refusal markers
/// first reads that warning as a refusal and stops counting a failure that is real.
#[test]
fn a_failure_with_both_markers_is_debt_not_refusal() {
    let log = concat!(
        "warning: no path ends this program\n",
        "  why:  walked the live program (reactive txns, entry nodes); no endprogram\n",
        "error: the compiler produced invalid IR for '/tmp/x.ll'\n",
        "  why:  Briev's emitted-module check rejected the module: line 4504\n",
    );
    assert_eq!(
        classify(log),
        "FAIL",
        "an invalid module is debt even when the log also carries the liveness warning"
    );
}

/// A log with only a refusal marker is a refusal — the program was not emitted, so
/// no invalid module is claimed.
#[test]
fn a_pure_refusal_is_not_counted_as_debt() {
    let log = "  why:  walked the live program (reactive txns); no endprogram, halt, or trap\n";
    assert_eq!(classify(log), "REFUSED");
}

/// An unrecognised failure is counted, never excused: a gate may under-count by
/// being strict, never by being charitable with something it cannot explain.
#[test]
fn the_classifier_never_excuses_an_unrecognised_failure() {
    let log = "something the classifier has never seen\n";
    assert_eq!(
        classify(log),
        "FAIL",
        "an unexplained failure must be counted, or an unexpected crash becomes free"
    );
}

/// Every shipped triple has a declared entry, and each is a plain count.
///
/// An absent triple means an implicit baseline of 0 in the script — fine for
/// a aspirational target, but a shipped one must state its debt or the gate is
/// guessing.
#[test]
fn every_shipped_triple_has_a_declared_baseline() {
    let declared = declared_baseline();
    for triple in SHIPPED_TRIPLES {
        assert!(
            declared.contains_key(triple),
            "{BASELINE_PATH} has no entry for the shipped triple `{triple}`; \
             add `\"{triple}\" = N` so the gate is not silently assuming 0"
        );
    }
}

/// The record may not outlive the fact: measured failures must EQUAL the
/// declared count, so a fix forces the file down.
///
/// Opt-in because the sweep compiles every corpus program twice per triple
/// (~4 minutes for two triples). CI's cross-target job sets
/// `BRIEV_CROSS_TARGET_CHECK=1`.
#[test]
fn wasm32_corpus_baseline_only_shrinks() {
    if std::env::var("BRIEV_CROSS_TARGET_CHECK").as_deref() != Ok("1") {
        eprintln!(
            "skipping: set BRIEV_CROSS_TARGET_CHECK=1 to run the sweep \
             (scripts/cross_target_ir.sh does this in CI)"
        );
        return;
    }
    let declared = declared_baseline();
    let mut stale = Vec::new();
    for triple in SHIPPED_TRIPLES {
        let expected = *declared.get(triple).unwrap_or(&0);
        let actual = measured_failures(triple);
        if actual != expected {
            stale.push(format!("{triple}: declared {expected}, measured {actual}"));
        }
    }
    assert!(
        stale.is_empty(),
        "the baseline record is out of date — the ratchet only turns one way:\n  {}\n\
         If the count went DOWN, lower `{BASELINE_PATH}` to the measured value.\n\
         If it went UP, a regression landed: fix the emission rather than raising the number.",
        stale.join("\n  ")
    );
}

/// The baseline must not do the gate's job for it: 0 is the only acceptable
/// declared count for a triple with no known debt, and the web target's count is
/// pinned so a silent raise shows up in review.
#[test]
fn the_baseline_is_not_raised_silently() {
    let declared = declared_baseline();
    assert_eq!(
        declared.get("aarch64-unknown-linux-gnu").copied(),
        Some(0),
        "aarch64 had zero invalid programs; any non-zero count is a regression, not a baseline"
    );
    let wasm = declared.get("wasm32-unknown-wasi").copied().unwrap_or(0);
    assert!(
        wasm <= 27,
        "wasm32's declared baseline went up to {wasm} from 27. Raising it hides a regression: \
         fix the emission, or record the bug in BUGS.md and justify the move in this file's header"
    );
}

/// The corpus the sweep walks must actually contain the shipped web path.
///
/// This is the invariant that was missing: before 2026-11-25 the sweep read only
/// `tests/tier1/*.bv` and `tests/tier2/*.bv`, so **no `.rbv` program was ever
/// cross-target verified** while `.rbv` was the web product.
#[test]
fn the_sweep_corpus_includes_the_web_path() {
    let script = std::fs::read_to_string(repo_root().join("scripts/cross_target_ir.sh"))
        .expect("read scripts/cross_target_ir.sh");
    assert!(
        script.contains("*.rbv"),
        "scripts/cross_target_ir.sh no longer selects `*.rbv` programs: the web path would be \
         ungated again. That is the exact gap this test exists to prevent."
    );
    let rbv_count = walk(&repo_root())
        .iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("rbv"))
        .count();
    assert!(
        rbv_count >= 29,
        "expected the `.rbv` corpus to hold at least the 29 programs that existed when the web \
         path was gated; found {rbv_count}. A shrinking corpus is a quiet way to lose coverage."
    );
}

/// Every `.rbv` file under `examples/` and `tests/`, excluding archives.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("examples"), root.join("tests")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) != Some("archive") {
                    stack.push(path);
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("rbv") {
                out.push(path);
            }
        }
    }
    out
}
