// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H5 inventory port: float computation density over typed HIR.
//! Mirror of `analysis::density` (AST, still authoritative until the
//! consumer flip). Reuses `ComputeDensity` and `is_float_type`.
//!
//! Coverage mirrors the AST walker exactly: BARE top-level reactive
//! transactions only; program float bindings = top-level lets WITH an
//! initializer, `const` declarations (declared type included — see
//! `HirProgram::const_types`), and state declarations; per-body float
//! determination = TOP-LEVEL lets only (nested guard lets excluded, per
//! the backend-metric parity note in the AST module).

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::density::{is_float_type, ComputeDensity};
use crate::ast::Type;
use std::collections::{HashMap, HashSet};

pub fn compute_densities(program: &HirProgram) -> HashMap<String, ComputeDensity> {
    let program_float = collect_program_float_bindings(program);
    let mut out = HashMap::new();
    for c in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Transaction)
            && c.owner.is_none()
            && !c.exported
            && c.sync_domains.is_empty()
            && c.is_reactive
    }) {
        out.insert(c.name.clone(), density_of_body(&c.body.statements, &program_float));
    }
    out
}

fn collect_program_float_bindings(program: &HirProgram) -> HashSet<String> {
    let const_types: HashMap<&str, &Type> = program
        .const_types
        .iter()
        .map(|(n, t)| (n.as_str(), t))
        .collect();
    let mut bindings: Vec<(String, Option<Type>, Option<&HirExpr>)> = Vec::new();
    for g in &program.globals {
        if g.is_state_decl {
            bindings.push((g.name.clone(), g.ty.clone(), None));
            continue;
        }
        if let Some(e) = &g.expr {
            bindings.push((g.name.clone(), g.ty.clone(), Some(e)));
        }
    }
    for (name, e) in &program.const_exprs {
        bindings.push((
            name.clone(),
            const_types.get(name.as_str()).map(|t| (*t).clone()),
            Some(e),
        ));
    }
    float_fixpoint(&bindings)
}

fn density_of_body(body: &[HirStmt], program_float: &HashSet<String>) -> ComputeDensity {
    let top_lets: Vec<&HirStmt> = body
        .iter()
        .filter(|s| matches!(s.kind, HirStmtKind::Let { .. }))
        .collect();
    let mut float_set: HashSet<String> = program_float.clone();
    while float_let_round(&top_lets, &mut float_set) {}
    let float_idents = count_float_idents(&top_lets, &float_set);
    let cross_ops: u32 = top_lets
        .iter()
        .filter_map(|s| match &s.kind {
            HirStmtKind::Let { expr: Some(e), .. } => Some(count_cross_float_ops(e, &float_set)),
            _ => None,
        })
        .sum();
    let per_field = if float_idents == 0 {
        0.0
    } else {
        cross_ops as f64 / float_idents as f64
    };
    ComputeDensity {
        float_idents,
        cross_ops,
        per_field,
    }
}

/// One float-let fixpoint round. Returns whether the set grew.
fn float_let_round(top_lets: &[&HirStmt], float_set: &mut HashSet<String>) -> bool {
    let before = float_set.len();
    for s in top_lets {
        let HirStmtKind::Let { name, names, expr: Some(e), .. } = &s.kind else {
            continue;
        };
        if !expr_is_float(e, float_set) {
            continue;
        }
        mark_float_names(name, names, float_set);
    }
    float_set.len() != before
}

fn mark_float_names(name: &str, names: &[String], float_set: &mut HashSet<String>) {
    float_set.insert(name.to_string());
    for n in names {
        float_set.insert(n.clone());
    }
}

fn count_float_idents(top_lets: &[&HirStmt], float_set: &HashSet<String>) -> usize {
    top_lets
        .iter()
        .filter(|s| matches!(&s.kind, HirStmtKind::Let { name, .. } if float_set.contains(name)))
        .count()
}

fn float_fixpoint(bindings: &[(String, Option<Type>, Option<&HirExpr>)]) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    for (name, ty, e) in bindings {
        let explicit_float = ty.as_ref().map_or(false, is_float_type);
        let literal_float = e.map_or(false, expr_has_float_literal);
        if explicit_float || literal_float {
            out.insert(name.clone());
        }
    }
    while float_binding_round(bindings, &mut out) {}
    out
}

/// One binding fixpoint round. Returns whether the set grew.
fn float_binding_round(
    bindings: &[(String, Option<Type>, Option<&HirExpr>)],
    out: &mut HashSet<String>,
) -> bool {
    let before = out.len();
    for (name, _ty, e) in bindings {
        if e.map_or(false, |x| expr_is_float(x, out)) {
            out.insert(name.clone());
        }
    }
    out.len() != before
}

fn expr_has_float_literal(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Literal(HirLiteral::Float(_)) => true,
        HirExprKind::Binary { left, right, .. } => {
            expr_has_float_literal(left) || expr_has_float_literal(right)
        }
        HirExprKind::Unary { inner, .. } => expr_has_float_literal(inner),
        HirExprKind::Call { args, .. } => args.iter().any(expr_has_float_literal),
        HirExprKind::Cast { inner, .. } => expr_has_float_literal(inner),
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            items.iter().any(expr_has_float_literal)
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            expr_has_float_literal(condition)
                || expr_has_float_literal(then_branch)
                || else_branch.as_ref().map_or(false, |x| expr_has_float_literal(x))
        }
        _ => false,
    }
}

fn expr_is_float(expr: &HirExpr, float_set: &HashSet<String>) -> bool {
    match &expr.kind {
        HirExprKind::Literal(HirLiteral::Float(_)) => true,
        HirExprKind::Identifier(n) => float_set.contains(n),
        HirExprKind::Binary { left, right, .. } => {
            expr_is_float(left, float_set) || expr_is_float(right, float_set)
        }
        HirExprKind::Unary { inner, .. } => expr_is_float(inner, float_set),
        HirExprKind::Call { args, .. } => args.iter().any(|a| expr_is_float(a, float_set)),
        HirExprKind::Cast { inner, .. } => expr_is_float(inner, float_set),
        HirExprKind::Field { receiver, .. } => expr_is_float(receiver, float_set),
        HirExprKind::Index(a, i) => {
            expr_is_float(a, float_set) || expr_is_float(i, float_set)
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            items.iter().any(|x| expr_is_float(x, float_set))
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            expr_is_float(condition, float_set)
                || expr_is_float(then_branch, float_set)
                || else_branch
                    .as_ref()
                    .map_or(false, |x| expr_is_float(x, float_set))
        }
        _ => false,
    }
}

fn count_cross_float_ops(expr: &HirExpr, float_set: &HashSet<String>) -> u32 {
    match &expr.kind {
        HirExprKind::Binary { left, right, .. } => {
            let count = if expr_refs_float(left, float_set) && expr_refs_float(right, float_set) {
                1u32
            } else {
                0u32
            };
            count
                + count_cross_float_ops(left, float_set)
                + count_cross_float_ops(right, float_set)
        }
        _ => 0,
    }
}

fn expr_refs_float(expr: &HirExpr, float_set: &HashSet<String>) -> bool {
    match &expr.kind {
        HirExprKind::Identifier(n) => float_set.contains(n),
        HirExprKind::Binary { left, right, .. } => {
            expr_refs_float(left, float_set) || expr_refs_float(right, float_set)
        }
        HirExprKind::Unary { inner, .. } => expr_refs_float(inner, float_set),
        HirExprKind::Call { args, .. } => args.iter().any(|a| expr_refs_float(a, float_set)),
        HirExprKind::Field { receiver, .. } => expr_refs_float(receiver, float_set),
        HirExprKind::Index(a, i) => {
            expr_refs_float(a, float_set) || expr_refs_float(i, float_set)
        }
        HirExprKind::Cast { inner, .. } => expr_refs_float(inner, float_set),
        HirExprKind::Deref(inner) => expr_refs_float(inner, float_set),
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            items.iter().any(|x| expr_refs_float(x, float_set))
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            expr_refs_float(condition, float_set)
                || expr_refs_float(then_branch, float_set)
                || else_branch
                    .as_ref()
                    .map_or(false, |x| expr_refs_float(x, float_set))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_program;

    fn compare(source: &str) -> HashMap<String, ComputeDensity> {
        let tokens = crate::lexer::tokenize(source).unwrap();
        let items = crate::parser::Parser::new(tokens, source).parse_program().unwrap();
        let ast = crate::analysis::density::compute_densities(&items);
        let hir = compute_densities(&lower_program(&items));
        assert_eq!(hir, ast, "HIR/AST density drift");
        hir
    }

    #[test]
    fn float_and_int_txns_match_ast() {
        let source = "let solar: Float = 1.0;\n\
                      let count: Int = 0;\n\
                      node phys [count < 1000][count == 1000] {\n\
                        let ax: Float = solar * 1.0;\n\
                        let sum: Float = ax + solar;\n\
                        ax = ax + sum;\n\
                        count = count + 1;\n\
                        term;\n\
                      };\n\
                      node ints [count < 1000][count == 1000] {\n\
                        let i: Int = count + 1;\n\
                        count = i;\n\
                        term;\n\
                      };";
        let d = compare(source);
        let phys = &d["phys"];
        assert_eq!(phys.float_idents, 2);
        assert!(phys.cross_ops >= 1);
        assert_eq!(d["ints"].float_idents, 0);
        assert_eq!(d["ints"].cross_ops, 0);
        assert_eq!(d["ints"].per_field, 0.0);
    }

    #[test]
    fn const_float_typing_seeds_propagation() {
        let source = "const scale: Float = 2.0;\n\
                      let count: Int = 0;\n\
                      node run [count < 10][count == 10] {\n\
                        let x = scale * 3.0;\n\
                        count = count + 1;\n\
                        term;\n\
                      };";
        let d = compare(source);
        assert_eq!(d["run"].float_idents, 1);
    }
}
