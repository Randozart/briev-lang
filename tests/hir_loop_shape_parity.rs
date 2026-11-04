// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H4 slice A parity: HIR loop shapes and program convergence must equal the
//! AST analysis exactly (both now return the shared consumer types).

use briev_compiler::analysis::loop_shape::{
    build_loop_shapes as ast_shapes, program_convergence as ast_program_convergence,
};
use briev_compiler::analysis::transition_graph::ReactorTransitionGraph;
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::lower_program;
use briev_compiler::hir::loop_shape::{
    build_loop_shapes as hir_shapes, program_convergence as hir_program_convergence,
};
use briev_compiler::hir::transition_graph::HirReactorTransitionGraph;
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

fn compare_file(src: &str) -> Vec<String> {
    let items = parse(src);
    let program = lower_program(&items);
    let ast_graph = ReactorTransitionGraph::build(&items, &None, &[]);
    let hir_graph = HirReactorTransitionGraph::build(&program, None, &[]);
    let ast = ast_shapes(&ast_graph, &items, 4);
    let hir = hir_shapes(&program, &hir_graph, 4);
    let mut bad = Vec::new();

    if ast.len() != hir.len() {
        bad.push(format!("shape count ast={} hir={}", ast.len(), hir.len()));
    }
    for (name, a) in &ast {
        match hir.get(name) {
            Some(h) if a == h => {}
            Some(h) => bad.push(format!("{name}: shape differs\n  ast={a:?}\n  hir={h:?}")),
            None => bad.push(format!("{name}: missing HIR shape")),
        }
    }
    for name in hir.keys() {
        if !ast.contains_key(name) {
            bad.push(format!("{name}: extra HIR shape"));
        }
    }

    let apc = ast_program_convergence(&ast_graph, &items, false);
    let hpc = hir_program_convergence(&program, &hir_graph, false);
    if apc != hpc {
        bad.push(format!(
            "program_convergence differs\n  ast={apc:?}\n  hir={hpc:?}"
        ));
    }
    bad
}

#[test]
fn curated_loop_shape_parity() {
    let src = "let count: Int = 0;\n\
               let total: Int = 0;\n\
               let bound: Int = 100;\n\
               txn tick [count < bound] { count = count + 1; total = total + count; term; };";
    let bad = compare_file(src);
    assert!(bad.is_empty(), "HIR loop-shape parity:\n{}", bad.join("\n"));
}

#[test]
fn benchmarks_loop_shape_parity() {
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
        for b in compare_file(&source) {
            mismatches.push(format!("{rel}: {b}"));
        }
    }
    println!(
        "HIR_LOOP_SHAPE_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "HIR loop-shape mismatches:\n{}",
        mismatches.join("\n")
    );
}
