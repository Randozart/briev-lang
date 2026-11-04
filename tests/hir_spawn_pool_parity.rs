// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.5 parity: HIR spawn-pool capacities/terms/errors must equal the AST ones.

use briev_compiler::analysis::spawn_pool::analyze as ast_analyze;
use briev_compiler::ast::{Expr, TopLevel};
use briev_compiler::hir::expr::{HirExpr, HirExprKind, HirLiteral};
use briev_compiler::hir::lower_program;
use briev_compiler::hir::spawn_pool::analyze as hir_analyze;
use std::collections::HashMap;
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

fn ast_bound(e: &Expr) -> String {
    match e {
        Expr::Identifier(n) => format!("id:{n}"),
        Expr::Decimal(n) => format!("lit:{n}"),
        Expr::Field(o, f) => format!("field:{}:{f}", ast_bound(o)),
        Expr::BinaryOp(op, l, r) => format!("bin:{op:?}:{}:{}", ast_bound(l), ast_bound(r)),
        _ => "other".to_string(),
    }
}

fn hir_bound(e: &HirExpr) -> String {
    match &e.kind {
        HirExprKind::Identifier(n) => format!("id:{n}"),
        HirExprKind::Literal(HirLiteral::Int(n)) => format!("lit:{n}"),
        HirExprKind::Field { receiver, name } => format!("field:{}:{name}", hir_bound(receiver)),
        HirExprKind::Binary { op, left, right } => {
            format!("bin:{op:?}:{}:{}", hir_bound(left), hir_bound(right))
        }
        _ => "other".to_string(),
    }
}

fn canon_terms(mut terms: Vec<(i64, String)>) -> Vec<(i64, String)> {
    terms.sort();
    terms
}

fn compare(src: &str) -> Vec<String> {
    let items = parse(src);
    let program = lower_program(&items);
    let (a_caps, a_dep, a_errs, a_storage) = ast_analyze(&items);
    let (h_caps, h_dep, h_errs, h_storage) = hir_analyze(&program);
    let mut bad = Vec::new();
    if a_caps != h_caps {
        bad.push(format!("capacities ast={a_caps:?} hir={h_caps:?}"));
    }
    if a_errs != h_errs {
        bad.push(format!("errors ast={a_errs:?} hir={h_errs:?}"));
    }
    if a_storage != h_storage {
        bad.push(format!("storage ast={a_storage:?} hir={h_storage:?}"));
    }
    let a_terms: HashMap<String, Vec<(i64, String)>> = a_dep
        .into_iter()
        .map(|(k, v)| {
            (k, canon_terms(v.into_iter().map(|t| (t.multiplier, ast_bound(&t.bound))).collect()))
        })
        .collect();
    let h_terms: HashMap<String, Vec<(i64, String)>> = h_dep
        .into_iter()
        .map(|(k, v)| {
            (k, canon_terms(v.into_iter().map(|t| (t.multiplier, hir_bound(&t.bound))).collect()))
        })
        .collect();
    if a_terms != h_terms {
        bad.push(format!("dependent ast={a_terms:?} hir={h_terms:?}"));
    }
    bad
}

#[test]
fn curated_spawn_pool_parity() {
    let src = "let ticks: Int = 0;\n\
               let N: Int = 4;\n\
               txn work [ticks < N][ticks == N] { let h = spawn Counter(); ticks = ticks + 1; term; };\n\
               txn fixed [ticks < 3][ticks == 3] { let g = spawn Counter(); term; };";
    let bad = compare(src);
    assert!(bad.is_empty(), "HIR spawn-pool parity:\n{}", bad.join("\n"));
}

#[test]
fn corpus_spawn_pool_parity() {
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
        "HIR_SPAWN_POOL_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(mismatches.is_empty(), "HIR spawn-pool mismatches:\n{}", mismatches.join("\n"));
}
