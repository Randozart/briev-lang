// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4a parity: HIR tail-recursion / tail-loop plans must equal the AST
//! analysis (still authoritative).

use briev_compiler::analysis::tail_recursion::{
    build_tail_loop_plan as ast_loop, build_tail_recursion_plan as ast_tail,
};
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::lower_program;
use briev_compiler::hir::tail_recursion::{build_tail_loop_plan as hir_loop, build_tail_recursion_plan as hir_tail};
use std::path::{Path, PathBuf};

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn assert_parity(src: &str, label: &str) {
    let items = parse(src);
    let program = lower_program(&items);
    assert_eq!(
        ast_tail(&items).defns,
        hir_tail(&program).defns,
        "{label}: tail-recursion classes"
    );
    assert_eq!(
        ast_loop(&items).defns,
        hir_loop(&program).defns,
        "{label}: tail-loop set"
    );
}

#[test]
fn curated_tail_recursion_parity() {
    assert_parity(
        "defn f(n: Int) -> Int { when n <= 0 { term 0; }; term f(n - 1); };",
        "direct-tail",
    );
    assert_parity(
        "enum R { Ok(Int), Err(Int) };\n\
         defn f(n: Int) -> Int { when n <= 0 { term 0; }; let r: R = Ok(n); term match r { Ok(x) => f(x - 1) + 1, Err(_) => f(n - 1) }; };",
        "non-tail",
    );
    assert_parity(
        "defn a(n: Int) -> Int { term b(n); };\ndefn b(n: Int) -> Int { term a(n); };\ndefn c(n: Int) -> Int { term n; };",
        "mutual",
    );
    // A tail-recursive defn with a defer is not loop-lowerable.
    assert_parity(
        "defn g(n: Int) -> Int { defer { let x: Int = 1; }; when n <= 0 { term 0; }; term g(n - 1); };",
        "cleanup",
    );
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
fn corpus_tail_recursion_parity() {
    let root = Path::new(PROJECT_ROOT);
    let mut files = Vec::new();
    bv_files(&root.join("tests/tier1"), &mut files);
    bv_files(&root.join("lib/std"), &mut files);
    files.sort();

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
        let rel = path.strip_prefix(root).unwrap_or(path).display();
        if ast_tail(&items).defns != hir_tail(&program).defns {
            mismatches.push(format!("{rel}: tail-recursion classes"));
        }
        if ast_loop(&items).defns != hir_loop(&program).defns {
            mismatches.push(format!("{rel}: tail-loop set"));
        }
    }

    println!(
        "HIR_TAIL_RECURSION_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "HIR tail-recursion parity mismatches:\n{}",
        mismatches.join("\n")
    );
}
