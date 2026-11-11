// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-11 (H5 port): batch-loop shape detection over typed HIR.
//!
//! Mirrors `analysis::batch_shape` (AST remains the fallback/oracle). The
//! result keeps the AST `BatchShape` type because the backend re-emits the
//! guard/inner statements; construction crosses `hir::to_ast` once, and any
//! unreconstructible node rejects the program (fallback to AST).

use super::expr::{HirExpr, HirExprKind};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::batch_shape::BatchShape;
use crate::analysis::node_decompose::PredicateClass;
use crate::ast::{BinaryOpKind, Statement};
use std::collections::HashMap;

pub enum Segment {
    Compute(Vec<HirStmt>),
    Guard {
        condition: HirExpr,
        body: Vec<HirStmt>,
        classification: PredicateClass,
        nested: Vec<Segment>,
    },
}

pub fn split_into_segments(body: &[HirStmt]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut compute = Vec::new();
    for stmt in body {
        if let HirStmtKind::Guarded(cond, guard_body) = &stmt.kind {
            if !compute.is_empty() {
                segments.push(Segment::Compute(std::mem::take(&mut compute)));
            }
            segments.push(Segment::Guard {
                condition: cond.clone(),
                body: guard_body.clone(),
                classification: classify_predicate(cond),
                nested: split_into_segments(guard_body),
            });
        } else {
            compute.push(stmt.clone());
        }
    }
    if !compute.is_empty() {
        segments.push(Segment::Compute(compute));
    }
    segments
}

pub fn classify_predicate(cond: &HirExpr) -> PredicateClass {
    match &cond.kind {
        HirExprKind::Literal(super::expr::HirLiteral::Bool(true)) => PredicateClass::AlwaysTrue,
        HirExprKind::Literal(super::expr::HirLiteral::Bool(false)) => PredicateClass::AlwaysFalse,
        HirExprKind::Binary { op, left, right }
            if matches!(op, BinaryOpKind::Eq | BinaryOpKind::Neq) =>
        {
            match (const_value(left), const_value(right)) {
                (Some(a), Some(b)) => {
                    let equal = a == b;
                    if (*op == BinaryOpKind::Eq) == equal {
                        PredicateClass::AlwaysTrue
                    } else {
                        PredicateClass::AlwaysFalse
                    }
                }
                _ => PredicateClass::Runtime,
            }
        }
        _ => PredicateClass::Runtime,
    }
}

fn const_value(e: &HirExpr) -> Option<i64> {
    match &e.kind {
        HirExprKind::Literal(super::expr::HirLiteral::Int(n)) => Some(*n),
        _ => None,
    }
}

pub fn detect_batch_shapes(
    swan_songs: &HashMap<String, (Vec<HirStmt>, Vec<Vec<HirStmt>>)>,
) -> Option<HashMap<String, BatchShape>> {
    let mut result = HashMap::new();
    for (name, (stripped, _)) in swan_songs {
        let Some(Some(shape)) = detect_for_body(name, stripped) else {
            continue;
        };
        result.insert(name.clone(), shape);
    }
    Some(result)
}

fn detect_for_body(txn_name: &str, body: &[HirStmt]) -> Option<Option<BatchShape>> {
    let segments = split_into_segments(body);
    let mut guard: Option<(HirExpr, Vec<HirStmt>)> = None;
    let mut counter = None;
    let mut batch_size = None;
    let mut pre: Vec<HirStmt> = Vec::new();
    let mut post: Vec<HirStmt> = Vec::new();
    let mut seen_guard = false;
    for seg in &segments {
        match seg {
            Segment::Compute(stmts) => {
                if seen_guard {
                    post.extend(stmts.clone());
                } else {
                    pre.extend(stmts.clone());
                }
            }
            Segment::Guard { condition, body, classification, .. } => {
                if seen_guard || *classification != PredicateClass::Runtime {
                    return None;
                }
                seen_guard = true;
                let (c, n) = extract_batch_condition(condition)?;
                counter = Some(c);
                batch_size = Some(n);
                guard = Some((condition.clone(), body.clone()));
            }
        }
    }
    let (cond, gbody) = guard?;
    let counter = counter?;
    let batch_size = batch_size?;
    let pre_has_inc = pre.iter().any(|s| is_counter_increment(s, &counter));
    let post_has_inc = post.iter().any(|s| is_counter_increment(s, &counter));
    if !pre_has_inc && !post_has_inc {
        return None;
    }
    if !pre_has_inc {
        return None;
    }
    let mut inner = pre.clone();
    inner.extend(post);
    let guard_ast = Statement::Guarded(
        crate::hir::to_ast::expr(&cond)?,
        crate::hir::to_ast::stmt_body(&gbody)?,
    );
    let guard_body = crate::hir::to_ast::stmt_body(&gbody)?;
    let inner_body = crate::hir::to_ast::stmt_body(&inner)?;
    Some(Some(BatchShape {
        txn_name: txn_name.to_string(),
        counter,
        batch_size,
        guard: guard_ast,
        guard_body,
        inner_body,
    }))
}

fn extract_batch_condition(cond: &HirExpr) -> Option<(String, usize)> {
    let HirExprKind::Binary { op: BinaryOpKind::Eq, left, right } = &cond.kind else {
        return None;
    };
    extract_mod_zero(left, right).or_else(|| extract_mod_zero(right, left))
}

fn extract_mod_zero(a: &HirExpr, b: &HirExpr) -> Option<(String, usize)> {
    let HirExprKind::Binary { op: BinaryOpKind::Mod, left, right } = &a.kind else {
        return None;
    };
    let HirExprKind::Identifier(counter) = &left.kind else {
        return None;
    };
    let HirExprKind::Literal(super::expr::HirLiteral::Int(n)) = &right.kind else {
        return None;
    };
    let HirExprKind::Literal(super::expr::HirLiteral::Int(0)) = &b.kind else {
        return None;
    };
    (*n > 1).then(|| (counter.clone(), *n as usize))
}

fn is_counter_increment(stmt: &HirStmt, counter: &str) -> bool {
    let HirStmtKind::Assign(lhs, rhs) = &stmt.kind else {
        return false;
    };
    if !matches!(&lhs.kind, HirExprKind::Identifier(n) if n == counter) {
        return false;
    }
    let HirExprKind::Binary { op: BinaryOpKind::Add, left, right } = &rhs.kind else {
        return false;
    };
    (matches!(&left.kind, HirExprKind::Identifier(n) if n == counter)
        && matches!(&right.kind, HirExprKind::Literal(super::expr::HirLiteral::Int(1))))
        || (matches!(&left.kind, HirExprKind::Literal(super::expr::HirLiteral::Int(1)))
            && matches!(&right.kind, HirExprKind::Identifier(n) if n == counter))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn songs(src: &str) -> (HashMap<String, (Vec<Statement>, Vec<Vec<Statement>>)>, HashMap<String, (Vec<HirStmt>, Vec<Vec<HirStmt>>)>) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let program = crate::hir::lower_program(&items);
        let state: std::collections::HashSet<String> = program
            .globals
            .iter()
            .filter(|g| g.is_state_decl || g.ty.is_some())
            .map(|g| g.name.clone())
            .collect();
        let mut ast = HashMap::new();
        let mut hir = HashMap::new();
        for item in &items {
            if let crate::ast::TopLevel::Transaction(t) = item {
                ast.insert(
                    t.name.clone(),
                    crate::analysis::swan_song::hoist_swan_song(&t.body, &state),
                );
            }
        }
        for c in &program.callables {
            if matches!(c.kind, crate::hir::HirCallableKind::Transaction)
                && c.owner.is_none()
                && !c.exported
            {
                hir.insert(
                    c.name.clone(),
                    crate::hir::swan_song::hoist_swan_song(&c.body.statements, &state),
                );
            }
        }
        (ast, hir)
    }

    fn parity(src: &str) {
        let (ast_songs, hir_songs) = songs(src);
        let expected = crate::analysis::batch_shape::detect_batch_shapes(&ast_songs);
        let actual = detect_batch_shapes(&hir_songs).expect("reconstructible");
        assert_eq!(actual, expected, "batch-shape drift:\n{src}");
    }

    #[test]
    fn post_increment_guard_matches_ast() {
        parity(
            "state let count: Int = 0;\nstate let x0: Float = 0.0;\n\
             node step [true][true] {\n\
                 x0 = x0 + 1.0;\n\
                 count = count + 1;\n\
                 when count % 5000000 == 0 { PrintLn#(x0); };\n\
             };",
        );
    }

    #[test]
    fn rejected_shapes_match_ast() {
        parity(
            "state let count: Int = 0;\n\
             node pre [true][true] { when count % 5 == 0 { PrintLn#(count); }; count = count + 1; };\n\
             node nonperiodic [true][true] { count = count + 1; when count == 9 { PrintLn#(count); }; };",
        );
    }
}
