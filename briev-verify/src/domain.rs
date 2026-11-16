// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Admission boundary for the signed 64-bit body model. Source consumers must
//! resolve declarations using their actual target and operator environment.

use crate::ast::{BinaryOpKind, Expr, OutputType, Statement, TopLevel, Type, visit};
use std::collections::HashSet;

/// Resolved scalar semantics, supplied by the compiler rather than type names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarModel {
    SignedWord {
        bits: u64,
    },
    /// 2026-11-15 (plan 2026-11-15-value-summaries.md phase 1): Bool is a
    /// distinct admitted model, not a 64-bit word. The symbolic evaluator
    /// (`SymbolicValue::bool_literal`) and the SMT encoder (native SMT-LIB
    /// Boolean, `==`/`!=` on same-sort operands, `Not`) already handle it
    /// end-to-end; admitting it here is a gate fix, not a capability fix.
    Bool,
    Unsupported,
}

impl ScalarModel {
    /// The current arithmetic engine models exactly signed wrapping 64-bit
    /// words, plus Bool as a distinct admitted sort.
    pub fn admitted(self) -> bool {
        matches!(self, Self::SignedWord { bits: 64 } | Self::Bool)
    }
}

pub type TypeResolver<'a> = dyn Fn(&Type) -> ScalarModel + 'a;

/// Keep globals outside the model until their initialization and state semantics
/// are represented. Rejecting a shadowed name is conservative, never a proof.
pub(crate) fn global_names(items: &[TopLevel]) -> HashSet<String> {
    let mut names = HashSet::new();
    for item in items {
        match item {
            TopLevel::Constant(c) => {
                names.insert(c.name.clone());
            }
            TopLevel::StateDecl(s) => {
                names.insert(s.name.clone());
            }
            TopLevel::CompileTimeLet(name, _) | TopLevel::CompileTimeConst(name, _) => {
                names.insert(name.clone());
            }
            TopLevel::Statement(statement) => {
                if let Statement::Let {
                    name,
                    names: bindings,
                    ..
                } = &**statement
                {
                    names.insert(name.clone());
                    names.extend(bindings.iter().cloned());
                }
            }
            TopLevel::Export(e) => names.extend(global_names(std::slice::from_ref(&*e.inner))),
            TopLevel::SyncGroup { item, .. } => {
                names.extend(global_names(std::slice::from_ref(&**item)))
            }
            _ => {}
        }
    }
    names
}

/// Inspect the original declaration before loop extraction or helper inlining
/// can erase types. Both contract targets and helper candidates use this gate.
pub(crate) fn unsupported_declaration(
    item: &TopLevel,
    resolve: &TypeResolver<'_>,
    globals: &HashSet<String>,
) -> Option<String> {
    let (parameters, output, outputs, body, contract, generic) = match item {
        TopLevel::Definition(d) => (
            &d.parameters,
            &d.output_type,
            &d.outputs,
            &d.body,
            &d.contract,
            !d.type_params.is_empty() || d.variadic_param.is_some(),
        ),
        TopLevel::Transaction(t) => (
            &t.parameters,
            &t.output_type,
            &t.outputs,
            &t.body,
            &t.contract,
            !t.type_params.is_empty() || t.variadic_param.is_some(),
        ),
        TopLevel::Export(e) => return unsupported_declaration(&e.inner, resolve, globals),
        TopLevel::SyncGroup { item, .. } => return unsupported_declaration(item, resolve, globals),
        _ => return None,
    };
    if generic {
        return Some("generic or variadic declaration has no scalar arithmetic model".into());
    }
    if outputs.len() > 1 {
        return Some("multiple results are outside the scalar model".into());
    }
    for (_, ty) in parameters {
        if let Some(reason) = check_type(ty, resolve) {
            return Some(reason);
        }
    }
    for ty in outputs {
        if let Some(reason) = check_type(ty, resolve) {
            return Some(reason);
        }
    }
    if let Some(output) = output {
        match output {
            OutputType::Single(ty) => {
                if let Some(reason) = check_type(ty, resolve) {
                    return Some(reason);
                }
            }
            _ => return Some("structured result is outside the scalar model".into()),
        }
    }
    if let Some(reason) = local_types(body, resolve) {
        return Some(reason);
    }
    declaration_expressions(contract, body, globals)
}

/// Scan both contract halves and the complete body, so an unused floating
/// expression or a global reference cannot disappear during simplification.
fn declaration_expressions(
    contract: &crate::ast::Contract,
    body: &[Statement],
    globals: &HashSet<String>,
) -> Option<String> {
    let mut reason = None;
    let mut inspect = |expr: &Expr| {
        if let Some(why) = expression_node_reason(expr) {
            reason = Some(why);
        }
        if let Expr::Identifier(name) = expr {
            if globals.contains(name) {
                reason = Some(format!("global {name} is outside the state-free model"));
            }
        }
    };
    visit::walk_expr(&contract.pre_condition, &mut inspect);
    visit::walk_expr(&contract.post_condition, &mut inspect);
    visit::walk_stmts(body, &mut inspect);
    reason
}

/// Recheck generated obligations too: invariant extraction must not introduce
/// exceptional arithmetic or floating literals after declaration admission.
pub(crate) fn unsupported_expression(expr: &Expr) -> Option<String> {
    let mut reason = None;
    visit::walk_expr(expr, &mut |node| {
        if let Some(why) = expression_node_reason(node) {
            reason = Some(why);
        }
    });
    reason
}

pub(crate) fn expression_node_reason(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Float(_) => Some("floating literal is outside the integer model".into()),
        Expr::BinaryOp(BinaryOpKind::Div | BinaryOpKind::Mod, _, divisor)
            if !matches!(&**divisor, Expr::Decimal(value) if *value != 0 && *value != -1) =>
            Some("division/remainder needs a nonzero, non-minus-one constant divisor; exceptional domains are not modeled".into()),
        _ => None,
    }
}

// Return Some on rejection (rather than Result) to match the prover's admission gates.
fn check_type(ty: &Type, resolve: &TypeResolver<'_>) -> Option<String> {
    if resolve(ty).admitted() {
        return None;
    }
    Some(format!(
        "type {ty:?} is outside the signed 64-bit word model"
    ))
}

fn local_types(body: &[Statement], resolve: &TypeResolver<'_>) -> Option<String> {
    for statement in body {
        let reason = match statement {
            Statement::Let { ty: Some(ty), .. } => check_type(ty, resolve),
            Statement::Guarded(_, body)
            | Statement::Foreach { body, .. }
            | Statement::Block(body)
            | Statement::SyncBlock(body)
            | Statement::Defer(body) => local_types(body, resolve),
            _ => None,
        };
        if reason.is_some() {
            return reason;
        }
    }
    None
}

#[cfg(kani)]
mod proofs {
    use super::ScalarModel;

    /// Every machine width is admitted iff it is the implemented word width.
    #[kani::proof]
    fn scalar_admission_is_exact() {
        let bits: u64 = kani::any();
        assert_eq!(ScalarModel::SignedWord { bits }.admitted(), bits == 64);
        assert!(!ScalarModel::Unsupported.admitted());
        kani::cover!(bits == 64);
        kani::cover!(bits != 64);
    }
}
