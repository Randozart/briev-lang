// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b parity: HIR swan-song detection/hoisting must match the AST version.

use briev_compiler::analysis::swan_song::{has_swan_song as ast_has, hoist_swan_song as ast_hoist};
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::lower_program;
use briev_compiler::hir::swan_song::{has_swan_song as hir_has, hoist_swan_song as hir_hoist};
use briev_compiler::hir::HirCallableKind;
use std::collections::HashSet;
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
    let state_fields: HashSet<String> = program.globals.iter().map(|g| g.name.clone()).collect();
    let mut bad = Vec::new();
    for txn in briev_compiler::analysis::effective_txns(&items) {
        let Some(hir_txn) = program.callables.iter().find(|c| {
            matches!(c.kind, HirCallableKind::Transaction) && c.owner.is_none() && c.name == txn.name
        }) else {
            bad.push(format!("{}: missing HIR txn", txn.name));
            continue;
        };
        let ast_flag = ast_has(&txn.body);
        let hir_flag = hir_has(&hir_txn.body.statements);
        if ast_flag != hir_flag {
            bad.push(format!("{}: has_swan_song ast={ast_flag} hir={hir_flag}", txn.name));
        }
        let (ast_body, ast_hoist) = ast_hoist(&txn.body, &state_fields);
        let (hir_body, hir_hoist) = hir_hoist(&hir_txn.body.statements, &state_fields);
        if ast_body.len() != hir_body.len()
            || ast_hoist.len() != hir_hoist.len()
            || ast_hoist.iter().zip(&hir_hoist).any(|(a, h)| a.len() != h.len())
        {
            bad.push(format!(
                "{}: hoist shape ast=({},{},{:?}) hir=({},{},{:?})",
                txn.name,
                ast_body.len(),
                ast_hoist.len(),
                ast_hoist.iter().map(|b| b.len()).collect::<Vec<_>>(),
                hir_body.len(),
                hir_hoist.len(),
                hir_hoist.iter().map(|b| b.len()).collect::<Vec<_>>(),
            ));
        }
    }
    bad
}

#[test]
fn curated_swan_song_parity() {
    let src = "let nesc: Int = 0;\n\
               let count: Int = 0;\n\
               txn tick [count < 10] { count = count + 1; nesc = count; when count == 10 { endprogram nesc; }; term; };";
    let bad = compare(src);
    assert!(bad.is_empty(), "HIR swan-song parity:\n{}", bad.join("\n"));
}

#[test]
fn corpus_swan_song_parity() {
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
        "HIR_SWAN_SONG_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(mismatches.is_empty(), "HIR swan-song mismatches:\n{}", mismatches.join("\n"));
}
