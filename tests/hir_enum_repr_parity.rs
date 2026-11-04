// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b parity: HIR enum representation plan must equal the AST plan.

use briev_compiler::analysis::enum_repr::build_enum_representation_plan as ast_plan;
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::enum_repr::build_enum_representation_plan as hir_plan;
use briev_compiler::hir::lower_program;
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
    let ast = ast_plan(&items);
    let hir = hir_plan(&program);
    let mut bad = Vec::new();
    if ast.defns != hir.defns {
        bad.push(format!("defns ast={:?} hir={:?}", ast.defns, hir.defns));
    }
    if ast.variants != hir.variants {
        bad.push(format!("variants ast={:?} hir={:?}", ast.variants, hir.variants));
    }
    if ast.escaping_return_lets != hir.escaping_return_lets {
        bad.push(format!(
            "escaping lets ast={:?} hir={:?}",
            ast.escaping_return_lets, hir.escaping_return_lets
        ));
    }
    bad
}

#[test]
fn curated_enum_repr_parity() {
    let src = "enum R { Ok(Int), Err(Int) };\n\
               enum S { A };\n\
               defn step(s: S) -> R { term Ok(1); };\n\
               defn fsm(n: Int, state: S, sum: Int) -> Int {\n\
                 when n <= 0 { term sum; };\n\
                 let r: R = step(state);\n\
                 term match r { Ok(next) => fsm(n - 1, next, sum + 1), Err(_) => fsm(n - 1, state, sum) };\n\
               };";
    let bad = compare(src);
    assert!(bad.is_empty(), "HIR enum-repr parity:\n{}", bad.join("\n"));
}

#[test]
fn corpus_enum_repr_parity() {
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
        "HIR_ENUM_REPR_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(mismatches.is_empty(), "HIR enum-repr mismatches:\n{}", mismatches.join("\n"));
}
