// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b parity: HIR vector-phi candidate discovery must equal the AST version.

use briev_compiler::analysis::slp_isomorphism::{analyze_body as ast_analyze, VectorPhiCandidate};
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::lower_program;
use briev_compiler::hir::slp_isomorphism::analyze_body as hir_analyze;
use briev_compiler::hir::HirCallableKind;
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

fn canon(mut groups: Vec<VectorPhiCandidate>) -> Vec<(String, String, usize, Vec<String>)> {
    groups.sort_by(|a, b| {
        (a.group_name.clone(), a.width, a.fields.clone())
            .cmp(&(b.group_name.clone(), b.width, b.fields.clone()))
    });
    groups
        .into_iter()
        .map(|g| (g.group_name, format!("{:?}", g.element_ty), g.width, g.fields))
        .collect()
}

fn compare(src: &str) -> Vec<String> {
    let items = parse(src);
    let program = lower_program(&items);
    let mut bad = Vec::new();
    for txn in briev_compiler::analysis::effective_txns(&items) {
        let Some(hir_txn) = program.callables.iter().find(|c| {
            matches!(c.kind, HirCallableKind::Transaction) && c.owner.is_none() && c.name == txn.name
        }) else {
            bad.push(format!("{}: missing HIR txn", txn.name));
            continue;
        };
        let ast = canon(ast_analyze(&txn.body, 2));
        let hir = canon(hir_analyze(&hir_txn.body.statements, 2));
        if ast != hir {
            bad.push(format!("{}:\n  ast={ast:?}\n  hir={hir:?}", txn.name));
        }
    }
    bad
}

#[test]
fn corpus_slp_isomorphism_parity() {
    let root = Path::new(PROJECT_ROOT);
    let mut files = Vec::new();
    bv_files(&root.join("tests/tier1"), &mut files);
    bv_files(&root.join("lib/std"), &mut files);
    bv_files(&root.join("benchmarks"), &mut files);
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
        "HIR_SLP_ISOMORPHISM_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(mismatches.is_empty(), "HIR SLP mismatches:\n{}", mismatches.join("\n"));
}
