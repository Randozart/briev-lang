// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.5 parity: HIR task-handle linearity errors must equal the AST errors.

use briev_compiler::analysis::task_linear::analyze as ast_analyze;
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::lower_program;
use briev_compiler::hir::task_linear::analyze as hir_analyze;
use std::path::{Path, PathBuf};

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn bv_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            bv_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("bv") {
            out.push(path);
        }
    }
}

fn compare(src: &str) -> Vec<String> {
    let items = parse(src);
    let program = lower_program(&items);
    let ast = ast_analyze(&items);
    let hir = hir_analyze(&program);
    if ast == hir {
        Vec::new()
    } else {
        vec![format!("ast={ast:?} hir={hir:?}")]
    }
}

#[test]
fn curated_task_linear_parity() {
    let src = "defn job(n: Int) -> Int { term n; };\n\
               let t = spawn job(5);\n\
               let a = await t;\n\
               let b = await t;";
    let bad = compare(src);
    assert!(bad.is_empty(), "HIR task-linear parity:\n{}", bad.join("\n"));
}

#[test]
fn corpus_task_linear_parity() {
    let root = Path::new(PROJECT_ROOT);
    let mut files = Vec::new();
    bv_files(&root.join("benchmarks"), &mut files);
    bv_files(&root.join("tests/tier1"), &mut files);
    bv_files(&root.join("lib/std"), &mut files);
    files.sort();
    let mut compared = 0usize;
    let mut skipped = 0usize;
    let mut mismatches = Vec::new();
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else { continue };
        let Ok(items) = std::panic::catch_unwind(|| parse(&source)) else { continue };
        let program = lower_program(&items);
        if briev_compiler::hir::unsupported_counts(&program).total() != 0 {
            skipped += 1;
            continue;
        }
        compared += 1;
        let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
        for b in compare(&source) {
            mismatches.push(format!("{rel}: {b}"));
        }
    }
    println!(
        "HIR_TASK_LINEAR_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(mismatches.is_empty(), "HIR task-linear mismatches:\n{}", mismatches.join("\n"));
}
