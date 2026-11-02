// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H0d corpus audit: every parsed `.bv` in Tier-1 + stdlib lowers to verified
//! HIR; unsupported syntax is counted. The totals are a downward baseline for
//! H1 normalization.

use briev_compiler::hir::{lower_program, unsupported_counts, verify};
use std::path::{Path, PathBuf};

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");
const BASELINE_EXPRESSIONS: usize = 56;
const BASELINE_STATEMENTS: usize = 49;

fn bv_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            bv_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("bv") {
            out.push(path);
        }
    }
}

#[test]
fn corpus_lowers_with_verified_ids_and_bounded_unsupported_syntax() {
    let root = Path::new(PROJECT_ROOT);
    let mut files = Vec::new();
    bv_files(&root.join("tests/tier1"), &mut files);
    bv_files(&root.join("lib/std"), &mut files);
    files.sort();

    let mut expressions = 0usize;
    let mut statements = 0usize;
    let mut parsed = 0usize;
    let mut worst: Vec<(usize, String)> = Vec::new();
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(tokens) = briev_compiler::lexer::tokenize(&source) else {
            continue;
        };
        let mut parser = briev_compiler::parser::Parser::new(tokens, &source);
        let Ok(items) = parser.parse_program() else {
            continue;
        };
        parsed += 1;
        let program = lower_program(&items);
        verify(&program).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let counts = unsupported_counts(&program);
        expressions += counts.expressions;
        statements += counts.statements;
        worst.push((
            counts.total(),
            path.strip_prefix(root).unwrap_or(path).display().to_string(),
        ));
    }
    worst.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    println!(
        "HIR_UNSUPPORTED files={parsed} expressions={expressions} statements={statements} total={}",
        expressions + statements
    );
    for (count, path) in worst.iter().take(12) {
        println!("HIR_UNSUPPORTED_FILE {count:4} {path}");
    }
    assert!(
        expressions <= BASELINE_EXPRESSIONS,
        "unsupported expression count grew: {expressions} > {BASELINE_EXPRESSIONS}"
    );
    assert!(
        statements <= BASELINE_STATEMENTS,
        "unsupported statement count grew: {statements} > {BASELINE_STATEMENTS}"
    );
}
