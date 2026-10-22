// Copyright 2026 Randy Smits-Schreuder Goedheijt
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Loop-carried ownership analysis for inline nested enum state.
//!
//! A direct self-tail-recursive defn needs a carrier slot when a recursive
//! argument is a local bound to an sret result whose layout contains an inline
//! nested payload. That local is a temporary; a recursive callee must not use
//! it as persistent state.

use crate::analysis::enum_repr::{build_enum_representation_plan, EnumRepresentation};
use crate::analysis::tail_recursion::{
    build_tail_recursion_plan, TailRecursionClass,
};
use crate::ast::{Expr, Statement, TopLevel};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarrierArgClass {
    InlineNested,
    ExistingCarrier,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierParam {
    /// Zero-based index of the recursive-call argument / defn parameter.
    pub param_index: usize,
    pub param_name: String,
    /// Local bound to the sret result in at least one tail arm.
    pub result_local: String,
    /// Sret callee producing the temporary result.
    pub sret_callee: String,
}

#[derive(Debug, Clone, Default)]
pub struct CarrierInfo {
    pub carriers: Vec<CarrierParam>,
}

#[derive(Debug, Clone, Default)]
pub struct TailCarrierPlan {
    pub defns: BTreeMap<String, CarrierInfo>,
}

impl TailCarrierPlan {
    pub fn info(&self, name: &str) -> Option<&CarrierInfo> {
        self.defns.get(name)
    }
}

pub fn build_tail_carrier_plan(items: &[TopLevel]) -> TailCarrierPlan {
    let tail = build_tail_recursion_plan(items);
    let enum_plan = build_enum_representation_plan(items);
    let mut defns = BTreeMap::new();

    for item in items {
        let TopLevel::Definition(d) = item else { continue };
        if tail.class(&d.name) != TailRecursionClass::TailRecursive {
            continue;
        }
        let self_calls = tail_self_calls_in_body(&d.body, &d.name);
        if std::env::var_os("BRIEV_DIAG_TAIL_CARRIER").is_some() {
            eprintln!(
                "diag carrier: {} self_calls={:?} enum_plan={:?}",
                d.name, self_calls, enum_plan
            );
        }
        if self_calls.is_empty() {
            continue;
        }
        let mut carriers = Vec::new();
        for (param_index, param) in d.parameters.iter().enumerate() {
            let mut classes = Vec::new();
            let mut result_local = String::new();
            let mut sret_callee = String::new();
            let sret_locals = sret_local_bindings(d, &enum_plan);
            if std::env::var_os("BRIEV_DIAG_TAIL_CARRIER").is_some() {
                eprintln!("diag carrier locals: {:?} params={:?}", sret_locals, d.parameters);
            }
            for args in &self_calls {
                let Some(arg) = args.get(param_index) else {
                    classes.push(CarrierArgClass::Unsupported);
                    continue;
                };
                let class = match arg {
                    Expr::Identifier(local) if *local == param.0 => {
                        CarrierArgClass::ExistingCarrier
                    }
                    Expr::Identifier(local) => {
                        let Some((sret_local, callee)) = sret_locals.get(local) else {
                            classes.push(CarrierArgClass::Unsupported);
                            continue;
                        };
                        result_local = sret_local.clone();
                        sret_callee = callee.clone();
                        CarrierArgClass::InlineNested
                    }
                    _ => CarrierArgClass::Unsupported,
                };
                classes.push(class);
            }
            if classes.iter().any(|c| *c == CarrierArgClass::Unsupported) {
                continue;
            }
            if !classes
                .iter()
                .any(|c| *c == CarrierArgClass::InlineNested)
            {
                continue;
            }
            carriers.push(CarrierParam {
                param_index,
                param_name: param.0.clone(),
                result_local,
                sret_callee,
            });
        }
        if !carriers.is_empty() {
            defns.insert(d.name.clone(), CarrierInfo { carriers });
        }
    }
    TailCarrierPlan { defns }
}

/// Direct sret-result locals plus locals bound by destructuring a direct
/// sret-result match (`let r = step(...); match r { Ok(next) => ... }`).
fn sret_local_bindings(
    d: &crate::ast::Definition,
    enum_plan: &crate::analysis::enum_repr::EnumRepresentationPlan,
) -> std::collections::HashMap<String, (String, String)> {
    use crate::ast::visit::walk_stmts;
    let mut direct = std::collections::HashMap::new();
    for stmt in &d.body {
        if let Statement::Let {
            name,
            expr: Some(Expr::Call(callee, _, _)),
            ..
        } = stmt
        {
            if enum_plan.defn_class(callee) == EnumRepresentation::ReturnedSmallCandidate {
                direct.insert(name.clone(), (name.clone(), callee.clone()));
            }
        }
    }
    let mut bound = direct.clone();
    walk_stmts(&d.body, &mut |e| {
        if let Expr::Match(scrut, arms) = e {
            if let Expr::Identifier(scrut_local) = scrut.as_ref() {
                let Some((source_local, callee)) = direct.get(scrut_local) else {
                    return;
                };
                for arm in arms {
                    collect_pattern_bindings(&arm.pattern, &mut |binding| {
                        bound
                            .entry(binding.clone())
                            .or_insert_with(|| (source_local.clone(), callee.clone()));
                    });
                }
            }
        }
    });
    bound
}

fn collect_pattern_bindings(pat: &crate::ast::Pattern, out: &mut impl FnMut(&String)) {
    match pat {
        crate::ast::Pattern::Binding(name) => out(name),
        crate::ast::Pattern::TypedBinding(name, _) => out(name),
        crate::ast::Pattern::EnumVariant(_, pats) => {
            for p in pats {
                collect_pattern_bindings(p, out);
            }
        }
        crate::ast::Pattern::Tuple(pats) | crate::ast::Pattern::Multi(pats) => {
            for p in pats {
                collect_pattern_bindings(p, out);
            }
        }
        _ => {}
    }
}

fn tail_self_calls_in_body(body: &[Statement], name: &str) -> Vec<Vec<Expr>> {
    let mut out = Vec::new();
    for stmt in body {
        if let Statement::Term(Some(e)) | Statement::Tail(e) = stmt {
            tail_self_calls_in_expr(e, name, &mut out);
        }
    }
    out
}

fn tail_self_calls_in_expr(expr: &Expr, name: &str, out: &mut Vec<Vec<Expr>>) {
    match expr {
        Expr::Call(callee, args, _) if callee == name => out.push(args.clone()),
        Expr::Match(_, arms) => {
            for arm in arms {
                tail_self_calls_in_expr(&arm.body, name, out);
            }
        }
        Expr::Block(stmts) => {
            for stmt in stmts.iter().rev() {
                match stmt {
                    Statement::Term(Some(e)) | Statement::Tail(e) | Statement::Expression(e) => {
                        tail_self_calls_in_expr(e, name, out);
                        break;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
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
    fn fsm_like_state_is_carrier() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             enum S { A };\n\
             defn step(s: S) -> R { term Ok(1); };\n\
             defn loop(n: Int, state: S, sum: Int) -> Int {\n\
               when n <= 0 { term sum; };\n\
               let r: R = step(state);\n\
               term match r {\n\
                 Ok(next) => loop(n - 1, next, sum + 1),\n\
                 Err(_) => loop(n - 1, state, sum),\n\
               };\n\
             };\n",
        );
        let plan = build_tail_carrier_plan(&items);
        let info = plan.info("loop").expect("carrier info");
        assert_eq!(info.carriers.len(), 1);
        assert_eq!(info.carriers[0].param_index, 1);
        assert_eq!(info.carriers[0].param_name, "state");
        assert_eq!(info.carriers[0].result_local, "r");
        assert_eq!(info.carriers[0].sret_callee, "step");
    }

    #[test]
    fn plain_tail_recursion_has_no_carrier() {
        let items = parse("defn f(n: Int) -> Int { when n <= 0 { term 0; }; term f(n - 1); };\n");
        let plan = build_tail_carrier_plan(&items);
        assert!(plan.info("f").is_none());
    }

    #[test]
    fn non_tail_recursion_has_no_carrier() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             enum S { A };\n\
             defn step(s: S) -> R { term Ok(1); };\n\
             defn loop(n: Int, state: S) -> Int {\n\
               when n <= 0 { term 0; };\n\
               let r: R = step(state);\n\
               term match r { Ok(next) => loop(n - 1, next) + 1, Err(_) => loop(n - 1, state) };\n\
             };\n",
        );
        let plan = build_tail_carrier_plan(&items);
        assert!(plan.info("loop").is_none());
    }
}
