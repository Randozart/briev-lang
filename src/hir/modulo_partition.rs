// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H5 inventory port: modulo-partition detection over typed HIR.
//! Mirror of `analysis::modulo_partition` (AST, still authoritative until
//! the consumer flip). Reuses `ModuloPartition` — the type carries no AST
//! nodes.
//!
//! Coverage mirrors the AST walker exactly: BARE top-level reactive
//! transactions only (the AST iterates `TopLevel::Transaction` directly, so
//! sync-wrapped and export-wrapped copies are excluded), in program order.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::program::{HirCallable, HirCallableKind, HirProgram};
use crate::analysis::modulo_partition::ModuloPartition;
use crate::ast::BinaryOpKind;

pub fn detect_modulo_partition(program: &HirProgram) -> Option<ModuloPartition> {
    let reactive: Vec<&HirCallable> = program
        .callables
        .iter()
        .filter(|c| {
            matches!(c.kind, HirCallableKind::Transaction)
                && c.owner.is_none()
                && !c.exported
                && c.sync_domains.is_empty()
                && c.is_reactive
        })
        .collect();
    if reactive.len() < 2 {
        return None;
    }
    let first_pre = pre_of(reactive[0])?;
    let (counter, divisor) = extract_mod_info(first_pre)?;
    let mut cases: Vec<(i64, String)> = Vec::new();
    for t in &reactive {
        let residue = extract_mod_guard(pre_of(t)?, &counter, divisor)?;
        cases.push((residue, t.name.clone()));
    }
    if cases.len() != reactive.len() {
        return None;
    }
    Some(ModuloPartition {
        counter,
        divisor,
        cases,
    })
}

fn pre_of(c: &HirCallable) -> Option<&HirExpr> {
    c.contract.as_ref().map(|ct| &ct.pre)
}

fn extract_mod_info(expr: &HirExpr) -> Option<(String, i64)> {
    let HirExprKind::Binary { op, left, right } = &expr.kind else {
        return None;
    };
    match op {
        BinaryOpKind::Mod => {
            let HirExprKind::Identifier(counter) = &left.kind else {
                return None;
            };
            let HirExprKind::Literal(HirLiteral::Int(divisor)) = &right.kind else {
                return None;
            };
            Some((counter.clone(), *divisor))
        }
        BinaryOpKind::Eq => extract_mod_info(left).or_else(|| extract_mod_info(right)),
        BinaryOpKind::And => extract_mod_info(left).or_else(|| extract_mod_info(right)),
        _ => None,
    }
}

fn extract_mod_guard(expr: &HirExpr, counter: &str, divisor: i64) -> Option<i64> {
    let HirExprKind::Binary { op, left, right } = &expr.kind else {
        return None;
    };
    match op {
        BinaryOpKind::And => {
            extract_mod_guard(left, counter, divisor).or_else(|| extract_mod_guard(right, counter, divisor))
        }
        BinaryOpKind::Eq => {
            let HirExprKind::Binary {
                op: BinaryOpKind::Mod,
                left: mod_counter,
                right: mod_divisor,
            } = &left.kind
            else {
                return None;
            };
            let HirExprKind::Identifier(name) = &mod_counter.kind else {
                return None;
            };
            let HirExprKind::Literal(HirLiteral::Int(div)) = &mod_divisor.kind else {
                return None;
            };
            if name != counter || *div != divisor {
                return None;
            }
            match &right.kind {
                HirExprKind::Literal(HirLiteral::Int(residue)) => Some(*residue),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_program;

    fn compare(source: &str) -> Option<ModuloPartition> {
        let tokens = crate::lexer::tokenize(source).unwrap();
        let items = crate::parser::Parser::new(tokens, source).parse_program().unwrap();
        let ast = crate::analysis::modulo_partition::detect_modulo_partition(&items);
        let hir = detect_modulo_partition(&lower_program(&items));
        assert_eq!(hir, ast, "HIR/AST modulo-partition drift");
        hir
    }

    #[test]
    fn detects_eight_way_partition() {
        let mut source = String::from("let count: Int = 0; let total: Int = 64;\n");
        for n in 0..8 {
            source.push_str(&format!(
                "node t{n} [count < total && count % 8 == {n}][count == total] {{ count = count + 1; term; }};\n"
            ));
        }
        let p = compare(&source).expect("partition");
        assert_eq!(p.counter, "count");
        assert_eq!(p.divisor, 8);
        assert_eq!(p.cases.len(), 8);
        for (i, (residue, name)) in p.cases.iter().enumerate() {
            assert_eq!(*residue, i as i64);
            assert_eq!(name, &format!("t{i}"));
        }
    }

    #[test]
    fn single_txn_and_divergent_are_not_partitions() {
        assert!(compare("let count: Int = 0;\nnode t0 [count % 8 == 0][true] { term; };").is_none());
        assert!(compare(
            "let count: Int = 0; let total: Int = 8;\n\
             node t0 [count % 8 == 0][true] { term; };\n\
             node t1 [count < total][true] { term; };"
        )
        .is_none());
        assert!(compare(
            "let count: Int = 0;\n\
             node t0 [count % 8 == 0][true] { term; };\n\
             node t1 [count % 4 == 1][true] { term; };"
        )
        .is_none());
    }
}
