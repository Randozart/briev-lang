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
    assert_parity(
        "let b: Int = 0;\nnode n [b > 0][true] { term; };",
        "contract",
    );
    // Observed by `endprogram <expr>` → surface.
    assert_parity(
        "let c: Int = 0;\nnode n [true][true] { endprogram c; };",
        "endprogram",
    );
    // Written by a txn body → machine state.
    assert_parity(
        "let d: Int = 0;\nnode n [true][true] { d = d + 1; term; };",
        "written",
    );
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
            let mut diff: Vec<String> = ast.symmetric_difference(&hir).cloned().collect();
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

/// 2026-11-17: `export let` must be classified the same by both walks, and it
/// must never be plain — an exported symbol is host-visible storage, not a
/// register-resident SSA value. Before this, the AST (whose `let_of_item` is
/// export-transparent for `build_field_index`) counted it plain while the HIR
/// walk did not, and the exported name reached the read path with no
/// declaration at all (`load i64, ptr @K` → clang: use of undefined value).
#[test]
fn exported_let_parity_and_never_plain() {
    let src = "export let K: Int = 7;\n\
               let done: Bool = false;\n\
               node main [!done][done] { done = true; endprogram; };\n";
    assert_parity(src, "exported-let");

    let items = parse(src);
    let empty = HashSet::new();
    let ast = ast_plain(&items, None, &empty);
    assert!(
        !ast.contains("K"),
        "an exported let must never be classified plain, got {ast:?}"
    );

    // 2026-11-17 (reconciled with the export_abi fix): an exported let IS a
    // HIR global, lowered as `is_state_decl = true`. The AST `needs_state`
    // analysis (`export_abi::index_program`) registers an exported
    // `Statement::Let` in `state_fields` unconditionally, so HIR must see it
    // in `globals` too — but the HIR plain walk must skip it (an exported let
    // is never plain), which `is_state_decl = true` achieves. The
    // `hir_needs_state_parity` gate confirms both sides agree.
    let program = lower_program(&items);
    let k = program
        .globals
        .iter()
        .find(|g| g.name == "K")
        .expect("an exported top-level let is a HIR global");
    assert!(
        k.is_state_decl,
        "an exported let lowers as a state declaration so the HIR plain walk skips it"
    );
}
