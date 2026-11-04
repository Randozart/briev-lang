// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b parity: HIR loop shapes must equal the AST shapes on every field.

use briev_compiler::analysis::loop_shape::{
    build_loop_shapes as ast_shapes, program_convergence as ast_program_convergence, LoopShape,
};
use briev_compiler::analysis::transition_graph::ReactorTransitionGraph;
use briev_compiler::ast::{Expr, TopLevel};
use briev_compiler::hir::lower_program;
use briev_compiler::hir::loop_shape::{
    build_loop_shapes as hir_shapes, program_convergence as hir_program_convergence, HirLoopShape,
};
use briev_compiler::hir::transition_graph::HirReactorTransitionGraph;
use briev_compiler::hir::expr::{HirExprKind, HirLiteral};
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

fn ast_bound_expr(e: &Expr) -> String {
    match e {
        Expr::Decimal(n) => format!("lit:{n}"),
        Expr::Identifier(n) => format!("id:{n}"),
        other => format!("other:{other:?}"),
    }
}

fn hir_bound_expr(e: &briev_compiler::hir::expr::HirExpr) -> String {
    match &e.kind {
        HirExprKind::Literal(HirLiteral::Int(n)) => format!("lit:{n}"),
        HirExprKind::Identifier(n) => format!("id:{n}"),
        other => format!("other:{other:?}"),
    }
}

fn compare_shape(ast: &LoopShape, hir: &HirLoopShape, label: &str) -> Vec<String> {
    let mut bad = Vec::new();
    let mut check = |cond: bool, what: &str| {
        if !cond {
            bad.push(format!("{label}: {what}"));
        }
    };
    check(ast.txn_name == hir.txn_name, "txn_name");
    check(ast.counter == hir.counter, "counter");
    check(ast.bound == hir.bound, "bound");
    check(ast.direction == hir.direction, "direction");
    check(ast.counter_only_writes == hir.counter_only_writes, "counter_only_writes");
    check(ast.field_classes == hir.field_classes, "field_classes");
    check(ast.carried_fields == hir.carried_fields, "carried_fields");
    check(ast.local_fields == hir.local_fields, "local_fields");
    check(ast.vector_groups == hir.vector_groups, "vector_groups");
    check(ast.carried_float_count == hir.carried_float_count, "carried_float_count");
    check(ast.carried_scalar_count == hir.carried_scalar_count, "carried_scalar_count");
    check(ast.carried_aggregate_count == hir.carried_aggregate_count, "carried_aggregate_count");
    check(
        ast.temp_mediated_float_count == hir.temp_mediated_float_count,
        "temp_mediated_float_count",
    );
    check(ast.has_swan_song == hir.has_swan_song, "has_swan_song");
    check(ast.is_pure == hir.is_pure, "is_pure");
    build_conv(ast, hir, &mut bad, label);
    bad
}

fn build_conv(ast: &LoopShape, hir: &HirLoopShape, bad: &mut Vec<String>, label: &str) {
    use briev_compiler::analysis::loop_shape::Convergence;
    use briev_compiler::hir::loop_shape::HirConvergence;
    match (&ast.convergence, &hir.convergence) {
        (
            Convergence::CounterGeBound { counter: c1, bound: b1 },
            HirConvergence::CounterGeBound { counter: c2, bound: b2 },
        ) => {
            if c1 != c2 || b1 != b2 {
                bad.push(format!("{label}: convergence"));
            }
        }
        (Convergence::Unprovable, HirConvergence::Unprovable) => {}
        _ => bad.push(format!("{label}: convergence variant")),
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
        let Some(h) = hir.get(name) else {
            bad.push(format!("{name}: missing HIR shape"));
            continue;
        };
        bad.extend(compare_shape(a, h, name));
    }
    for name in hir.keys() {
        if !ast.contains_key(name) {
            bad.push(format!("{name}: extra HIR shape"));
        }
    }

    let apc = ast_program_convergence(&ast_graph, &items, false);
    let hpc = hir_program_convergence(&program, &hir_graph, false);
    if apc.has_natural_exit != hpc.has_natural_exit {
        bad.push("program_convergence: has_natural_exit".to_string());
    }
    let a_bounds: Vec<(String, String)> = apc
        .counter_ge_bounds
        .iter()
        .map(|(c, e)| (c.clone(), ast_bound_expr(e)))
        .collect();
    let h_bounds: Vec<(String, String)> = hpc
        .counter_ge_bounds
        .iter()
        .map(|(c, e)| (c.clone(), hir_bound_expr(e)))
        .collect();
    if a_bounds != h_bounds {
        bad.push(format!(
            "program_convergence bounds ast={a_bounds:?} hir={h_bounds:?}"
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
