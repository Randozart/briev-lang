// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b — recursive-carrier analysis over HIR.
//! Mirror of `analysis::tail_carrier` for inline nested enum state.

use super::enum_repr::HirEnumRepresentationPlan;
use super::expr_walk::walk_stmts;
use super::expr::{HirExpr, HirExprKind};
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{HirCallable, HirCallableKind, HirParam, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use super::tail_recursion::build_tail_recursion_plan;
use crate::analysis::destination_model::{
    CallableId, CallableNamespace, DestinationId, GeneralDestinationClass, ProofReason,
    ProofStatus, RecursiveCarrierArgClass, RecursiveCarrierPlan, RecursiveCarrierValue,
    SemanticValueId, SemanticValueKind,
};
use crate::analysis::enum_repr::EnumRepresentation;
use crate::analysis::tail_recursion::TailRecursionClass;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub fn build_recursive_carriers(
    program: &HirProgram,
) -> BTreeMap<CallableId, RecursiveCarrierPlan> {
    let tail = build_tail_recursion_plan(program);
    let enum_plan = super::enum_repr::build_enum_representation_plan(program);
    let mut defns = BTreeMap::new();
    for c in program.callables.iter().filter(|c| is_top_defn(c)) {
        if tail.class(&c.name) != TailRecursionClass::TailRecursive {
            continue;
        }
        let self_calls = tail_self_calls_in_body(&c.body.statements, &c.name);
        if self_calls.is_empty() {
            continue;
        }
        let callable = CallableId {
            namespace: CallableNamespace::Definition,
            name: c.name.clone(),
        };
        let carriers = carrier_parameters(&callable, c, &self_calls, &enum_plan);
        if !carriers.is_empty() {
            defns.insert(
                callable.clone(),
                RecursiveCarrierPlan {
                    callable,
                    carriers,
                    proof: carrier_proof(),
                },
            );
        }
    }
    defns
}

fn is_top_defn(c: &HirCallable) -> bool {
    matches!(c.kind, HirCallableKind::Defn) && c.owner.is_none() && !c.trait_default && !c.exported
}

fn carrier_parameters(
    callable: &CallableId,
    c: &HirCallable,
    self_calls: &[Vec<HirExpr>],
    enum_plan: &HirEnumRepresentationPlan,
) -> Vec<RecursiveCarrierValue> {
    let sret_locals = sret_local_bindings(c, enum_plan);
    c.params
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
                        let HirExprKind::Identifier(local) = &args[param_index].kind else {
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
                param_name: param.name.clone(),
                result_local,
                sret_callee,
                requires_backedge_copy: classes
                    .contains(&RecursiveCarrierArgClass::InlineNested),
                arg_classes: classes,
                carrier: DestinationId {
                    owner: callable.name.clone(),
                    path: format!("recursive/{param_index}/{}", param.name),
                },
                class: GeneralDestinationClass::StableCarrier,
                proof: carrier_proof(),
            })
        })
        .collect()
}

fn carrier_proof() -> ProofStatus {
    ProofStatus::Proven {
        reasons: BTreeSet::from([ProofReason::StableCarrierLifetime]),
    }
}

fn classify_carrier_arg(
    args: &[HirExpr],
    param_index: usize,
    param: &HirParam,
    sret_locals: &HashMap<String, (String, String)>,
) -> RecursiveCarrierArgClass {
    let Some(HirExpr { kind: HirExprKind::Identifier(local), .. }) = args.get(param_index) else {
        return RecursiveCarrierArgClass::Unsupported;
    };
    if local == &param.name {
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

fn sret_local_bindings(
    c: &HirCallable,
    enum_plan: &HirEnumRepresentationPlan,
) -> HashMap<String, (String, String)> {
    let mut direct = HashMap::new();
    for stmt in &c.body.statements {
        if let HirStmtKind::Let {
            name,
            expr: Some(HirExpr {
                kind: HirExprKind::Call { callee, .. },
                ..
            }),
            ..
        } = &stmt.kind
        {
            if enum_plan.defn_class(callee) == EnumRepresentation::ReturnedSmallCandidate {
                direct.insert(name.clone(), (name.clone(), callee.clone()));
            }
        }
    }
    let mut bound = direct.clone();
    walk_stmts(&c.body.statements, &mut |e| {
        if let HirExprKind::Match { scrutinee, arms } = &e.kind {
            let HirExprKind::Identifier(scrut_local) = &scrutinee.kind else {
                return;
            };
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
    });
    bound
}

fn collect_pattern_bindings(pat: &HirPattern, out: &mut impl FnMut(&String)) {
    match &pat.kind {
        HirPatternKind::Binding(name) | HirPatternKind::TypedBinding(name, _) => out(name),
        HirPatternKind::EnumVariant(_, pats)
        | HirPatternKind::Tuple(pats)
        | HirPatternKind::Multi(pats) => {
            for p in pats {
                collect_pattern_bindings(p, out);
            }
        }
        _ => {}
    }
}

fn tail_self_calls_in_body(body: &[HirStmt], name: &str) -> Vec<Vec<HirExpr>> {
    let mut out = Vec::new();
    for stmt in body {
        if let HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e) = &stmt.kind {
            tail_self_calls_in_expr(e, name, &mut out);
        }
    }
    out
}

fn tail_self_calls_in_expr(expr: &HirExpr, name: &str, out: &mut Vec<Vec<HirExpr>>) {
    match &expr.kind {
        HirExprKind::Call { callee, args, .. } if callee == name => out.push(args.clone()),
        HirExprKind::Match { arms, .. } => {
            for arm in arms {
                tail_self_calls_in_expr(&arm.body, name, out);
            }
        }
        HirExprKind::Block(stmts) => {
            for stmt in stmts.iter().rev() {
                match &stmt.kind {
                    HirStmtKind::Term(Some(e))
                    | HirStmtKind::Tail(e)
                    | HirStmtKind::Expression(e) => {
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
