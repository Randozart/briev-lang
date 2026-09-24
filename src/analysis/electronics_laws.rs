// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Component constitutive-law elaboration (2026-09-24 component-laws plan).
//!
//! A pin-bearing component's type-body `when` laws declare physics. This pass
//! turns those laws into a validated linear IR per instance. It knows
//! voltages, currents, dimensions, guards, and equations — never component
//! catalog names. Slice 2 validates laws; Slice 3 consumes them in the DC
//! solve.

use crate::analysis::electronics::{ComponentInstance, PinRef, TypeInfo};
use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel, UnaryOpKind};
use std::collections::BTreeMap;

/// A physical dimension relevant to the DC-law substrate.
///
/// Two independent base dimensions cover voltages, currents, resistance, and
/// dimensionless constants (`Ohm = Volt / Amp`). Other dimensions are legal
/// component metadata but are not yet solvable law arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LawDimension {
    volts: i16,
    amps: i16,
}

impl LawDimension {
    pub const DIMENSIONLESS: Self = Self { volts: 0, amps: 0 };
    pub const VOLT: Self = Self { volts: 1, amps: 0 };
    pub const AMP: Self = Self { volts: 0, amps: 1 };
    pub const OHM: Self = Self { volts: 1, amps: -1 };

    /// Compose dimensions under multiplication.
    fn mul(self, other: Self) -> Self {
        Self { volts: self.volts + other.volts, amps: self.amps + other.amps }
    }

    /// Compose dimensions under division.
    fn div(self, other: Self) -> Self {
        Self { volts: self.volts - other.volts, amps: self.amps - other.amps }
    }

    /// Canonical diagnostic name; derived exponents stay explicit and honest.
    fn name(self) -> String {
        match (self.volts, self.amps) {
            (0, 0) => "dimensionless".to_string(),
            (1, 0) => "volt".to_string(),
            (0, 1) => "amp".to_string(),
            (1, -1) => "ohm".to_string(),
            (v, a) => format!("volt^{v} amp^{a}"),
        }
    }

    /// Map a spec quantity dimension onto the DC-law substrate.
    fn from_quantity(dim: crate::ast::QuantityDim) -> Option<Self> {
        match dim {
            crate::ast::QuantityDim::Volt => Some(Self::VOLT),
            crate::ast::QuantityDim::Amp => Some(Self::AMP),
            crate::ast::QuantityDim::Ohm => Some(Self::OHM),
            _ => None,
        }
    }
}

/// One physical variable in a component law.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LawVariable {
    /// The pin's net potential.
    Voltage(PinRef),
    /// Branch current at the pin (positive into the pin).
    Current(PinRef),
}

impl LawVariable {
    /// A variable's physical dimension.
    fn dimension(&self) -> LawDimension {
        match self {
            Self::Voltage(_) => LawDimension::VOLT,
            Self::Current(_) => LawDimension::AMP,
        }
    }
}

/// A linear expression: `constant + Σ coefficient × variable`, with one
/// physical dimension shared by the constant and every completed term.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearExpression {
    pub constant: f64,
    pub dimension: LawDimension,
    pub terms: Vec<(LawVariable, f64)>,
}

impl LinearExpression {
    /// The zero expression in `dimension`.
    fn zero(dimension: LawDimension) -> Self {
        Self { constant: 0.0, dimension, terms: Vec::new() }
    }

    /// Combine coefficients and canonicalize order for deterministic IR and
    /// duplicate-law detection.
    fn canonicalize(mut self) -> Self {
        let mut merged: BTreeMap<LawVariable, f64> = BTreeMap::new();
        for (var, coeff) in self.terms {
            *merged.entry(var).or_default() += coeff;
        }
        self.terms = merged
            .into_iter()
            .filter(|(_, coeff)| coeff.abs() > f64::EPSILON)
            .collect();
        if self.constant.abs() <= f64::EPSILON {
            self.constant = 0.0;
        }
        self
    }
}

/// A supported law guard over linear voltage/current expressions.
#[derive(Debug, Clone, PartialEq)]
pub enum LawGuard {
    /// `when true` — the law is always active.
    Always,
    /// A linear comparison; `op` is a relational operator.
    Comparison {
        left: LinearExpression,
        op: BinaryOpKind,
        right: LinearExpression,
    },
}

/// One equation moved to `lhs - rhs == 0`.
#[derive(Debug, Clone, PartialEq)]
pub struct LawEquation {
    pub expression: LinearExpression,
    pub source: String,
}

/// One elaborated law on one component instance.
#[derive(Debug, Clone, PartialEq)]
pub struct ElaboratedLaw {
    pub guard: LawGuard,
    pub equations: Vec<LawEquation>,
    pub source: String,
}

/// Every elaborated law for one component instance.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentLaws {
    pub instance: String,
    pub type_name: String,
    pub laws: Vec<ElaboratedLaw>,
}

/// The law IR plus hard elaboration diagnostics.
#[derive(Debug, Default)]
pub struct LawIr {
    pub components: Vec<ComponentLaws>,
    pub errors: Vec<String>,
}

/// A scalar-or-variable expression during recursive elaboration.
#[derive(Debug, Clone)]
struct BuiltExpression {
    constant: f64,
    dimension: LawDimension,
    terms: Vec<(LawVariable, f64)>,
}

impl BuiltExpression {
    fn constant(value: f64, dimension: LawDimension) -> Self {
        Self { constant: value, dimension, terms: Vec::new() }
    }

    /// Exact zero expressed in `dimension` (polymorphic-zero result).
    fn zero_expression(dimension: LawDimension) -> Self {
        Self { constant: 0.0, dimension, terms: Vec::new() }
    }

    fn variable(variable: LawVariable) -> Self {
        let dimension = variable.dimension();
        Self { constant: 0.0, dimension, terms: vec![(variable, 1.0)] }
    }

    fn has_variable(&self) -> bool {
        !self.terms.is_empty()
    }

    fn into_linear(mut self) -> LinearExpression {
        let mut expr = LinearExpression {
            constant: self.constant,
            dimension: self.dimension,
            terms: std::mem::take(&mut self.terms),
        };
        expr.canonicalize()
    }
}

/// Inputs needed to elaborate type laws per instance. Bundled to keep helper
/// signatures under the parameter gate.
pub struct LawContext<'a> {
    pub instances: &'a BTreeMap<String, &'a ComponentInstance>,
    pub type_info: &'a BTreeMap<String, TypeInfo>,
    pub type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
}

/// One expression's elaboration site. Bundles the instance, shared tables,
/// and diagnostic source so expression helpers stay under the parameter gate.
struct ExpressionSite<'a> {
    instance: &'a str,
    ctx: &'a LawContext<'a>,
    source: &'a str,
}

/// Elaborate every pin-bearing type's laws once per declared instance.
/// Top-level laws remain the existing conditional-drive path; this pass owns
/// constitutive component behavior.
pub fn elaborate_component_laws(items: &[TopLevel], ctx: &LawContext<'_>) -> LawIr {
    let mut ir = LawIr::default();
    // Later declarations of the same component name shadow earlier prelude
    // declarations, exactly like the type tables.
    let mut type_defs: BTreeMap<&str, &crate::ast::TypeDef> = BTreeMap::new();
    for item in items {
        if let TopLevel::TypeDef(td) = item {
            if !td.body.pins.is_empty() {
                type_defs.insert(td.name.as_str(), td);
            }
        }
    }
    for td in type_defs.into_values() {
        if !td.body.when_laws.is_empty() {
            elaborate_type_laws(td, ctx, &mut ir);
        }
    }
    ir
}

/// Elaborate one type's laws on every instance of that type.
fn elaborate_type_laws(
    td: &crate::ast::TypeDef,
    ctx: &LawContext<'_>,
    ir: &mut LawIr,
) {
    for (inst_name, inst) in ctx.instances {
        if inst.type_name != td.name {
            continue;
        }
        let (laws, errors) = instance_type_laws(td, inst_name, ctx);
        ir.errors.extend(errors);
        if !laws.is_empty() {
            ir.components.push(ComponentLaws {
                instance: inst_name.clone(),
                type_name: td.name.clone(),
                laws,
            });
        }
    }
}

/// Elaborate all laws of one type for one instance.
fn instance_type_laws(
    td: &crate::ast::TypeDef,
    inst_name: &str,
    ctx: &LawContext<'_>,
) -> (Vec<ElaboratedLaw>, Vec<String>) {
    let mut laws = Vec::new();
    let mut errors = Vec::new();
    for raw in &td.body.when_laws {
        let source = format!("component law on '{}' (type '{}')", inst_name, td.name);
        match elaborate_law(&raw.guard, &raw.facts, inst_name, ctx, &source) {
            Ok(law) => laws.push(law),
            Err(error) => errors.push(error),
        }
    }
    (laws, errors)
}

/// Elaborate one raw law for one instance: guard, then equation facts.
fn elaborate_law(
    guard: &Expr,
    facts: &[Statement],
    instance: &str,
    ctx: &LawContext<'_>,
    source: &str,
) -> Result<ElaboratedLaw, String> {
    let site = ExpressionSite { instance, ctx, source };
    let guard = elaborate_guard(guard, &site)?;
    let mut equations = Vec::new();
    for fact in facts {
        let Some((lhs, rhs)) = equation_sides(fact) else { continue };
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
    Ok(ElaboratedLaw { guard, equations, source: source.to_string() })
}

/// Accept `lhs = rhs` fact syntax and `lhs == rhs` expression syntax.
fn equation_sides(fact: &Statement) -> Option<(&Expr, &Expr)> {
    match fact {
        Statement::Assign(lhs, rhs) => Some((lhs, rhs)),
        Statement::Expression(expr) => match expr {
            Expr::BinaryOp(BinaryOpKind::Eq, lhs, rhs) => Some((lhs, rhs)),
            _ => None,
        },
        _ => None,
    }
}

/// Elaborate the law's guard. Only the always form and a single linear
/// comparison are supported by the DC substrate today.
fn elaborate_guard(guard: &Expr, site: &ExpressionSite<'_>) -> Result<LawGuard, String> {
    let (instance, ctx, source) = (site.instance, site.ctx, site.source);
    if matches!(guard, Expr::Bool(true)) {
        return Ok(LawGuard::Always);
    }
    let Expr::BinaryOp(op, lhs, rhs) = guard else {
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
    let right = build_expression(rhs, site)?;
    let left = polymorphic_zero(build_expression(lhs, site)?, right.dimension);
    let right = polymorphic_zero(right, left.dimension);
    if left.dimension != right.dimension {
        return Err(format!(
            "{source} guard compares {} with {} — both sides must have the same dimension",
            left.dimension.name(),
            right.dimension.name()
        ));
    }
    Ok(LawGuard::Comparison { left: left.into_linear(), op: *op, right: right.into_linear() })
}

/// Build one side of an equation recursively.
fn build_expression(expr: &Expr, site: &ExpressionSite<'_>) -> Result<BuiltExpression, String> {
    let (instance, ctx, source) = (site.instance, site.ctx, site.source);
    match expr {
        Expr::Float(value) => Ok(BuiltExpression::constant(*value, LawDimension::DIMENSIONLESS)),
        Expr::Decimal(value) => {
            Ok(BuiltExpression::constant(*value as f64, LawDimension::DIMENSIONLESS))
        }
        Expr::UnitLiteral { value, unit } => {
            constant_quantity(*value, unit, source)
        }
        Expr::UnaryOp(UnaryOpKind::Neg, inner) => {
            let mut value = build_expression(inner, site)?;
            value.constant = -value.constant;
            for (_, coeff) in &mut value.terms {
                *coeff = -*coeff;
            }
            Ok(value)
        }
        Expr::Identifier(name) => spec_constant(name, site),
        Expr::Field(base, property) => pin_variable(base, property, site),
        Expr::BinaryOp(op, lhs, rhs) => binary_expression(*op, lhs, rhs, site),
        other => Err(format!(
            "{source} contains unsupported law expression `{other}` — use linear arithmetic over pin voltage/current and spec constants"
        )),
    }
}

/// A quantity literal in canonical SI units.
fn constant_quantity(value: f64, unit: &str, source: &str) -> Result<BuiltExpression, String> {
    let Some(crate::parser::quantity::UnitSuffix::Explicit { scale, dim }) =
        crate::parser::quantity::parse_unit_suffix(unit)
    else {
        return Err(format!("{source} uses unknown unit `{unit}`"));
    };
    let Some(dimension) = LawDimension::from_quantity(dim) else {
        return Err(format!(
            "{source} uses `{unit}`, but the DC-law substrate supports only Volt, Amp, and Ohm quantities"
        ));
    };
    Ok(BuiltExpression::constant(value * scale, dimension))
}

/// Resolve a bare spec parameter to its instance quantity.
fn spec_constant(name: &str, site: &ExpressionSite<'_>) -> Result<BuiltExpression, String> {
    let (instance, ctx, source) = (site.instance, site.ctx, site.source);
    // Known physics keys use canonical snake-case metadata storage; user law
    // parameters use the lowercase spelling.
    let storage_key = crate::parser::spec_name_to_key(name)
        .map(str::to_string)
        .unwrap_or_else(|| name.to_lowercase());
    let Some(inst) = ctx.instances.get(instance) else {
        return Err(format!("{source} names missing instance '{instance}'"));
    };
    let quantity = inst
        .specs
        .get(storage_key.as_str())
        .or_else(|| {
            ctx.type_info
                .get(&inst.type_name)
                .and_then(|info| info.spec_defaults.get(storage_key.as_str()))
        });
    let Some(crate::ast::PropertyValue::Quantity { si, dimension }) = quantity else {
        return Err(format!(
            "{source} references spec '{name}', but instance '{instance}' supplies no value — add `spec {name}: <quantity>;`"
        ));
    };
    let Some(dimension) = LawDimension::from_quantity(*dimension) else {
        return Err(format!(
            "{source} spec '{name}' is in an unsupported dimension — DC laws use Volt, Amp, and Ohm quantities"
        ));
    };
    Ok(BuiltExpression::constant(*si, dimension))
}

/// Resolve `pin.voltage` / `pin.current` to a law variable.
fn pin_variable(
    base: &Expr,
    property: &str,
    site: &ExpressionSite<'_>,
) -> Result<BuiltExpression, String> {
    let (instance, ctx, source) = (site.instance, site.ctx, site.source);
    let property = match property {
        "voltage" => "voltage",
        "current" => "current",
        other => {
            return Err(format!(
                "{source} accesses `{other}` — component laws read only `.voltage` and `.current`"
            ))
        }
    };
    // Bare type-body pin references were qualified per instance before this
    // pass. Resolve them with the same pin path logic as contracts.
    let resolved = resolve_law_pin(base, site)
        .ok_or_else(|| format!("{source} references a pin that is not declared on instance '{instance}'"))?;
    let variable = if property == "voltage" {
        LawVariable::Voltage(resolved)
    } else {
        LawVariable::Current(resolved)
    };
    Ok(BuiltExpression::variable(variable))
}

/// Resolve a pin access base. `qualify_pin_refs` produces `inst.a`; instance
/// qualification is accepted for uniformity.
fn resolve_law_pin(base: &Expr, site: &ExpressionSite<'_>) -> Option<PinRef> {
    let (instance, ctx) = (site.instance, site.ctx);
    // A bare pin is already instance-scoped inside a type law.
    if let Expr::Identifier(pin) = base {
        let inst = ctx.instances.get(instance)?;
        let pins = ctx.type_pins.get(&inst.type_name)?;
        let (_, number) = pins.iter().find(|(name, _)| name == pin)?;
        return Some(PinRef {
            component: instance.to_string(),
            pin: pin.clone(),
            number: *number,
        });
    }
    let Expr::Field(qualifier, pin) = base else { return None };
    let Expr::Identifier(actual_instance) = qualifier.as_ref() else { return None };
    let inst = ctx.instances.get(actual_instance)?;
    let pins = ctx.type_pins.get(&inst.type_name)?;
    let (_, number) = pins.iter().find(|(name, _)| name == pin)?;
    Some(PinRef {
        component: actual_instance.clone(),
        pin: pin.clone(),
        number: *number,
    })
}

/// Combine two expressions with an arithmetic operator.
fn binary_expression(
    op: BinaryOpKind,
    lhs: &Expr,
    rhs: &Expr,
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

/// Add two linear expressions of the same dimension. A literal dimensionless
/// zero is polymorphic (`current == 0`), but a nonzero constant is not.
fn add_expressions(left: BuiltExpression, right: BuiltExpression, source: &str) -> Result<BuiltExpression, String> {
    let left = polymorphic_zero(left, right.dimension);
    let right = polymorphic_zero(right, left.dimension);
    if left.dimension != right.dimension {
        return Err(format!(
            "{source} adds/subtracts {} and {} — both sides need the same dimension",
            left.dimension.name(),
            right.dimension.name()
        ));
    }
    let mut terms = left.terms;
    terms.extend(right.terms);
    Ok(BuiltExpression {
        constant: left.constant + right.constant,
        dimension: left.dimension,
        terms,
    })
}

/// Negate one expression without changing its dimension.
fn negate(mut value: BuiltExpression) -> BuiltExpression {
    value.constant = -value.constant;
    for (_, coeff) in &mut value.terms {
        *coeff = -*coeff;
    }
    value
}

/// Multiply expressions. One side must be constant; variable × variable is
/// nonlinear and rejected.
fn multiply_expressions(
    left: BuiltExpression,
    right: BuiltExpression,
    source: &str,
) -> Result<BuiltExpression, String> {
    if left.has_variable() && right.has_variable() {
        return Err(format!(
            "{source} multiplies two voltage/current expressions — the DC-law substrate supports linear terms only"
        ));
    }
    let dimension = left.dimension.mul(right.dimension);
    let (scaled, constant) = if left.has_variable() {
        (scale_terms(left.terms, right.constant), left.constant * right.constant)
    } else {
        (scale_terms(right.terms, left.constant), left.constant * right.constant)
    };
    Ok(BuiltExpression { constant, dimension, terms: scaled })
}

/// Divide by a constant. Division by a variable is nonlinear and rejected.
fn divide_expressions(
    left: BuiltExpression,
    right: BuiltExpression,
    source: &str,
) -> Result<BuiltExpression, String> {
    if right.has_variable() {
        return Err(format!(
            "{source} divides by a voltage/current expression — the DC-law substrate supports constant divisors only"
        ));
    }
    if right.constant.abs() <= f64::EPSILON {
        return Err(format!("{source} divides by zero"));
    }
    let dimension = left.dimension.div(right.dimension);
    Ok(BuiltExpression {
        constant: left.constant / right.constant,
        dimension,
        terms: scale_terms(left.terms, 1.0 / right.constant),
    })
}

/// Scale all variable coefficients by a constant.
fn scale_terms(terms: Vec<(LawVariable, f64)>, scale: f64) -> Vec<(LawVariable, f64)> {
    terms.into_iter().map(|(var, coeff)| (var, coeff * scale)).collect()
}

/// A literal `0` means zero in the other side's dimension. This is notation,
/// not implicit unit conversion: only exact zero with no variables changes.
fn polymorphic_zero(value: BuiltExpression, dimension: LawDimension) -> BuiltExpression {
    let is_zero = value.dimension == LawDimension::DIMENSIONLESS
        && value.constant == 0.0
        && !value.has_variable();
    if is_zero {
        BuiltExpression::zero_expression(dimension)
    } else {
        value
    }
}

/// Convert `lhs == rhs` to the canonical `lhs - rhs == 0`.
fn linear_equation(
    left: BuiltExpression,
    right: BuiltExpression,
    source: &str,
) -> Result<LawEquation, String> {
    let left = polymorphic_zero(left, right.dimension);
    let right = polymorphic_zero(right, left.dimension);
    if left.dimension != right.dimension {
        return Err(format!(
            "{source} equates {} with {} — both sides must have the same dimension",
            left.dimension.name(),
            right.dimension.name()
        ));
    }
    let negated = negate(right);
    let combined = add_expressions(left, negated, source)?.into_linear();
    Ok(LawEquation { expression: combined, source: source.to_string() })
}

/// Duplicate equations carry no independent behavior; rejecting them prevents
/// a component from looking more constrained than it is.
fn reject_duplicate_equations(equations: &[LawEquation]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for equation in equations {
        let key = format!("{:?}", equation.expression);
        let inserted = seen.insert(key);
        if !inserted {
            return Err(format!(
                "{} states the same equation more than once — remove the duplicate",
                equation.source
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::Parser;

    /// Parse and derive, returning only the component-law result.
    fn laws(src: &str) -> LawIr {
        let tokens = tokenize(src).unwrap();
        let mut parser = Parser::new(tokens, src);
        let items = parser.parse_program().unwrap();
        let (type_pins, type_info, _) = crate::analysis::electronics::collect_type_pins_for_laws(&items);
        let instances = crate::analysis::electronics::collect_instances_for_laws(&items, &type_pins);
        let instances: BTreeMap<String, &ComponentInstance> =
            instances.iter().map(|c| (c.name.clone(), c)).collect();
        elaborate_component_laws(
            &items,
            &LawContext { instances: &instances, type_info: &type_info, type_pins: &type_pins },
        )
    }

    const RESISTOR_LAW: &str = r#"
        type Resistor {
            pin a; pin b;
            reference "R";
            spec Resistance: Ohm;
            when true {
                a.voltage - b.voltage == Resistance * a.current;
                a.current + b.current == 0;
            }
        };
        let r1: Resistor = Resistor { value: "330R"; spec Resistance: 330Ohm; };
    "#;

    #[test]
    fn resistor_laws_elaborate_to_canonical_linear_ir() {
        let ir = laws(RESISTOR_LAW);
        assert!(ir.errors.is_empty(), "{:?}", ir.errors);
        assert_eq!(ir.components.len(), 1);
        let component = &ir.components[0];
        assert_eq!(component.instance, "r1");
        assert_eq!(component.laws.len(), 1);
        assert_eq!(component.laws[0].guard, LawGuard::Always);
        assert_eq!(component.laws[0].equations.len(), 2);

        let ohmic = &component.laws[0].equations[0].expression;
        assert_eq!(ohmic.dimension, LawDimension::VOLT);
        assert_eq!(ohmic.terms.len(), 3);
        assert!(ohmic
            .terms
            .iter()
            .any(|(v, c)| matches!(v, LawVariable::Current(_)) && (c + 330.0).abs() < 1e-9));

        let conservation = &component.laws[0].equations[1].expression;
        assert_eq!(conservation.dimension, LawDimension::AMP);
        assert_eq!(conservation.terms.len(), 2);
    }

    #[test]
    fn dimension_mismatch_is_a_hard_law_error() {
        let src = r#"
            type Broken {
                pin a; pin b;
                reference "B";
                spec Resistance: Ohm;
                when true { a.voltage - b.voltage == a.current; }
        };
            let bad: Broken = Broken { spec Resistance: 1Ohm; };
        "#;
        let ir = laws(src);
        assert!(ir.components.is_empty());
        assert_eq!(ir.errors.len(), 1, "{:?}", ir.errors);
        assert!(ir.errors[0].contains("volt") && ir.errors[0].contains("amp"), "{}", ir.errors[0]);
    }

    #[test]
    fn nonlinear_multiplication_is_rejected() {
        let src = r#"
            type Broken {
                pin a; pin b;
                reference "B";
                when true { a.voltage * b.voltage == 0Volt; }
            };
            let bad: Broken = Broken { };
        "#;
        let ir = laws(src);
        assert_eq!(ir.errors.len(), 1, "{:?}", ir.errors);
        assert!(ir.errors[0].contains("linear terms"), "{}", ir.errors[0]);
    }

    #[test]
    fn missing_spec_parameter_is_named() {
        let src = r#"
            type Resistor {
                pin a; pin b;
                reference "R";
                spec Resistance: Ohm;
                when true { a.voltage - b.voltage == Resistance * a.current; }
            };
            let r1: Resistor = Resistor { value: "330R"; };
        "#;
        let ir = laws(src);
        assert_eq!(ir.errors.len(), 1, "{:?}", ir.errors);
        assert!(
            ir.errors[0].contains("spec 'Resistance'") && ir.errors[0].contains("'r1'"),
            "{}",
            ir.errors[0]
        );
    }

    #[test]
    fn duplicate_equations_are_rejected() {
        let src = r#"
            type Dup {
                pin a; pin b;
                reference "D";
                when true {
                    a.voltage == b.voltage;
                    a.voltage == b.voltage;
                }
            };
            let d1: Dup = Dup {};
        "#;
        let ir = laws(src);
        assert_eq!(ir.errors.len(), 1, "{:?}", ir.errors);
        assert!(ir.errors[0].contains("same equation"), "{}", ir.errors[0]);
    }

    #[test]
    fn guarded_linear_comparison_elaborates() {
        let src = r#"
            type Switch {
                pin a; pin b;
                reference "S";
                when a.voltage >= b.voltage { a.current == 0Amp; }
            };
            let sw1: Switch = Switch {};
        "#;
        let ir = laws(src);
        assert!(ir.errors.is_empty(), "{:?}", ir.errors);
        let LawGuard::Comparison { op, .. } = &ir.components[0].laws[0].guard else {
            panic!("expected linear comparison")
        };
        assert_eq!(*op, BinaryOpKind::Ge);
    }
}

