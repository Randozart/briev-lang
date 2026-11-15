// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Restricted, optional contract export. A checked theorem describes the
//! emitted word program; source/native refinement remains a separate obligation.

mod model;

use crate::analysis::verify_bodies::BodyTypeModel;
use crate::ast::{
    BinaryOpKind as Op, Definition, Expr, OutputType, Statement, TopLevel, UnaryOpKind,
};
use briev_verify::domain::ScalarModel;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// Auditable inputs for an external Lean check; generation itself is no proof.
pub struct LeanExport {
    pub program: String,
    pub source: String,
    pub typed_source: String,
    pub manifest: serde_json::Value,
}

/// Parse and typecheck real Briev source, then translate one pure word function.
/// Expanded declarations are retained and hashed to bind resolved imports too.
pub fn export_source(source_path: &str, source: &str, name: &str) -> Result<LeanExport, String> {
    let (items, universe) = crate::library::parse_and_check(source_path, source)?;
    let model = BodyTypeModel::new(&items, &universe, 64);
    let mut definitions = Vec::new();
    collect_definitions(&items, name, &mut definitions);
    if definitions.len() != 1 {
        return Err("export requires exactly one matching definition".into());
    }
    let definition = definitions[0];
    let program = translate(definition, &model)?;
    let typed_source = crate::ast::format_program(&items);
    let manifest = serde_json::json!({
        "schema": "briev-lean-export/v2", "function": name,
        "status": "generated-unchecked", "theorem": "BrievExport.contract_checked",
        "theorems": ["BrievExport.vc_checked", "BrievExport.translation_checked", "BrievExport.contract_checked"],
        "checked_correspondence": "direct program and pre/postconditions agree with serialized assignment semantics for all results",
        "ast_serialization_refinement": "unproved",
        "semantics": "pure signed wrapping 64-bit words",
        "coverage": "selected definition only; straight-line locals, assignments, return, scalar contracts",
        "source_sha256": hash(source), "typed_source_sha256": hash(&typed_source),
        "program_sha256": hash(&program), "model_sha256": hash(MODEL),
        "exporter_source_sha256": hash(include_str!("lean.rs")),
        "model_serializer_sha256": hash(include_str!("lean/model.rs")),
        "type_adapter_source_sha256": hash(include_str!("../analysis/verify_bodies.rs")),
        "compiler_version": env!("CARGO_PKG_VERSION"),
        "assumptions": ["the stated precondition holds", "the emitted word-model semantics"],
        "lean_version": "4.24.0", "source_native_refinement": "unproved",
        "exporter_refinement": "unproved",
    });
    Ok(LeanExport {
        program,
        source: source.into(),
        typed_source,
        manifest,
    })
}

pub const MODEL: &str = include_str!("../../verification/lean/IntegerVC.lean");
pub const TOOLCHAIN: &str = include_str!("../../verification/lean/lean-toolchain");

fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn collect_definitions<'a>(items: &'a [TopLevel], name: &str, found: &mut Vec<&'a Definition>) {
    for item in items {
        match item {
            TopLevel::Definition(d) if d.name == name => found.push(d),
            TopLevel::Export(e) => {
                collect_definitions(std::slice::from_ref(&*e.inner), name, found)
            }
            TopLevel::SyncGroup { item, .. } => {
                collect_definitions(std::slice::from_ref(&**item), name, found)
            }
            _ => {}
        }
    }
}

fn require_word(model: &BodyTypeModel<'_>, ty: &crate::ast::Type) -> Result<(), String> {
    if model.resolve(ty) == (ScalarModel::SignedWord { bits: 64 }) {
        return Ok(());
    }
    Err(format!(
        "Lean export requires standard signed 64-bit scalar semantics: {ty:?}"
    ))
}

fn translate(d: &Definition, model: &BodyTypeModel<'_>) -> Result<String, String> {
    check_signature(d, model)?;
    let (env, binders, args) = parameter_bindings(d, model)?;
    let pre = proposition(&d.contract.pre_condition, &env)?;
    let mut post_env = env.clone();
    if post_env.insert("term".into(), "result".into()).is_some() {
        return Err("parameter shadows the contract result".into());
    }
    let post = proposition(&d.contract.post_condition, &post_env)?;
    let body = translate_body(&d.body, env, model)?;
    let semantics = model::declarations(d, &binders, &args)?;
    Ok(format!(
        r#"-- Generated restricted model; consult manifest.json for coverage and proof status.
import IntegerVC
import Std.Tactic

namespace BrievExport
abbrev Word := BrievIntegerVC.Word
def program{binders} : Word :=
{body}
def pre{binders} : Prop := {pre}
def post{binders} (result : Word) : Prop := {post}
{semantics}
theorem translation_checked{binders} :
    (program{args} = reference_program{args}) ∧
    (pre{args} ↔ BrievIntegerVC.holds (initial{args}) model_pre) ∧
    (∀ result : Word, post{args} result ↔
      BrievIntegerVC.holds (BrievIntegerVC.update (initial{args}) 0 result) model_post) := by
  refine ⟨rfl, ?_, fun result => ?_⟩
  all_goals first | rfl | simp only [pre, post, model_pre, model_post, initial,
    BrievIntegerVC.holds, BrievIntegerVC.eval, BrievIntegerVC.update,
    Int.not_lt, if_true, if_false, Nat.reduceEqDiff]
theorem contract_checked{binders} : pre{args} → post{args} (program{args}) := by
  rw [(translation_checked{args}).1]
  intro precondition
  have model_precondition := (translation_checked{args}).2.1.mp precondition
  have model_result := BrievIntegerVC.sequence_vc_sound model_pre model_post code vc_checked
    (initial{args}) model_precondition
  apply ((translation_checked{args}).2.2 (reference_program{args})).mpr
  simpa only [model_post, reference_program, initial, code, BrievIntegerVC.execute,
    BrievIntegerVC.update, BrievIntegerVC.holds, BrievIntegerVC.eval,
    if_true, if_false, Nat.reduceEqDiff] using model_result
#print axioms vc_checked
#print axioms translation_checked
#print axioms contract_checked
end BrievExport
"#
    ))
}

/// Admit the declaration before translating executable expressions or contracts.
fn check_signature(d: &Definition, model: &BodyTypeModel<'_>) -> Result<(), String> {
    if !d.type_params.is_empty()
        || !d.contract.explicit
        || d.variadic_param.is_some()
        || !d.modifiers.is_empty()
        || !d.annotations.is_empty()
        || d.derivation.is_some()
        || d.contract.watchdog.is_some()
        || d.contract.post_authority
    {
        return Err(
            "Lean export requires a monomorphic definition with an explicit contract".into(),
        );
    }
    let Some(OutputType::Single(output)) = &d.output_type else {
        return Err("Lean export requires one explicit scalar result".into());
    };
    require_word(model, output)?;
    Ok(())
}

/// Keep source parameter names out of generated Lean identifiers.
fn parameter_bindings(
    d: &Definition,
    model: &BodyTypeModel<'_>,
) -> Result<(HashMap<String, String>, String, String), String> {
    let mut env = HashMap::new();
    let mut binders = String::new();
    let mut args = String::new();
    for (index, (name, ty)) in d.parameters.iter().enumerate() {
        require_word(model, ty)?;
        let generated = format!("arg_{index}");
        binders.push_str(&format!(" ({generated} : Word)"));
        args.push_str(&format!(" {generated}"));
        if env.insert(name.clone(), generated).is_some() {
            return Err("duplicate parameter in export".into());
        }
    }
    Ok((env, binders, args))
}

fn translate_body(
    body: &[Statement],
    mut env: HashMap<String, String>,
    model: &BodyTypeModel<'_>,
) -> Result<String, String> {
    let mut lean = String::new();
    let mut locals = HashSet::new();
    for (index, statement) in body.iter().enumerate() {
        match statement {
            Statement::Let {
                name,
                names,
                ty,
                expr: Some(expr),
                modifiers,
            } if names.len() <= 1 && modifiers.is_empty() => {
                if let Some(ty) = ty {
                    require_word(model, ty)?;
                }
                if env.contains_key(name) || name == "term" {
                    return Err("Lean export does not model binding shadowing".into());
                }
                let value = word(expr, &env)?;
                locals.insert(name.clone());
                let generated = format!("local_{index}");
                lean.push_str(&format!("  let {generated} : Word := {value}\n"));
                env.insert(name.clone(), generated);
            }
            Statement::Assign(Expr::Identifier(name), expr) if locals.contains(name) => {
                let value = word(expr, &env)?;
                let generated = format!("local_{index}");
                lean.push_str(&format!("  let {generated} : Word := {value}\n"));
                env.insert(name.clone(), generated);
            }
            Statement::Term(Some(expr)) | Statement::Tail(expr) if index + 1 == body.len() => {
                lean.push_str(&format!("  {}\n", word(expr, &env)?));
                return Ok(lean);
            }
            _ => {
                return Err(format!(
                    "unsupported Lean export statement at index {index}"
                ));
            }
        }
    }
    Err("Lean export requires a final value return".into())
}

fn word(expr: &Expr, env: &HashMap<String, String>) -> Result<String, String> {
    match expr {
        Expr::Decimal(value) => Ok(format!("({} : Word)", *value as u64)),
        Expr::Identifier(name) => env
            .get(name)
            .cloned()
            .ok_or_else(|| format!("unbound/global word in Lean export: {name}")),
        Expr::UnaryOp(UnaryOpKind::Neg, inner) => Ok(format!("(-{})", word(inner, env)?)),
        Expr::BinaryOp(op @ (Op::Add | Op::Sub | Op::Mul), l, r) => {
            let symbol = match op {
                Op::Add => "+",
                Op::Sub => "-",
                _ => "*",
            };
            Ok(format!("({} {symbol} {})", word(l, env)?, word(r, env)?))
        }
        _ => Err("unsupported word expression in Lean export".into()),
    }
}

fn proposition(expr: &Expr, env: &HashMap<String, String>) -> Result<String, String> {
    match expr {
        Expr::Bool(value) => Ok(if *value { "True" } else { "False" }.into()),
        Expr::UnaryOp(UnaryOpKind::Not, inner) => Ok(format!("(¬ {})", proposition(inner, env)?)),
        Expr::BinaryOp(Op::And | Op::Or, l, r) => {
            let symbol = if matches!(expr, Expr::BinaryOp(Op::And, _, _)) {
                "∧"
            } else {
                "∨"
            };
            Ok(format!(
                "({} {symbol} {})",
                proposition(l, env)?,
                proposition(r, env)?
            ))
        }
        Expr::BinaryOp(op @ (Op::Eq | Op::Neq | Op::Lt | Op::Le | Op::Gt | Op::Ge), l, r) => {
            let symbol = match op {
                Op::Eq => "=",
                Op::Neq => "≠",
                Op::Lt => "<",
                Op::Le => "≤",
                Op::Gt => ">",
                _ => "≥",
            };
            let (l, r) = (word(l, env)?, word(r, env)?);
            if matches!(op, Op::Eq | Op::Neq) {
                return Ok(format!("({l} {symbol} {r})"));
            }
            Ok(format!("(({l}).toInt {symbol} ({r}).toInt)"))
        }
        _ => Err("unsupported proposition in Lean export".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_typechecked_words_and_binds_all_inputs() {
        let source = "defn cancel(x: Int64) [true][term == x] -> Int64 { let y: Int64 = x + 1; y = y - 1; term y; };";
        let export = export_source("<export>", source, "cancel").unwrap();
        assert_eq!(export.manifest["schema"], "briev-lean-export/v2");
        assert_eq!(
            export.manifest["theorems"],
            serde_json::json!([
                "BrievExport.vc_checked",
                "BrievExport.translation_checked",
                "BrievExport.contract_checked"
            ])
        );
        assert_eq!(export.manifest["source_sha256"], hash(source));
        assert_eq!(export.manifest["status"], "generated-unchecked");
        assert!(!export.program.contains("sorry"));
    }

    #[test]
    fn serialization_preserves_parameter_order_deterministically() {
        let source = "defn f(z: Int64, a: Int64, m: Int64) [true][term == z * a + m] -> Int64 { term z * a + m; };";
        let first = export_source("<ordered-export>", source, "f").unwrap();
        for _ in 0..8 {
            let next = export_source("<ordered-export>", source, "f").unwrap();
            assert_eq!(first.program, next.program);
            assert_eq!(first.manifest, next.manifest);
        }
    }

    #[test]
    fn unsupported_types_control_flow_and_calls_are_rejected() {
        for source in [
            "defn f(x: Float) [true][term == x] -> Float { term x; };",
            "defn f(x: Int32) [true][term == x] -> Int32 { term x; };",
            "defn f(x: Int64) [true][term == x] -> Int64 { when x > 0 { term x; }; term x; };",
            "defn g(x: Int64) -> Int64 { term x; }; defn f(x: Int64) [true][term == x] -> Int64 { term g(x); };",
            "defn f(x: Int64) [true][term == x] -> Int64 { term x / 1; };",
            "defn f(x: Int64) -> Int64 { term x; };",
            "defn f(x: Int64) [true][term == x + 1] -> Int64 { x = x + 1; term x; };",
            "defn f(x: Int64) [true][term == x] -> Int64 { let x: Int64 = x + 1; term x; };",
        ] {
            assert!(export_source("<export>", source, "f").is_err(), "{source}");
        }
    }

    #[test]
    fn false_contracts_are_exported_as_obligations_never_as_assumptions() {
        let source = "defn f(x: Int64) [true][term > x] -> Int64 { term x + 1; };";
        let export = export_source("<export>", source, "f").unwrap();
        assert!(export.program.contains("→ post arg_0 (program arg_0)"));
        assert_eq!(export.manifest["status"], "generated-unchecked");
    }
}
