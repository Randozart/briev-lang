// Copyright 2026 Randy Smits-Schreuder Goedheijt
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Direct self-tail-recursion analysis (TCO-safe sret prerequisite).

use crate::ast::{Expr, Statement, TopLevel};
use std::collections::BTreeMap;

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

pub fn build_tail_recursion_plan(items: &[TopLevel]) -> TailRecursionPlan {
    use crate::ast::visit::walk_stmts;
    let mut defns = BTreeMap::new();
    for item in items {
        let TopLevel::Definition(d) = item else { continue; };
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
        if let Statement::Term(Some(e)) | Statement::Tail(e) = stmt {
            total += tail_calls_in_expr(e, name);
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
}
