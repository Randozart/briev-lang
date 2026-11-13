// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-12 (H5 full-surface port): component constitutive-law elaboration
//! over typed HIR.
//!
//! Mirrors `analysis::electronics_laws`: a pin-bearing component's type-body
//! `hold` laws and named modes declare physics; this pass turns them into the
//! same validated linear IR per instance (`analysis::electronics_laws`
//! types, reused unchanged so the DC solve and every consumer read one IR).
//! The expression recursion walks `HirExpr`/`HirStmt` nodes; the pure linear
//! algebra (dimension arithmetic, BuiltExpression, equation canonicalization)
//! is shared from the AST module — one copy of the math (Rule 17), two front
//! ends. Nothing is derived by lowering back to the AST.
//!
//! The DC solve that consumes this IR (`analysis::electronics_dc`) is
//! AST-free — its inputs are only shared result types — so it is reused
//! verbatim and carries no HIR twin.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::program::{HirProgram, HirTypeDef};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::electronics_laws::{
    add_expressions, constant_quantity, divide_expressions, linear_equation, multiply_expressions,
    negate, polymorphic_zero, reject_duplicate_equations, spec_constant, BuiltExpression,
    ComponentLaws, ElaboratedLaw, ElaboratedMode, ExpressionSite, LawContext, LawDimension,
    LawGuard, LawIr,
};
use crate::ast::{BinaryOpKind, UnaryOpKind};
use std::collections::BTreeMap;

/// Elaborate every pin-bearing type's laws once per declared instance.
/// Mirrors the AST entry; the type table comes from HIR declarations
/// (later declarations shadow earlier prelude ones, like the type tables).
pub fn elaborate_component_laws(program: &HirProgram, ctx: &LawContext<'_>) -> LawIr {
    let mut ir = LawIr::default();
    let mut type_defs: BTreeMap<&str, &HirTypeDef> = BTreeMap::new();
    for td in &program.type_defs {
        if !td.pins.is_empty() {
            type_defs.insert(td.name.as_str(), td);
        }
    }
    for td in type_defs.into_values() {
        if !td.hold_laws.is_empty() || !td.mode_decls.is_empty() {
            elaborate_type_laws(td, ctx, &mut ir);
        }
    }
    ir
}

/// Elaborate one type's laws on every instance of that type.
fn elaborate_type_laws(td: &HirTypeDef, ctx: &LawContext<'_>, ir: &mut LawIr) {
    for (inst_name, inst) in ctx.instances {
        if inst.type_name != td.name {
            continue;
        }
        let (laws, modes, errors) = instance_type_laws(td, inst_name, ctx);
        ir.errors.extend(errors);
        if !laws.is_empty() || !modes.is_empty() {
            ir.components.push(ComponentLaws {
                instance: inst_name.clone(),
                type_name: td.name.clone(),
                laws,
                modes,
            });
        }
    }
}

/// Elaborate all laws of one type for one instance. Named modes and guarded
/// branches are separate authority mechanisms; always-active laws are
/// ordinary facts shared by every mode.
fn instance_type_laws(
    td: &HirTypeDef,
    inst_name: &str,
    ctx: &LawContext<'_>,
) -> (Vec<ElaboratedLaw>, Vec<ElaboratedMode>, Vec<String>) {
    let mut laws = Vec::new();
    let mut modes = Vec::new();
    let mut errors = Vec::new();
    let has_guarded_law = td
        .hold_laws
        .iter()
        .any(|law| !is_bool_true(&law.guard));
    if !td.mode_decls.is_empty() && has_guarded_law {
        errors.push(format!(
            "component type '{}' declares both named modes and guarded `when` laws — pick one \
             state authority. fix: express the branches as `mode <name> {{ … }}`, or keep guarded \
             laws and remove the named modes",
 td.name
 ));
 }
 let mut seen = std::collections::BTreeSet::new();
 for mode in &td.mode_decls {
 if !seen.insert(mode.name.as_str()) {
 errors.push(format!(
 "component type '{}' declares mode '{}' more than once — remove or rename the duplicate",
 td.name, mode.name
 ));
 }
 }
 for raw in &td.hold_laws {
 let source = format!("component law on '{}' (type '{}')", inst_name, td.name);
 match elaborate_law(&raw.guard, &raw.facts, inst_name, ctx, &source) {
 Ok(law) => laws.push(law),
 Err(error) => errors.push(error),
 }
 }
 for raw in &td.mode_decls {
 let source = format!(
 "component mode '{}' on '{}' (type '{}')",
 raw.name, inst_name, td.name
 );
 match elaborate_mode(raw, inst_name, ctx, &source) {
 Ok(mode) => modes.push(mode),
 Err(error) => errors.push(error),
 }
 }
 (laws, modes, errors)
}

/// The AST module's `Expr::Bool(true)` always-guard, on HIR literals.
fn is_bool_true(expr: &HirExpr) -> bool {
 matches!(
 &expr.kind,
 HirExprKind::Literal(HirLiteral::Bool(true))
 ) || matches!(
 &expr.kind,
 HirExprKind::Identifier(name) if name == "true"
 )
}

/// Elaborate one raw law for one instance: guard, then equation facts.
fn elaborate_law(
 guard: &HirExpr,
 facts: &[HirStmt],
 instance: &str,
 ctx: &LawContext<'_>,
 source: &str,
) -> Result<ElaboratedLaw, String> {
 let site = ExpressionSite {
 instance,
 ctx,
 source,
 };
 let guard = elaborate_guard(guard, &site)?;
 let mut equations = Vec::new();
 for fact in facts {
 let Some((lhs, rhs)) = equation_sides(fact) else {
 continue;
 };
 let left = build_expression(lhs, &site)?;
 let right = build_expression(rhs, &site)?;
 equations.push(linear_equation(left, right, source)?);
 }
 if equations.is_empty() {
 return Err(format!(
 "{source} states no electrical equation — a component law must constrain voltage or current"
 ));
 }
 reject_duplicate_equations(&mut equations)?;
 Ok(ElaboratedLaw {
 guard,
 equations,
 source: source.to_string(),
 })
}

/// Elaborate one named mode. Its facts are exactly law equations; there is
/// no per-mode guard because mode membership is the guard.
fn elaborate_mode(
 raw: &super::program::HirModeDecl,
 instance: &str,
 ctx: &LawContext<'_>,
 source: &str,
) -> Result<ElaboratedMode, String> {
 let site = ExpressionSite {
 instance,
 ctx,
 source,
 };
 let mut equations = Vec::new();
 for fact in &raw.facts {
 let Some((lhs, rhs)) = equation_sides(fact) else {
 continue;
 };
 let left = build_expression(lhs, &site)?;
 let right = build_expression(rhs, &site)?;
 equations.push(linear_equation(left, right, source)?);
 }
 if equations.is_empty() {
 return Err(format!(
 "{source} states no electrical equation — every component mode must constrain voltage or current"
 ));
 }
 reject_duplicate_equations(&equations)?;
 Ok(ElaboratedMode {
 name: raw.name.clone(),
 equations,
 source: source.to_string(),
 })
}

/// Accept `lhs = rhs` fact syntax and `lhs == rhs` expression syntax.
fn equation_sides(fact: &HirStmt) -> Option<(&HirExpr, &HirExpr)> {
 match &fact.kind {
 HirStmtKind::Assign(lhs, rhs) => Some((lhs, rhs)),
 HirStmtKind::Expression(expr) => match &expr.kind {
 HirExprKind::Binary {
 op: BinaryOpKind::Eq,
 left,
 right,
 } => Some((left.as_ref(), right.as_ref())),
 _ => None,
 },
 _ => None,
 }
}

/// Elaborate the law's guard. Only the always form and a single linear
/// comparison are supported by the DC substrate today.
fn elaborate_guard(guard: &HirExpr, site: &ExpressionSite<'_>) -> Result<LawGuard, String> {
 let source = site.source;
 if is_bool_true(guard) {
 return Ok(LawGuard::Always);
 }
 let HirExprKind::Binary { op, left, right } = &guard.kind else {
 return Err(format!(
 "{source} has an unsupported guard — component laws support `true` or one linear voltage/current comparison"
 ));
 };
 if !matches!(
 op,
 BinaryOpKind::Eq
 | BinaryOpKind::Neq
 | BinaryOpKind::Lt
 | BinaryOpKind::Gt
 | BinaryOpKind::Le
 | BinaryOpKind::Ge
 ) {
 return Err(format!(
 "{source} has an unsupported guard operator `{op}` — use one linear voltage/current comparison"
 ));
 }
 let right = build_expression(right, site)?;
 let left = polymorphic_zero(build_expression(left, site)?, right.dimension);
 let right = polymorphic_zero(right, left.dimension);
 if left.dimension != right.dimension {
 return Err(format!(
 "{source} guard compares {} with {} — both sides must have the same dimension",
 left.dimension.name(),
 right.dimension.name()
 ));
 }
 Ok(LawGuard::Comparison {
 left: left.into_linear(),
 op: *op,
 right: right.into_linear(),
 })
}

/// Build one side of an equation recursively over HIR nodes.
fn build_expression(expr: &HirExpr, site: &ExpressionSite<'_>) -> Result<BuiltExpression, String> {
 let source = site.source;
 match &expr.kind {
 HirExprKind::Literal(HirLiteral::Float(value)) => {
 Ok(BuiltExpression::constant(*value, LawDimension::DIMENSIONLESS))
 }
 HirExprKind::Literal(HirLiteral::Int(value)) => Ok(BuiltExpression::constant(
 *value as f64,
 LawDimension::DIMENSIONLESS,
 )),
 HirExprKind::UnitLiteral { value, unit } => constant_quantity(*value, unit, source),
 HirExprKind::Unary {
 op: UnaryOpKind::Neg,
 inner,
 } => {
 let mut value = build_expression(inner, site)?;
 value.constant = -value.constant;
 for (_, coeff) in &mut value.terms {
 *coeff = -*coeff;
 }
 Ok(value)
 }
 HirExprKind::Identifier(name) => spec_constant(name, site),
 HirExprKind::Field { receiver, name } => pin_variable(receiver, name, site),
 HirExprKind::Binary { op, left, right } => binary_expression(*op, left, right, site),
 other => Err(format!(
 "{source} contains unsupported law expression `{other:?}` — use linear arithmetic over pin voltage/current and spec constants"
 )),
 }
}

/// Resolve `pin.voltage` / `pin.current` to a law variable. Bare type-body
/// pin references were qualified per instance before this pass; instance
/// qualification is accepted for uniformity.
fn pin_variable(base: &HirExpr, property: &str, site: &ExpressionSite<'_>) -> Result<BuiltExpression, String> {
 let source = site.source;
 let property = match property {
 "voltage" => "voltage",
 "current" => "current",
 other => {
 return Err(format!(
 "{source} accesses `{other}` — component laws read only `.voltage` and `.current`"
 ))
 }
 };
 let resolved = resolve_law_pin(base, site).ok_or_else(|| {
 format!("{source} references a pin that is not declared on instance '{}'", site.instance)
 })?;
 let variable = if property == "voltage" {
 crate::analysis::electronics_laws::LawVariable::Voltage(resolved)
 } else {
 crate::analysis::electronics_laws::LawVariable::Current(resolved)
 };
 Ok(BuiltExpression::variable(variable))
}

/// Resolve a pin access base: a bare pin (already instance-scoped inside a
/// type law) or `inst.pin`.
fn resolve_law_pin(base: &HirExpr, site: &ExpressionSite<'_>) -> Option<crate::analysis::electronics::PinRef> {
 let ctx = site.ctx;
 if let HirExprKind::Identifier(pin) = &base.kind {
 let inst = ctx.instances.get(site.instance)?;
 let pins = ctx.type_pins.get(&inst.type_name)?;
 let (_, number) = pins.iter().find(|(name, _)| name == pin)?;
 return Some(crate::analysis::electronics::PinRef {
 component: site.instance.to_string(),
 pin: pin.clone(),
 number: *number,
 });
 }
 let HirExprKind::Field { receiver, name } = &base.kind else {
 return None;
 };
 let HirExprKind::Identifier(actual_instance) = &receiver.kind else {
 return None;
 };
 let inst = ctx.instances.get(actual_instance)?;
 let pins = ctx.type_pins.get(&inst.type_name)?;
 let (_, number) = pins.iter().find(|(pin, _)| pin == name)?;
 Some(crate::analysis::electronics::PinRef {
 component: actual_instance.clone(),
 pin: name.clone(),
 number: *number,
 })
}

/// Combine two expressions with an arithmetic operator.
fn binary_expression(
 op: BinaryOpKind,
 lhs: &HirExpr,
 rhs: &HirExpr,
 site: &ExpressionSite<'_>,
) -> Result<BuiltExpression, String> {
 let source = site.source;
 let left = build_expression(lhs, site)?;
 let right = build_expression(rhs, site)?;
 match op {
 BinaryOpKind::Add => add_expressions(left, right, source),
 BinaryOpKind::Sub => {
 let negated = negate(right);
 add_expressions(left, negated, source)
 }
 BinaryOpKind::Mul => multiply_expressions(left, right, source),
 BinaryOpKind::Div => divide_expressions(left, right, source),
 _ => Err(format!(
 "{source} uses operator `{op}` — law arithmetic supports +, -, *, and /"
 )),
 }
}

#[cfg(test)]
mod tests {
 //! AST-vs-HIR parity for the law elaboration surface: same source → the
 //! same `LawIr` (errors included), asserted with exact Debug equality.

 use super::*;
 use crate::analysis::electronics::{ComponentInstance, TypeInfo};
 use crate::ast::TopLevel;
 use std::collections::BTreeMap;

 fn parse_lower(src: &str) -> (Vec<TopLevel>, HirProgram) {
 let tokens = crate::lexer::tokenize(src).expect("tokenize");
 let mut parser = crate::parser::Parser::new(tokens, src);
 let items = parser.parse_program().expect("parse");
 let program = crate::hir::lower_program(&items);
 (items, program)
 }

 /// The AST oracle's law tables, built exactly as the AST pipeline does.
 fn ast_tables<'a>(
 items: &'a [TopLevel],
 ) -> (
 BTreeMap<String, Vec<(String, u64)>>,
 BTreeMap<String, TypeInfo>,
 Vec<ComponentInstance>,
 ) {
 let (type_pins, type_info, _) = crate::analysis::electronics::collect_type_pins(items);
 let (instances, _) =
 crate::analysis::electronics::collect_instances(items, &type_pins);
 (type_pins, type_info, instances)
 }

 /// The HIR law tables, built exactly as the HIR netlist pass does.
 fn hir_tables(
 program: &HirProgram,
 ) -> (
 BTreeMap<String, Vec<(String, u64)>>,
 BTreeMap<String, TypeInfo>,
 Vec<ComponentInstance>,
 ) {
 let (pins, info, _) = crate::hir::electronics::collect_type_pins(program);
 let (instances, _) = crate::hir::electronics::collect_instances(program, &pins);
 (pins, info, instances)
 }

 fn parity_laws(src: &str) -> String {
 let (items, program) = parse_lower(src);
 let (a_pins, a_info, a_list) = ast_tables(&items);
 let a_map: BTreeMap<String, &ComponentInstance> =
 a_list.iter().map(|c| (c.name.clone(), c)).collect();
 let ast_ir = crate::analysis::electronics_laws::elaborate_component_laws(
 &items,
 &LawContext {
 instances: &a_map,
 type_info: &a_info,
 type_pins: &a_pins,
 },
 );
 let (h_pins, h_info, h_list) = hir_tables(&program);
 let h_map: BTreeMap<String, &ComponentInstance> =
 h_list.iter().map(|c| (c.name.clone(), c)).collect();
 let hir_ir = elaborate_component_laws(
 &program,
 &LawContext {
 instances: &h_map,
 type_info: &h_info,
 type_pins: &h_pins,
 },
 );
 assert_eq!(
 format!("{hir_ir:?}"),
 format!("{ast_ir:?}"),
 "HIR/AST law-IR drift:\n{src}"
 );
 format!("{ast_ir:?}")
 }

 const RESISTOR_LAW: &str = r#"
        type Resistor {
            pin a; pin b;
            reference "R";
            spec Resistance: Ohm;
            hold true {
                a.voltage - b.voltage == Resistance * a.current;
                a.current + b.current == 0;
            }
        };
        let r1: Resistor = Resistor { value: "330R"; spec Resistance: 330Ohm; };
    "#;

 #[test]
 fn resistor_laws_match() {
 let dump = parity_laws(RESISTOR_LAW);
 assert!(dump.contains("r1"));
 assert!(dump.contains("volts: 1"), "{dump}");
 }

 #[test]
 fn dimension_mismatch_matches() {
 let src = r#"
            type Broken {
                pin a; pin b;
                reference "B";
                hold true { a.voltage - b.voltage == a.current; }
            };
            let bad: Broken = Broken { };
        "#;
 let dump = parity_laws(src);
 assert!(dump.contains("volt") && dump.contains("amp"), "{dump}");
 }

 #[test]
 fn nonlinear_multiplication_matches() {
 let src = r#"
            type Broken {
                pin a; pin b;
                reference "B";
                hold true { a.voltage * b.voltage == 0Volt; }
            };
            let bad: Broken = Broken { };
        "#;
 let dump = parity_laws(src);
 assert!(dump.contains("linear terms"), "{dump}");
 }

 #[test]
 fn missing_spec_parameter_matches() {
 let src = r#"
            type Resistor {
                pin a; pin b;
                reference "R";
                hold true { a.voltage - b.voltage == Resistance * a.current; }
            };
            let r1: Resistor = Resistor { value: "330R"; };
        "#;
 let dump = parity_laws(src);
 assert!(dump.contains("spec 'Resistance'") && dump.contains("'r1'"), "{dump}");
 }

 #[test]
 fn guarded_comparison_matches() {
 let src = r#"
            type Switch {
                pin a; pin b;
                reference "S";
                hold a.voltage >= b.voltage { a.current == 0Amp; }
            };
            let sw1: Switch = Switch {};
        "#;
 let dump = parity_laws(src);
 assert!(dump.contains("Comparison"), "{dump}");
 }

 #[test]
 fn modes_elaborate_and_duplicate_rejected() {
 let src = r#"
            type Spst {
                pin a; pin b;
                reference "S";
                spec Resistance: Ohm;
                mode closed { a.voltage == b.voltage; }
                mode open { a.current == 0Amp; }
            };
            let sw: Spst = Spst { spec Resistance: 1Ohm; };
        "#;
 let dump = parity_laws(src);
 assert!(dump.contains("closed") && dump.contains("open"), "{dump}");

 let dup = r#"
            type Spst {
                pin a; pin b;
                reference "S";
                mode closed { a.voltage == b.voltage; }
                mode closed { a.current == 0Amp; }
            };
            let sw: Spst = Spst {};
        "#;
 let dump = parity_laws(dup);
 assert!(dump.contains("more than once"), "{dump}");
 }

 #[test]
 fn mode_plus_guarded_law_conflict_matches() {
 let src = r#"
            type Spst {
                pin a; pin b;
                reference "S";
                mode closed { a.voltage == b.voltage; }
                hold a.voltage >= b.voltage { a.current == 0Amp; }
            };
            let sw: Spst = Spst {};
        "#;
 let dump = parity_laws(src);
 assert!(dump.contains("state authority"), "{dump}");
    }
}
