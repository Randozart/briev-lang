// Copyright 2026 Randy Smits-Schreuder Goedheijt
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Recursive-carrier analysis for inline nested enum state.
//!
//! A direct self-tail-recursive defn needs a stable carrier slot when a
//! recursive argument is a local bound to an sret result whose layout contains
//! an inline nested payload. That local is a temporary; a recursive callee
//! must not use it as persistent state.
//!
//! This module emits the unified `RecursiveCarrierPlan` directly. The retired
//! `TailCarrierPlan`/`CarrierInfo`/`CarrierParam` ABI view no longer exists.

use crate::analysis::destination_model::{
    CallableId, CallableNamespace, DestinationId, GeneralDestinationClass, ProofReason,
    ProofStatus, RecursiveCarrierArgClass, RecursiveCarrierPlan, RecursiveCarrierValue,
    SemanticValueId, SemanticValueKind,
};
use crate::analysis::enum_repr::{build_enum_representation_plan, EnumRepresentation};
use crate::analysis::tail_recursion::{build_tail_recursion_plan, TailRecursionClass};
use crate::ast::{Expr, Statement, TopLevel, Type};
use std::collections::BTreeMap;

pub fn build_recursive_carriers(
    items: &[TopLevel],
) -> BTreeMap<CallableId, RecursiveCarrierPlan> {
    let tail = build_tail_recursion_plan(items);
    let enum_plan = build_enum_representation_plan(items);
    let mut defns = BTreeMap::new();

    for item in items {
        let TopLevel::Definition(d) = item else { continue };
        if tail.class(&d.name) != TailRecursionClass::TailRecursive {
            continue;
        }
        let self_calls = tail_self_calls_in_body(&d.body, &d.name);
        if self_calls.is_empty() {
            continue;
        }
        let callable = CallableId {
            namespace: CallableNamespace::Definition,
            name: d.name.clone(),
        };
        let carriers = carrier_parameters(&callable, d, &self_calls, &enum_plan);
        if !carriers.is_empty() {
            defns.insert(callable.clone(), RecursiveCarrierPlan {
                callable,
                carriers,
                proof: carrier_proof(),
            });
        }
    }
    defns
}

fn carrier_parameters(
    callable: &CallableId,
    d: &crate::ast::Definition,
    self_calls: &[Vec<Expr>],
    enum_plan: &crate::analysis::enum_repr::EnumRepresentationPlan,
) -> Vec<RecursiveCarrierValue> {
    let sret_locals = sret_local_bindings(d, enum_plan);
    d.parameters
        .iter()
        .enumerate()
        .filter_map(|(param_index, param)| {
            let classes: Vec<RecursiveCarrierArgClass> = self_calls
                .iter()
                .map(|args| classify_carrier_arg(args, param_index, param, &sret_locals))
                .collect();
            let (result_local, sret_callee) = classes
                .iter()
                .zip(self_calls.iter())
                .find_map(|(class, args)| match class {
                    RecursiveCarrierArgClass::InlineNested => {
                        let Expr::Identifier(local) = &args[param_index] else {
                            return None;
                        };
                        sret_locals.get(local).cloned()
                    }
                    _ => None,
                })?;
            if !carrier_is_approved(&classes) {
                return None;
            }
            Some(RecursiveCarrierValue {
                parameter: SemanticValueId {
                    callable: callable.clone(),
                    kind: SemanticValueKind::Parameter,
                    path: format!("param/{param_index}/{param_index}"),
                    incarnation: 0,
                },
                param_index,
                param_name: param.0.clone(),
                result_local,
                sret_callee,
                requires_backedge_copy: classes
                    .contains(&RecursiveCarrierArgClass::InlineNested),
                arg_classes: classes,
                carrier: DestinationId {
                    owner: callable.name.clone(),
                    path: format!("recursive/{param_index}/{}", param.0),
                },
                class: GeneralDestinationClass::StableCarrier,
                proof: carrier_proof(),
            })
        })
        .collect()
}

fn carrier_proof() -> ProofStatus {
    ProofStatus::Proven {
        reasons: std::collections::BTreeSet::from([
            ProofReason::StableCarrierLifetime,
        ]),
    }
}

fn classify_carrier_arg(
    args: &[Expr],
    param_index: usize,
    param: &(String, Type),
    sret_locals: &std::collections::HashMap<String, (String, String)>,
) -> RecursiveCarrierArgClass {
    let Some(Expr::Identifier(local)) = args.get(param_index) else {
        return RecursiveCarrierArgClass::Unsupported;
    };
    if local == &param.0 {
        return RecursiveCarrierArgClass::ExistingCarrier;
    }
    sret_locals
        .contains_key(local)
        .then_some(RecursiveCarrierArgClass::InlineNested)
        .unwrap_or(RecursiveCarrierArgClass::Unsupported)
}

fn carrier_is_approved(classes: &[RecursiveCarrierArgClass]) -> bool {
    classes
        .iter()
        .all(|class| *class != RecursiveCarrierArgClass::Unsupported)
        && classes.contains(&RecursiveCarrierArgClass::InlineNested)
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

    fn plan_for<'a>(
        plan: &'a BTreeMap<CallableId, RecursiveCarrierPlan>,
        name: &str,
    ) -> Option<&'a RecursiveCarrierPlan> {
        plan.values().find(|p| p.callable.name == name)
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
        let plan = build_recursive_carriers(&items);
        let info = plan_for(&plan, "loop").expect("carrier info");
        assert_eq!(info.carriers.len(), 1);
        assert_eq!(info.carriers[0].param_index, 1);
        assert_eq!(info.carriers[0].param_name, "state");
        assert_eq!(info.carriers[0].result_local, "r");
        assert_eq!(info.carriers[0].sret_callee, "step");
    }

    #[test]
    fn plain_tail_recursion_has_no_carrier() {
        let items = parse("defn f(n: Int) -> Int { when n <= 0 { term 0; }; term f(n - 1); };\n");
        let plan = build_recursive_carriers(&items);
        assert!(plan_for(&plan, "f").is_none());
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
        let plan = build_recursive_carriers(&items);
        assert!(plan_for(&plan, "loop").is_none());
    }
}
