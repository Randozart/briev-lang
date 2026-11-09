// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! P1 negative tests: each verification invariant fails on a crafted
//! violation (a green suite would mean the checker is asleep).

use briev_compiler::casting::graph::CastingGraph;
use briev_compiler::hir::expr::{HirExpr, HirExprKind};
use briev_compiler::hir::stmt::{HirStmt, HirStmtKind};
use briev_compiler::hir::{lower_program, verify, verify_law_markers, verify_typed, HirProgram};
use briev_compiler::type_universe::TypeUniverse;

fn lower(src: &str) -> HirProgram {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    let items = parser.parse_program().expect("parse");
    lower_program(&items)
}

fn annotate(program: &mut HirProgram) {
    let universe = TypeUniverse::new();
    let graph = CastingGraph::new();
    briev_compiler::hir::typing::annotate_types(program, &universe, &graph);
}

fn first_guarded_body(program: &mut HirProgram) -> &mut Vec<HirStmt> {
    for callable in &mut program.callables {
        for stmt in &mut callable.body.statements {
            if let HirStmtKind::Guarded(_, body) = &mut stmt.kind {
                return body;
            }
        }
    }
    panic!("guarded body expected");
}

fn walk_exprs<F: FnMut(&mut HirExpr)>(program: &mut HirProgram, f: &mut F) {
    for callable in &mut program.callables {
        briev_compiler::hir::walk::walk_body_mut(&mut callable.body.statements, f);
    }
}

#[test]
fn duplicate_node_id_is_rejected() {
    let mut program = lower("defn f() -> Int { term 1; };");
    let body = &mut program.callables[0].body.statements;
    let id = body[0].id;
    let HirStmtKind::Term(Some(expr)) = &mut body[0].kind else {
        panic!("term expected");
    };
    expr.id = id; // an expression and its statement may not share an id
    let err = verify(&program).expect_err("duplicate id must fail");
    assert!(err.contains("duplicate NodeId"), "{err}");
}

#[test]
fn through_without_hold_is_rejected() {
    let mut program = lower("node n [true][true] { when true { term; }; };");
    first_guarded_body(&mut program).push(HirStmt {
        id: briev_compiler::hir::ids::IdSupply::default().node(),
        span: None,
        kind: HirStmtKind::MetadataAssignment(
            "through".to_string(),
            briev_compiler::ast::PropertyValue::Identifier("Fet".into()),
        ),
    });
    let err = verify_law_markers(&program).expect_err("through without hold must fail");
    assert!(err.contains("through"), "{err}");
}

#[test]
fn duplicate_hold_marker_is_rejected() {
    let mut program = lower("node n [true][true] { when true { term; }; };");
    let marker = |id| HirStmt {
        id,
        span: None,
        kind: HirStmtKind::MetadataAssignment(
            "hold".to_string(),
            briev_compiler::ast::PropertyValue::Bool(true),
        ),
    };
    let mut ids = briev_compiler::hir::ids::IdSupply::default();
    let body = first_guarded_body(&mut program);
    body.push(marker(ids.node()));
    body.push(marker(ids.node()));
    let err = verify_law_markers(&program).expect_err("two hold markers must fail");
    assert!(err.contains("2 `hold` markers"), "{err}");
}

#[test]
fn hold_marker_outside_a_guarded_body_is_rejected() {
    let mut program = lower("node n [true][true] { { term; }; };");
    let body = &mut program.callables[0].body.statements;
    let HirStmtKind::Block(inner) = &mut body[0].kind else {
        panic!("block expected");
    };
    inner.push(HirStmt {
        id: briev_compiler::hir::ids::IdSupply::default().node(),
        span: None,
        kind: HirStmtKind::MetadataAssignment(
            "hold".to_string(),
            briev_compiler::ast::PropertyValue::Bool(true),
        ),
    });
    let err = verify_law_markers(&program).expect_err("stray hold marker must fail");
    assert!(err.contains("outside a guarded body"), "{err}");
}

#[test]
fn identifier_type_disagreement_is_rejected() {
    let mut program = lower("defn f(x: Int) -> Int { term x; };");
    annotate(&mut program);
    walk_exprs(&mut program, &mut |expr: &mut HirExpr| {
        if let HirExprKind::Identifier(name) = &expr.kind {
            if name == "x" {
                expr.ty = Some(briev_compiler::ast::Type::float());
            }
        }
    });
    let universe = TypeUniverse::new();
    let graph = CastingGraph::new();
    let err = verify_typed(&program, &universe, &graph)
        .expect_err("identifier type drift must fail");
    assert!(err.contains("annotated"), "{err}");
}

#[test]
fn unresolved_cast_path_is_rejected() {
    let mut program = lower("defn f(x: Int) -> Float { term x as Float; };");
    annotate(&mut program);
    walk_exprs(&mut program, &mut |expr: &mut HirExpr| {
        if let HirExprKind::Cast { via, .. } = &mut expr.kind {
            *via = None;
        }
    });
    let universe = TypeUniverse::new();
    let graph = CastingGraph::new();
    let err = verify_typed(&program, &universe, &graph)
        .expect_err("missing cast path must fail");
    assert!(err.contains("resolved `via`"), "{err}");
}

#[test]
fn a_clean_program_passes_both_layers() {
    let mut program = lower(
        "defn f(x: Int) -> Float { term x as Float; };\n\
         node n [true][true] { when true { term; }; };",
    );
    annotate(&mut program);
    verify(&program).expect("clean program must verify");
    verify_law_markers(&program).expect("clean markers must verify");
    let universe = TypeUniverse::new();
    let graph = CastingGraph::new();
    verify_typed(&program, &universe, &graph).expect("clean typed program must verify");
}
