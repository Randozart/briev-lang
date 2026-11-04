// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.3a parity: HIR `classify_plain_lets` must equal the AST analysis (still
//! authoritative) for declaration, proof surface, body writes, and scalar
//! type/init rules.

use briev_compiler::analysis::storage_class::classify_plain_lets as ast_plain;
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::lower_program;
use briev_compiler::hir::storage_class::classify_plain_lets as hir_plain;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn assert_parity(src: &str, label: &str) {
    let items = parse(src);
    let empty = HashSet::new();
    let ast = ast_plain(&items, None, &empty);
    let hir = hir_plain(&lower_program(&items), None, &empty);
    assert_eq!(ast, hir, "{label}: plain-set parity");
}

#[test]
fn curated_storage_class_parity() {
    // Bare, unreferenced scalar let infers plain.
    assert_parity("let a: Int = 0;", "bare");
    // Referenced by a txn contract pre-condition → machine state.
    assert_parity("let b: Int = 0;\nnode n [b > 0][true] { term; };", "contract");
    // Observed by `endprogram <expr>` → surface.
    assert_parity("let c: Int = 0;\nnode n [true][true] { endprogram c; };", "endprogram");
    // Written by a txn body → machine state.
    assert_parity("let d: Int = 0;\nnode n [true][true] { d = d + 1; term; };", "written");
    // Declared `plain` always.
    assert_parity("plain let e: Int = 0;", "declared-plain");
    // Declared `state` never plain.
    assert_parity("state let f: Int = 0;", "declared-state");
    // Non-scalar initializer (collection literal) stays state.
    assert_parity("let g: List<Int> = [1];", "non-scalar-init");
    // Non-scalar declared type stays state.
    assert_parity("let h: Widget = 0;", "non-scalar-type");
}

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
fn corpus_storage_class_parity() {
    let root = Path::new(PROJECT_ROOT);
    let mut files = Vec::new();
    bv_files(&root.join("tests/tier1"), &mut files);
    bv_files(&root.join("lib/std"), &mut files);
    files.sort();

    let empty = HashSet::new();
    let mut compared = 0usize;
    let mut skipped = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(items) = std::panic::catch_unwind(|| parse(&source)) else {
            continue;
        };
        let program = lower_program(&items);
        if briev_compiler::hir::unsupported_counts(&program).total() != 0 {
            skipped += 1;
            continue;
        }
        compared += 1;
        let ast = ast_plain(&items, None, &empty);
        let hir = hir_plain(&program, None, &empty);
        if ast != hir {
            let rel = path.strip_prefix(root).unwrap_or(path).display();
            let mut diff: Vec<String> = ast
                .symmetric_difference(&hir)
                .cloned()
                .collect();
            diff.sort();
            mismatches.push(format!("{rel}: {diff:?}"));
        }
    }

    println!(
        "HIR_STORAGE_CLASS_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "HIR storage-class parity mismatches:\n{}",
        mismatches.join("\n")
    );
}
