// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.1 parity: HIR `needs_state`/`needs_plain` must equal the AST analysis
//! (still authoritative) over the corpus. Files with unsupported HIR nodes are
//! counted and skipped until H3.0 closes their lowering.

use briev_compiler::analysis::export_abi::{
    compute_defn_needs_plain as ast_needs_plain, compute_defn_needs_state as ast_needs_state,
};
use briev_compiler::analysis::storage_class::classify_plain_lets;
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::{
    compute_defn_needs_plain as hir_needs_plain, compute_defn_needs_state as hir_needs_state,
    lower_program, unsupported_counts,
};
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
    let plain = classify_plain_lets(&items, None, &HashSet::new());
    let ast = ast_needs_state(&items, &plain);
    let hir = hir_needs_state(&lower_program(&items), &plain);
    assert_eq!(ast, hir, "{label}: needs_state parity");
    let ast_plain = ast_needs_plain(&items, &plain);
    let hir_plain = hir_needs_plain(&lower_program(&items), &plain);
    assert_eq!(ast_plain, hir_plain, "{label}: needs_plain parity");
}

#[test]
fn curated_needs_state_parity() {
    // Pure defn — stateless.
    assert_parity(
        "defn pure(x: Int) -> Int { term x + 1; };",
        "pure",
    );
    // Reads a top-level state global → stateful.
    assert_parity(
        "let g: Int = 0;\ndefn readg() -> Int { term g; };",
        "state-read",
    );
    // Calls a transaction → stateful (txns always thread %state).
    assert_parity(
        "txn tick(x: Int) -> Int { term x + 1; };\ndefn callt() -> Int { term tick(1); };",
        "txn-call",
    );
    // State-carrying intrinsic → stateful.
    assert_parity(
        "defn alloc_one() -> Int { let p: Int = Malloc#(8); term p; };",
        "intrinsic",
    );
    // Stateful member reached by dot-call forces the caller stateful.
    assert_parity(
        "obj Cell { v: Int;\n  defn make(this: Cell) -> Int { let p: Int = Malloc#(8); term p; };\n};\ndefn use(c: Cell) -> Int { term c.make(); };",
        "method-call",
    );
    // Stateless member (reads its parameter) does NOT force state.
    assert_parity(
        "obj Box { v: Int;\n  defn get(this: Box) -> Int { term this.v; };\n};\ndefn use2(b: Box) -> Int { term b.get(); };",
        "method-pure",
    );
    // Trait default over a state-carrying intrinsic.
    assert_parity(
        "trait T { defn d(this: Self) -> Int { let p: Int = Malloc#(8); term p; }; };",
        "trait-default",
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
fn corpus_needs_state_parity() {
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
        if unsupported_counts(&program).total() != 0 {
            skipped += 1;
            continue;
        }
        compared += 1;
        let plain = classify_plain_lets(&items, None, &HashSet::new());
        let ast = ast_needs_state(&items, &plain);
        let hir = hir_needs_state(&program, &plain);
        if ast != hir {
            let rel = path.strip_prefix(root).unwrap_or(path).display();
            let mut diff: Vec<String> = ast
                .iter()
                .filter(|(k, v)| hir.get(*k) != Some(v))
                .map(|(k, v)| format!("{k}: ast={v} hir={:?}", hir.get(k)))
                .collect();
            let mut extra: Vec<String> = hir
                .keys()
                .filter(|k| !ast.contains_key(*k))
                .cloned()
                .collect();
            diff.sort();
            extra.sort();
            mismatches.push(format!("{rel}\n  {}\n  extra-hir: {extra:?}", diff.join("\n  ")));
        }
    }

    println!(
        "HIR_NEEDS_STATE_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "HIR needs_state parity mismatches:\n{}",
        mismatches.join("\n")
    );
}
