// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Independent serialization into the checked assignment semantics. Keep this
//! separate from direct Word emission so per-program proofs can catch drift.

use crate::ast::{BinaryOpKind as Op, Definition, Expr, Statement, UnaryOpKind};
use std::collections::HashMap;

/// Serialize an already admitted definition, retaining mutable local slots
/// instead of the direct emitter's fresh SSA names on each assignment.
pub(super) fn declarations(d: &Definition, binders: &str, args: &str) -> Result<String, String> {
    let parameters: HashMap<String, usize> = d
        .parameters
        .iter()
        .enumerate()
        .map(|(index, (name, _))| (name.clone(), index + 1))
        .collect();
    let pre = assertion(&d.contract.pre_condition, &parameters)?;
    let mut post_bindings = parameters.clone();
    post_bindings.insert("term".into(), 0);
    let post = assertion(&d.contract.post_condition, &post_bindings)?;
    let code = statements(&d.body, parameters)?;
    let mut initial = "0".to_string();
    for index in (0..d.parameters.len()).rev() {
        initial = format!("if name = {} then arg_{index} else {initial}", index + 1);
    }
    Ok(format!(
        r#"def code : BrievIntegerVC.Program := [{code}]
def model_pre : BrievIntegerVC.Assertion := {pre}
def model_post : BrievIntegerVC.Assertion := {post}
def initial{binders} : BrievIntegerVC.State := fun name => {initial}
def reference_program{binders} : Word := BrievIntegerVC.execute (initial{args}) code 0

theorem vc_checked : ∀ s, BrievIntegerVC.holds s
    (.implies model_pre (BrievIntegerVC.wp code model_post)) := by
  intro s
  simp_all [model_pre, model_post, code, BrievIntegerVC.wp,
    BrievIntegerVC.substAssertion, BrievIntegerVC.subst, BrievIntegerVC.holds,
    BrievIntegerVC.eval, BitVec.add_sub_cancel, BitVec.sub_add_cancel]
  <;> bv_omega
"#
    ))
}

fn statements(body: &[Statement], mut bindings: HashMap<String, usize>) -> Result<String, String> {
    let parameter_count = bindings.len();
    let mut code = Vec::new();
    for (index, statement) in body.iter().enumerate() {
        match statement {
            Statement::Let {
                name,
                expr: Some(expr),
                ..
            } => {
                let value = expression(expr, &bindings)?;
                let slot = bindings.len() + 1;
                if bindings.insert(name.clone(), slot).is_some() {
                    return Err("model serialization does not allow shadowing".into());
                }
                code.push(format!("({slot}, {value})"));
            }
            Statement::Assign(Expr::Identifier(name), expr) => {
                let slot = *bindings.get(name).ok_or("unbound model assignment")?;
                if slot <= parameter_count {
                    return Err("model parameters are immutable".into());
                }
                code.push(format!("({slot}, {})", expression(expr, &bindings)?));
            }
            Statement::Term(Some(expr)) | Statement::Tail(expr) if index + 1 == body.len() => {
                code.push(format!("(0, {})", expression(expr, &bindings)?));
                return Ok(code.join(", "));
            }
            _ => return Err("unsupported assignment-model statement".into()),
        }
    }
    Err("assignment model requires a final return".into())
}

fn expression(expr: &Expr, bindings: &HashMap<String, usize>) -> Result<String, String> {
    match expr {
        Expr::Decimal(value) => Ok(format!("(.literal ({} : Word))", *value as u64)),
        Expr::Identifier(name) => bindings
            .get(name)
            .map(|slot| format!("(.variable {slot})"))
            .ok_or_else(|| format!("unbound model word: {name}")),
        Expr::UnaryOp(UnaryOpKind::Neg, value) => Ok(format!(
            "(.sub (.literal 0) {})",
            expression(value, bindings)?
        )),
        Expr::BinaryOp(op @ (Op::Add | Op::Sub | Op::Mul), left, right) => {
            let constructor = match op {
                Op::Add => "add",
                Op::Sub => "sub",
                _ => "mul",
            };
            Ok(format!(
                "(.{constructor} {} {})",
                expression(left, bindings)?,
                expression(right, bindings)?
            ))
        }
        _ => Err("unsupported assignment-model expression".into()),
    }
}

fn assertion(expr: &Expr, bindings: &HashMap<String, usize>) -> Result<String, String> {
    match expr {
        Expr::Bool(value) => Ok(format!("(.boolean {value})")),
        Expr::UnaryOp(UnaryOpKind::Not, inner) => {
            Ok(format!("(.not {})", assertion(inner, bindings)?))
        }
        Expr::BinaryOp(op @ (Op::And | Op::Or), left, right) => {
            let constructor = if *op == Op::And { "and" } else { "or" };
            Ok(format!(
                "(.{constructor} {} {})",
                assertion(left, bindings)?,
                assertion(right, bindings)?
            ))
        }
        Expr::BinaryOp(
            op @ (Op::Eq | Op::Neq | Op::Lt | Op::Le | Op::Gt | Op::Ge),
            left,
            right,
        ) => relation(
            op,
            &expression(left, bindings)?,
            &expression(right, bindings)?,
        ),
        _ => Err("unsupported assignment-model assertion".into()),
    }
}

fn relation(op: &Op, left: &str, right: &str) -> Result<String, String> {
    match op {
        Op::Eq => Ok(format!("(.equal {left} {right})")),
        Op::Neq => Ok(format!("(.not (.equal {left} {right}))")),
        Op::Lt => Ok(format!("(.signedLt {left} {right})")),
        Op::Gt => Ok(format!("(.signedLt {right} {left})")),
        Op::Le => Ok(format!("(.not (.signedLt {right} {left}))")),
        Op::Ge => Ok(format!("(.not (.signedLt {left} {right}))")),
        _ => Err("unsupported assignment-model relation".into()),
    }
}
