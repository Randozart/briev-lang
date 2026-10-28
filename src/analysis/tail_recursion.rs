// Copyright 2026 Randy Smits-Schreuder Goedheijt
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Direct self-tail-recursion analysis (TCO-safe sret prerequisite).

use crate::ast::{Expr, Statement, TopLevel};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailRecursionClass {
    NotRecursive,
    TailRecursive,
    NonTailRecursive,
}

#[derive(Debug, Clone, Default)]
pub struct TailRecursionPlan {
    pub defns: BTreeMap<String, TailRecursionClass>,
}

impl TailRecursionPlan {
    pub fn class(&self, name: &str) -> TailRecursionClass {
        self.defns
            .get(name)
            .copied()
            .unwrap_or(TailRecursionClass::NotRecursive)
    }
}

/// Definitions eligible for direct loop lowering. This is deliberately
/// narrower than `TailRecursive`: the lowering needs an explicit, stable
/// parameter representation and must not change cleanup semantics.
#[derive(Debug, Clone, Default)]
pub struct TailLoopPlan {
    pub defns: BTreeSet<String>,
}

impl TailLoopPlan {
    pub fn is_lowerable(&self, name: &str) -> bool {
        self.defns.contains(name)
    }
}

pub fn build_tail_loop_plan(items: &[TopLevel]) -> TailLoopPlan {
    let tail = build_tail_recursion_plan(items);
    let mut defns = BTreeSet::new();
    for item in items {
        let TopLevel::Definition(d) = item else {
            continue;
        };
        if tail.class(&d.name) != TailRecursionClass::TailRecursive {
            continue;
        }
        if !loop_lowerable_signature(d) || has_cleanup_or_boundary(&d.body) {
            continue;
        }
        let calls = tail_self_calls_in_body(&d.body, &d.name);
        if !tail_calls_match_arity(&calls, d.parameters.len()) {
            continue;
        }
        defns.insert(d.name.clone());
    }
    TailLoopPlan { defns }
}

fn loop_lowerable_signature(d: &crate::ast::Definition) -> bool {
    d.type_params.is_empty()
        && d.variadic_param.is_none()
        && d.parameters.iter().all(|(_, ty)| loop_storable_type(ty))
}

fn tail_calls_match_arity(calls: &[Vec<Expr>], arity: usize) -> bool {
    !calls.is_empty() && calls.iter().all(|args| args.len() == arity)
}

/// The backend maps boxed values, pointers, enum handles, and `i64` all to
/// one i64 slot. Aggregates are rejected until their slot representation is
/// proven (Slice/Dyn/Vector).
fn loop_storable_type(ty: &crate::ast::Type) -> bool {
    use crate::ast::Type;
    !matches!(
        ty,
        Type::Vector(_, _)
            | Type::Dyn(_)
            | Type::Task(_)
            | Type::TypeVar(_)
            | Type::Width(_)
            | Type::Number(_)
            | Type::LayoutPtr(_)
    ) && !matches!(ty, Type::Custom(name) if name == "Slice")
}

fn has_cleanup_or_boundary(stmts: &[Statement]) -> bool {
    let mut bad = stmts.iter().any(|stmt| match stmt {
        Statement::Defer(_) | Statement::Rollback(_) | Statement::EndProgram(_) => true,
        Statement::Guarded(_, body)
        | Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Mutex(body)
        | Statement::Foreach { body, .. } => has_cleanup_or_boundary(body),
        _ => false,
    });
    if bad {
        return true;
    }
    // Match arms are expressions. A defer/rollback/endprogram hidden in an
    // arm block has the same per-frame cleanup/boundary semantics.
    crate::ast::visit::walk_stmts(stmts, &mut |expr| {
        if let crate::ast::Expr::Block(arm_stmts) = expr {
            bad |= has_cleanup_or_boundary(arm_stmts);
        }
    });
    bad
}

fn tail_self_calls_in_body(body: &[Statement], name: &str) -> Vec<Vec<Expr>> {
    let mut out = Vec::new();
    collect_tail_self_calls(body, name, &mut out);
    out
}

fn collect_tail_self_calls(body: &[Statement], name: &str, out: &mut Vec<Vec<Expr>>) {
    for stmt in body {
        match stmt {
            Statement::Term(Some(e)) | Statement::Tail(e) => {
                collect_tail_self_calls_expr(e, name, out);
            }
            // `when cond { term f(...); }` is a guarded tail path; the
            // recursive call still exits the frame on that edge.
            Statement::Guarded(_, inner) | Statement::Block(inner) => {
                collect_tail_self_calls(inner, name, out);
            }
            _ => {}
        }
    }
}

fn collect_tail_self_calls_expr(expr: &Expr, name: &str, out: &mut Vec<Vec<Expr>>) {
    match expr {
        Expr::Call(callee, args, _) if callee == name => out.push(args.clone()),
        Expr::Match(_, arms) => {
            for arm in arms {
                collect_tail_self_calls_expr(&arm.body, name, out);
            }
        }
        Expr::Block(stmts) => {
            for stmt in stmts.iter().rev() {
                match stmt {
                    Statement::Term(Some(e)) | Statement::Tail(e) | Statement::Expression(e) => {
                        collect_tail_self_calls_expr(e, name, out);
                        break;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

pub fn build_tail_recursion_plan(items: &[TopLevel]) -> TailRecursionPlan {
    use crate::ast::visit::walk_stmts;
    let mut defns = BTreeMap::new();
    for item in items {
        let TopLevel::Definition(d) = item else {
            continue;
        };
        let mut total = 0usize;
        walk_stmts(&d.body, &mut |e| {
            if let Expr::Call(name, _, _) = e {
                if name == &d.name {
                    total += 1;
                }
            }
        });
        let class = if total == 0 {
            TailRecursionClass::NotRecursive
        } else {
            let tail = tail_calls_in_body(&d.body, &d.name);
            if tail == total {
                TailRecursionClass::TailRecursive
            } else {
                TailRecursionClass::NonTailRecursive
            }
        };
        defns.insert(d.name.clone(), class);
    }
    TailRecursionPlan { defns }
}

fn tail_calls_in_body(body: &[Statement], name: &str) -> usize {
    let mut total = 0;
    for stmt in body {
        match stmt {
            Statement::Term(Some(e)) | Statement::Tail(e) => {
                total += tail_calls_in_expr(e, name);
            }
            Statement::Guarded(_, inner) | Statement::Block(inner) => {
                total += tail_calls_in_body(inner, name);
            }
            _ => {}
        }
    }
    total
}

fn tail_calls_in_expr(expr: &Expr, name: &str) -> usize {
    match expr {
        Expr::Call(callee, _, _) if callee == name => 1,
        Expr::Match(_, arms) => arms
            .iter()
            .map(|arm| tail_calls_in_expr(&arm.body, name))
            .sum(),
        Expr::Block(stmts) => tail_calls_in_block(stmts, name),
        _ => 0,
    }
}

fn tail_calls_in_block(stmts: &[Statement], name: &str) -> usize {
    for stmt in stmts.iter().rev() {
        match stmt {
            Statement::Term(Some(e)) | Statement::Tail(e) | Statement::Expression(e) => {
                return tail_calls_in_expr(e, name);
            }
            _ => {}
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<TopLevel> {
        let toks = crate::lexer::tokenize(src).expect("tokenize");
        crate::parser::Parser::new(toks, src)
            .parse_program()
            .expect("parse")
    }

    #[test]
    fn direct_tail_recursion_is_classified() {
        let items = parse("defn f(n: Int) -> Int { when n <= 0 { term 0; }; term f(n - 1); };\n");
        let plan = build_tail_recursion_plan(&items);
        assert_eq!(plan.class("f"), TailRecursionClass::TailRecursive);
    }

    #[test]
    fn non_tail_recursion_is_classified() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn f(n: Int) -> Int { when n <= 0 { term 0; }; let r: R = Ok(n); term match r { Ok(x) => f(x - 1) + 1, Err(_) => f(n - 1) }; };\n",
        );
        let plan = build_tail_recursion_plan(&items);
        assert_eq!(plan.class("f"), TailRecursionClass::NonTailRecursive);
    }

    #[test]
    fn mutual_and_non_recursive_are_not_tail() {
        let items = parse(
            "defn a(n: Int) -> Int { term b(n); };\n\
             defn b(n: Int) -> Int { term a(n); };\n\
             defn c(n: Int) -> Int { term n; };\n",
        );
        let plan = build_tail_recursion_plan(&items);
        assert_eq!(plan.class("a"), TailRecursionClass::NotRecursive);
        assert_eq!(plan.class("b"), TailRecursionClass::NotRecursive);
        assert_eq!(plan.class("c"), TailRecursionClass::NotRecursive);
    }
    #[test]
    fn direct_tail_recursion_is_loop_lowerable() {
        let items = parse("defn f(n: Int) -> Int { when n <= 0 { term 0; }; term f(n - 1); };\n");
        let plan = build_tail_loop_plan(&items);
        assert!(plan.is_lowerable("f"));
    }

    #[test]
    fn guarded_tail_recursion_is_loop_lowerable() {
        let items = parse(
            "defn f(n: Int) -> Int { when n <= 0 { term 0; }; when n > 0 { term f(n - 1); }; };\n",
        );
        let plan = build_tail_loop_plan(&items);
        assert!(plan.is_lowerable("f"), "a term inside a when-block is still a tail edge");
    }

    #[test]
    fn cleanup_and_generic_bodies_are_not_loop_lowerable() {
        let items = parse(
            "defn f(n: Int) -> Int { defer { }; term f(n - 1); };\n             defn g<T>(n: Int) -> Int { term g(n - 1); };\n",
        );
        let plan = build_tail_loop_plan(&items);
        assert!(!plan.is_lowerable("f"));
        assert!(!plan.is_lowerable("g"));
    }

    #[test]
    fn cleanup_in_a_match_arm_is_not_loop_lowerable() {
        let items = parse(
            "defn f(n: Int) -> Int { term match n { 0 => { defer { }; term 0; }, _ => f(n - 1) }; };\n",
        );
        let plan = build_tail_loop_plan(&items);
        assert!(!plan.is_lowerable("f"));
    }

    #[test]
    fn non_tail_recursion_is_not_loop_lowerable() {
        let items = parse("defn f(n: Int) -> Int { term f(n - 1) + 1; };\n");
        let plan = build_tail_loop_plan(&items);
        assert!(!plan.is_lowerable("f"));
    }
}
