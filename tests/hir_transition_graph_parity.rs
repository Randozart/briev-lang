// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b parity: HIR reactor transition graph must equal the AST graph
//! (still authoritative) on every node fact.

use briev_compiler::analysis::transition_graph::{ReactorTransitionGraph, ReactorNode};
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::lower_program;
use briev_compiler::hir::transition_graph::{HirReactorNode, HirReactorTransitionGraph};
use std::collections::HashSet;
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

fn bp_dbg(
    bp: &Option<briev_compiler::analysis::transition_graph::BoundedPre>,
) -> Option<(String, String, String, Option<i64>)> {
    bp.as_ref().map(|b| {
        (
            b.var.clone(),
            b.bound_var.clone(),
            format!("{:?}", b.direction),
            b.bound_literal,
        )
    })
}

fn inc_dbg(
    inc: &Option<briev_compiler::analysis::transition_graph::IncrementInfo>,
) -> Option<(String, i64)> {
    inc.as_ref().map(|i| (i.var.clone(), i.delta))
}

fn compare_ast_hir(ast: &ReactorNode, hir: &HirReactorNode, label: &str) -> Vec<String> {
    let mut bad = Vec::new();
    let mut check = |cond: bool, what: &str| {
        if !cond {
            bad.push(format!("{label}: {what}"));
        }
    };
    check(ast.name == hir.name, "name");
    check(ast.is_reactive == hir.is_reactive, "is_reactive");
    check(bp_dbg(&ast.bounded_pre) == bp_dbg(&hir.bounded_pre), "bounded_pre");
    check(inc_dbg(&ast.increments) == inc_dbg(&hir.increments), "increments");
    check(ast.is_pure_body == hir.is_pure_body, "is_pure_body");
    check(ast.write_set == hir.write_set, "write_set");
    check(
        ast.is_effectively_pure == hir.is_effectively_pure,
        "is_effectively_pure",
    );
    check(
        ast.lexicographic_vars == hir.lexicographic_vars,
        "lexicographic_vars",
    );
    bad
}

fn assert_graph_parity(src: &str, label: &str) {
    let items = parse(src);
    let ast = ReactorTransitionGraph::build(&items, &None, &[]);
    let program = lower_program(&items);
    let hir = HirReactorTransitionGraph::build(&program, None, &[]);

    let mut bad = Vec::new();
    if ast.nodes.len() != hir.nodes.len() {
        bad.push(format!(
            "{label}: node count ast={} hir={}",
            ast.nodes.len(),
            hir.nodes.len()
        ));
    } else {
        for (a, h) in ast.nodes.iter().zip(hir.nodes.iter()) {
            bad.extend(compare_ast_hir(a, h, label));
        }
    }
    if ast.has_triggers != hir.has_triggers {
        bad.push(format!("{label}: has_triggers"));
    }
    if ast.live_fields != hir.live_fields {
        bad.push(format!(
            "{label}: live_fields ast={:?} hir={:?}",
            ast.live_fields, hir.live_fields
        ));
    }
    if ast.has_unguarded_ffi != hir.has_unguarded_ffi {
        bad.push(format!(
            "{label}: has_unguarded_ffi ast={:?} hir={:?}",
            ast.has_unguarded_ffi, hir.has_unguarded_ffi
        ));
    }
    assert!(bad.is_empty(), "HIR transition-graph parity:\n{}", bad.join("\n"));
}

#[test]
fn curated_transition_graph_parity() {
    assert_graph_parity(
        "let count: Int = 0;\n\
         let total: Int = 0;\n\
         defn helper(x: Int) -> Int { term x + 1; };\n\
         txn tick [count < 100] { count = count + 1; total = total + count; let y: Int = helper(count); term; };",
        "bounded-counter",
    );
    assert_graph_parity(
        "let n: Int = 128;\n\
         let bits: Int = 0;\n\
         txn pop [bits != 0] { bits = bits & (bits - 1); term; };",
        "popcount",
    );
    assert_graph_parity(
        "let x: Int = 0;\n\
         let y: Int = 0;\n\
         txn dec [x > 0 && y > 0] { x = x - 1; y = y - 1; term; };",
        "lexicographic",
    );
    assert_graph_parity(
        "let a: Int = 0;\n\
         sync<g> txn bump [a < 10] { a = a + 1; term; };",
        "sync-group",
    );
}

#[test]
fn corpus_transition_graph_parity() {
    let root = Path::new(PROJECT_ROOT);
    let mut files = Vec::new();
    bv_files(&root.join("tests/tier1"), &mut files);
    bv_files(&root.join("lib/std"), &mut files);
    bv_files(&root.join("benchmarks"), &mut files);
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
        let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
        let ast = ReactorTransitionGraph::build(&items, &None, &[]);
        let hir = HirReactorTransitionGraph::build(&program, None, &[]);
        if ast.nodes.len() != hir.nodes.len() {
            mismatches.push(format!(
                "{rel}: node count ast={} hir={}",
                ast.nodes.len(),
                hir.nodes.len()
            ));
            continue;
        }
        for (a, h) in ast.nodes.iter().zip(hir.nodes.iter()) {
            mismatches.extend(compare_ast_hir(a, h, &rel));
        }
        if ast.has_triggers != hir.has_triggers {
            mismatches.push(format!("{rel}: has_triggers"));
        }
        if ast.live_fields != hir.live_fields {
            mismatches.push(format!(
                "{rel}: live_fields ast={:?} hir={:?}",
                ast.live_fields, hir.live_fields
            ));
        }
        if ast.has_unguarded_ffi != hir.has_unguarded_ffi {
            mismatches.push(format!(
                "{rel}: has_unguarded_ffi ast={:?} hir={:?}",
                ast.has_unguarded_ffi, hir.has_unguarded_ffi
            ));
        }
    }

    println!(
        "HIR_TRANSITION_GRAPH_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "HIR transition-graph parity mismatches:\n{}",
        mismatches.join("\n")
    );
    let _ = HashSet::<String>::new();
}
