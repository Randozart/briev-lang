// ===----------------------------------------------------------------------=== //
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
// ===----------------------------------------------------------------------=== //
//! The width-literal ratchet: a hardcoded target width may only get rarer.
//!
//! `i64` written into emitted IR is the most common defect in the LLVM backend —
//! 103 `inttoptr i64` sites against 65 places that already ask `ctx.int_bits` —
//! and it is *invisible* on x86-64, because there the guess is right. That is why
//! it survived three separate fixes (#119, #163, #164) and why a supported 32-bit
//! target found 12 remaining invalid programs in 2026-11-25.
//!
//! A ban would be wrong: a large share of the occurrences are legitimate, since
//! the C runtime's ABI is fixed-width on every triple (`long long` is 64 bits even
//! where the pointer is 32). So this is a counted budget, stored in
//! `config/width-literals.toml`, that only turns one way:
//!
//! * a file's count may go DOWN, and then the config must be lowered (a stale
//!   record is the thing this test refuses);
//! * a file's count may not go UP, and the failure names the file and the delta;
//! * a file that is not in the config has a budget of 0.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The budget file, relative to the repository root.
const CONFIG: &str = "config/width-literals.toml";

/// Files that ASSERT emitted text, so they must contain the literal they assert.
/// Matched by suffix: `tests.rs`, `*_tests.rs`, `crash_regressions.rs`.
const ASSERTION_SUFFIXES: [&str; 2] = ["tests.rs", "crash_regressions.rs"];

/// Repository root.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Parse `"path" = count` entries under `[files]`.
fn parse_config(text: &str) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let mut in_table = false;
    for (n, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('[') {
            in_table = trimmed == "[files]";
            continue;
        }
        if !in_table {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            panic!("{CONFIG}:{}: not a `\"file\" = count` entry", n + 1);
        };
        let file = key.trim().trim_matches('"').to_string();
        let count: u64 = value
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{CONFIG}:{}: `{}` is not a count", n + 1, value.trim()));
        out.insert(file, count);
    }
    out
}

/// Whether this line is documentation or a legitimately 64-bit use.
fn excluded(line: &str) -> bool {
    let code = line.trim_start();
    if code.starts_with("//") || code.is_empty() {
        return true;
    }
    // The C runtime's ABI is fixed-width on every triple: `long long` is 64 bits
    // even where the pointer is 32, so a call to it is not a target-width guess.
    if code.contains("briev_") || code.contains("__briev") {
        return true;
    }
    // `double` bit patterns and state-slot layout are genuinely 64-bit.
    if code.contains("double") || code.contains("f64") {
        return true;
    }
    false
}

/// Count target-width literals in one file, per the documented exclusions.
fn count_in(path: &Path) -> u64 {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut count = 0;
    for line in text.lines() {
        if excluded(line) {
            continue;
        }
        // `i64` as a standalone type token, inside the string literals that build
        // IR. Word-boundary matched so `i640` or an identifier cannot count.
        // `i32` is deliberately NOT counted: it is frequently the correct width for
        // a genuine 32-bit value (a Char, a Bit<32> field), so counting it would
        // make the budget noisy without measuring the thing that breaks targets.
        for needle in ["i64"] {
            let mut rest = line;
            while let Some(i) = rest.find(needle) {
                let before_ok = i == 0 || !is_ident(rest.as_bytes()[i - 1] as char);
                let after = rest[i + needle.len()..].chars().next();
                let after_ok = after.map(|c| !is_ident(c)).unwrap_or(true);
                if before_ok && after_ok {
                    count += 1;
                }
                rest = &rest[i + needle.len()..];
            }
        }
    }
    count
}

/// Identifier characters (so `word_i64` and `i64x` do not count).
fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every `.rs` file under `src/backend/llvm`, by path relative to that directory.
fn backend_files() -> Vec<(String, PathBuf)> {
    let root = repo_root().join("src/backend/llvm");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if ASSERTION_SUFFIXES
                .iter()
                .any(|suffix| name.ends_with(suffix))
            {
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            out.push((rel, path));
        }
    }
    out.sort();
    out
}

/// No file may exceed its declared budget.
#[test]
fn no_file_exceeds_its_width_literal_budget() {
    let config = parse_config(
        &std::fs::read_to_string(repo_root().join(CONFIG)).expect("read the budget file"),
    );
    let mut over = Vec::new();
    for (rel, path) in backend_files() {
        let budget = config.get(&rel).copied().unwrap_or(0);
        let actual = count_in(&path);
        if actual > budget {
            over.push(format!(
                "{rel}: {actual} (budget {budget}, +{})",
                actual - budget
            ));
        }
    }
    assert!(
        over.is_empty(),
        "hardcoded target-width literals went UP. A width that is meant to follow the \
         target must come from the value (`operand_width`), the target (`ctx.int_bits`), or \
         the casting graph — never a literal, because a literal is invisible on x86-64 and \
         wrong on every 32-bit target:\n  {}\n\
         If the new use is genuinely fixed-width (a C ABI symbol, a `double` image), put it \
         on a line this test excludes and say why in the comment.",
        over.join("\n  ")
    );
}

/// A budget that understates reality is a lie, so the record must be CURRENT.
///
/// This is the direction that keeps the ratchet turning: removing a literal
/// without lowering the number of the file you removed it from fails here.
#[test]
fn the_budget_matches_reality() {
    let config = parse_config(
        &std::fs::read_to_string(repo_root().join(CONFIG)).expect("read the budget file"),
    );
    let mut stale = Vec::new();
    for (rel, path) in backend_files() {
        let budget = config.get(&rel).copied().unwrap_or(0);
        let actual = count_in(&path);
        // Printed always: whoever has to recalibrate needs the table, not a diff.
        eprintln!("  \"{rel}\" = {actual}");
        if actual < budget {
            stale.push(format!("{rel}: declared {budget}, actual {actual}"));
        }
    }
    assert!(
        stale.is_empty(),
        "the budget is out of date — lower it, because the ratchet only turns one way:\n  {}\n\
         Edit {CONFIG}.",
        stale.join("\n  ")
    );
}

/// The budget covers the files that emit IR, not just the ones that exist.
#[test]
fn the_budget_names_the_files_that_emit() {
    let config = parse_config(
        &std::fs::read_to_string(repo_root().join(CONFIG)).expect("read the budget file"),
    );
    for required in [
        "emit_expr.rs",
        "emit_stmt.rs",
        "emit_toplevel.rs",
        "helpers.rs",
    ] {
        assert!(
            config.contains_key(required),
            "{required} emits IR and must have a budget; without an entry its budget is 0, \
             which would claim it has no literals at all"
        );
    }
}
