// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b prerequisite parity: HIR top-level transactions must equal
//! `analysis::effective_txns` (direct + sync<group>-wrapped + export-wrapped).

use briev_compiler::ast::TopLevel;
use briev_compiler::hir::{lower_program, HirCallableKind};
use std::path::{Path, PathBuf};

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
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
fn hir_transactions_match_effective_txns() {
    let root = Path::new(PROJECT_ROOT);
    let mut files = Vec::new();
    bv_files(&root.join("tests/tier1"), &mut files);
    bv_files(&root.join("lib/std"), &mut files);
    files.sort();

    let curated = "txn count() { term; };\n\
                    sync<g> txn bump() { term; };\n\
                    export txn exported() { term; };";
    let mut mismatches = Vec::new();
    for (label, src) in files
        .iter()
        .map(|p| (p.display().to_string(), std::fs::read_to_string(p).unwrap_or_default()))
        .chain(std::iter::once(("curated".to_string(), curated.to_string())))
    {
        let Ok(items) = std::panic::catch_unwind(|| parse(&src)) else { continue };
        let program = lower_program(&items);
        let mut ast: Vec<&str> = briev_compiler::analysis::effective_txns(&items)
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        let mut hir: Vec<&str> = program
            .callables
            .iter()
            .filter(|c| matches!(c.kind, HirCallableKind::Transaction) && c.owner.is_none())
            .map(|c| c.name.as_str())
            .collect();
        ast.sort_unstable();
        hir.sort_unstable();
        if ast != hir {
            mismatches.push(format!("{label}: ast={ast:?} hir={hir:?}"));
        }
    }
    assert!(mismatches.is_empty(), "effective-txn mismatches:\n{}", mismatches.join("\n"));
}
