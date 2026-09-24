// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Unguarded constitutive-law DC solve (2026-09-24 component-laws plan).
//!
//! Slice 3 solves always-active linear component laws against the contract's
//! ideal voltage boundaries. Law-bearing components are grouped by connected
//! nets so one incomplete group cannot make an unrelated group look
//! underdetermined. The solver reports inconsistency and free variables; it
//! never chooses an implicit operating point.

use crate::analysis::electronics::{ComponentInstance, Net, PinRef, TypeInfo};
use crate::analysis::electronics_laws::{ComponentLaws, LawGuard, LawVariable};
use std::collections::{BTreeMap, BTreeSet};

/// One solved unguarded DC operating point.
#[derive(Debug, Clone, Default)]
pub struct DcSolution {
    /// Net name → solved or boundary voltage.
    pub net_voltage: BTreeMap<String, f64>,
    /// Instance + expanded pin → solved branch current.
    pub pin_current: BTreeMap<(String, String), f64>,
}

/// Inputs for one solve pass. Bundled to keep the public entry under the
/// parameter gate.
pub struct DcContext<'a> {
    pub nets: &'a [Net],
    pub components: &'a [ComponentLaws],
    pub instances: &'a BTreeMap<String, &'a ComponentInstance>,
    pub type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
    pub unpop: &'a std::collections::HashSet<String>,
    /// Contract voltage drives: ideal boundary conditions.
    pub drives: &'a BTreeMap<String, f64>,
}

/// A system variable after pin-voltage variables are mapped to nets.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SystemVariable {
    Net(String),
    Current { component: String, pin: String },
}

/// One matrix row `Σ coefficient × variable = constant`.
#[derive(Debug, Default)]
struct SystemRow {
    coefficients: BTreeMap<usize, f64>,
    constant: f64,
}

/// One connected solve group: law components joined by shared nets.
#[derive(Debug, Default)]
struct ComponentGroup<'a> {
    components: Vec<&'a ComponentLaws>,
    nets: BTreeSet<String>,
}

/// Solve every unguarded law group. Returns all successful group solutions
/// plus hard diagnostics. A failed group has no solution, never a default.
pub fn solve_dc_laws(ctx: &DcContext<'_>) -> (Vec<DcSolution>, Vec<String>) {
    let groups = component_groups(ctx);
    let mut solutions = Vec::new();
    let mut errors = Vec::new();
    for group in groups {
        match solve_group(&group, ctx) {
            Ok(solution) => solutions.push(solution),
            Err(error) => errors.push(error),
        }
    }
    (solutions, errors)
}

/// Absorb every existing group already touching one of the component's nets.
fn merge_touching_groups<'a>(
    target: &mut ComponentGroup<'a>,
    groups: &mut Vec<ComponentGroup<'a>>,
    net_group: &mut BTreeMap<String, usize>,
    nets: &[String],
) {
    let touching: Vec<usize> = nets
        .iter()
        .filter_map(|net| net_group.get(net).copied())
        .collect();
    for index in touching {
        let moved = std::mem::take(&mut groups[index]);
        target.components.extend(moved.components);
        target.nets.extend(moved.nets);
    }
}

/// Group law-bearing components by shared nets. Unpopulated parts contribute
/// no equations in the absent state and are excluded from this first pass.
fn component_groups<'a>(ctx: &DcContext<'a>) -> Vec<ComponentGroup<'a>> {
    let mut groups: Vec<ComponentGroup<'_>> = Vec::new();
    let mut net_group: BTreeMap<String, usize> = BTreeMap::new();
    for component in ctx.components {
        if ctx.unpop.contains(&component.instance) {
            continue;
        }
        let nets = component_nets(component, ctx);
        let mut target = ComponentGroup {
            components: vec![component],
            nets: nets.iter().cloned().collect(),
        };
        merge_touching_groups(&mut target, &mut groups, &mut net_group, &nets);
        groups.retain(|group| !group.components.is_empty());
        register_group_nets(&target, &mut net_group, groups.len());
        groups.push(target);
    }
    groups
}

/// Map every net in a newly pushed group to its index.
fn register_group_nets(
    target: &ComponentGroup<'_>,
    net_group: &mut BTreeMap<String, usize>,
    index: usize,
) {
    for net in &target.nets {
        net_group.insert(net.clone(), index);
    }
}

/// Every net touched by one law-bearing component's pins.
fn component_nets(
    component: &ComponentLaws,
    ctx: &DcContext<'_>,
) -> Vec<String> {
    let mut nets = Vec::new();
    let Some(inst) = ctx.instances.get(&component.instance) else { return nets };
    let Some(pins) = ctx.type_pins.get(&inst.type_name) else { return nets };
    for (pin, _) in pins {
        let key = (component.instance.as_str(), pin.as_str());
        if let Some(net) = ctx
            .nets
            .iter()
            .find(|net| net.pins.iter().any(|p| p.component == key.0 && p.pin == key.1))
        {
            nets.push(net.name.clone());
        }
    }
    nets.sort();
    nets.dedup();
    nets
}

/// Build and solve one connected law group.
fn solve_group(
    group: &ComponentGroup<'_>,
    ctx: &DcContext<'_>,
) -> Result<DcSolution, String> {
    let mut rows = Vec::new();
    let mut variables = VariableTable::default();
    append_law_equations(group, ctx, &mut variables, &mut rows)?;
    append_kcl(group, ctx, &mut variables, &mut rows);
    let values = solve_linear_system(rows, &variables, group)?;
    let mut solution = DcSolution::default();
    for net in &group.nets {
        let value = match ctx.drives.get(net) {
            Some(fixed) => *fixed,
            None => values
                .get(&SystemVariable::Net(net.clone()))
                .copied()
                .unwrap_or_default(),
        };
        solution.net_voltage.insert(net.clone(), value);
    }
    for (variable, value) in values {
        let SystemVariable::Current { component, pin } = variable else { continue };
        solution.pin_current.insert((component, pin), value);
    }
    Ok(solution)
}

/// Bijective variable table with deterministic indices.
#[derive(Debug, Default)]
struct VariableTable {
    indices: BTreeMap<SystemVariable, usize>,
    order: Vec<SystemVariable>,
}

impl VariableTable {
    /// Get or create a deterministic variable index.
    fn insert(&mut self, variable: SystemVariable) -> usize {
        if let Some(&index) = self.indices.get(&variable) {
            return index;
        }
        let index = self.order.len();
        self.order.push(variable.clone());
        self.indices.insert(variable, index);
        index
    }

    fn len(&self) -> usize {
        self.order.len()
    }
}

/// Add every always-active law equation to the system.
fn append_law_equations(
    group: &ComponentGroup<'_>,
    ctx: &DcContext<'_>,
    variables: &mut VariableTable,
    rows: &mut Vec<SystemRow>,
) -> Result<(), String> {
    for component in &group.components {
        component_law_rows(component, ctx, variables, rows)?;
    }
    Ok(())
}

/// Add one component's always-active equations to the system.
fn component_law_rows(
    component: &ComponentLaws,
    ctx: &DcContext<'_>,
    variables: &mut VariableTable,
    rows: &mut Vec<SystemRow>,
) -> Result<(), String> {
    for law in &component.laws {
        law_equation_rows(law, ctx, variables, rows)?;
    }
    Ok(())
}

/// Add one law's equations. Guarded modes are Slice 4; they are excluded
/// here rather than approximated as always active.
fn law_equation_rows(
    law: &crate::analysis::electronics_laws::ElaboratedLaw,
    ctx: &DcContext<'_>,
    variables: &mut VariableTable,
    rows: &mut Vec<SystemRow>,
) -> Result<(), String> {
    if law.guard != LawGuard::Always {
        return Ok(());
    }
    for equation in &law.equations {
        let row = law_equation_row(equation, ctx, variables)
            .map_err(|_| format!("{} has a pin on no net", law.source))?;
        rows.push(row);
    }
    Ok(())
}

/// Convert one equality to a system row.
fn law_equation_row(
    equation: &crate::analysis::electronics_laws::LawEquation,
    ctx: &DcContext<'_>,
    variables: &mut VariableTable,
) -> Result<SystemRow, String> {
    let mut row = SystemRow {
        coefficients: Default::default(),
        constant: -equation.expression.constant,
    };
    for (variable, coefficient) in &equation.expression.terms {
        append_term(&mut row, variable, *coefficient, ctx, variables)?;
    }
    Ok(row)
}

/// Convert numeric row keys to a dense coefficient vector just before solve.
fn dense_row(row: &SystemRow, width: usize) -> (Vec<f64>, f64) {
    let mut coefficients = vec![0.0; width];
    for (index, coefficient) in &row.coefficients {
        if *index < width {
            coefficients[*index] += coefficient;
        }
    }
    (coefficients, row.constant)
}

/// Add KCL on every non-boundary net in the group.
fn append_kcl(
    group: &ComponentGroup<'_>,
    ctx: &DcContext<'_>,
    variables: &mut VariableTable,
    rows: &mut Vec<SystemRow>,
) {
    let law_components: BTreeSet<&str> = group
        .components
        .iter()
        .map(|component| component.instance.as_str())
        .collect();
    for net in &group.nets {
        if ctx.drives.contains_key(net) {
            // An ideal voltage boundary supplies whatever current the laws
            // require; KCL would otherwise hide the branch current.
            continue;
        }
        let row = net_kcl_row(net, &law_components, ctx, variables);
        if !row.coefficients.is_empty() {
            rows.push(row);
        }
    }
}

/// Build Σ I(pin) = 0 for one non-boundary net.
fn net_kcl_row(
    net: &str,
    law_components: &BTreeSet<&str>,
    ctx: &DcContext<'_>,
    variables: &mut VariableTable,
) -> SystemRow {
    let mut row = SystemRow::default();
    for pin in net_law_pins(net, law_components, ctx) {
        let variable = SystemVariable::Current {
            component: pin.component.clone(),
            pin: pin.pin.clone(),
        };
        let index = variables.insert(variable);
        *row.coefficients.entry(index).or_default() += 1.0;
    }
    row
}

/// Law-bearing pins on one net, deterministic order.
fn net_law_pins(
    net: &str,
    law_components: &BTreeSet<&str>,
    ctx: &DcContext<'_>,
) -> Vec<PinRef> {
    let Some(model) = ctx.nets.iter().find(|candidate| candidate.name == net) else {
        return Vec::new();
    };
    let mut pins: Vec<PinRef> = model
        .pins
        .iter()
        .filter(|pin| law_components.contains(pin.component.as_str()))
        .cloned()
        .collect();
    pins.sort();
    pins
}

/// Add one law term to a row. Fixed net voltages are boundary conditions and
/// move to the constant side; only unknown potentials become variables.
fn append_term(
    row: &mut SystemRow,
    variable: &LawVariable,
    coefficient: f64,
    ctx: &DcContext<'_>,
    variables: &mut VariableTable,
) -> Result<(), String> {
    if let LawVariable::Voltage(pin) = variable {
        if let Some(net) = pin_net(pin, ctx) {
            if let Some(fixed) = ctx.drives.get(&net) {
                row.constant -= coefficient * fixed;
                return Ok(());
            }
        }
    }
    let Some(system_variable) = system_variable(variable, ctx) else {
        return Err("pin on no net".to_string());
    };
    let index = variables.insert(system_variable);
    *row.coefficients.entry(index).or_default() += coefficient;
    Ok(())
}

/// Map a law variable onto the DC system's net/current variables.
fn system_variable(
    variable: &LawVariable,
    ctx: &DcContext<'_>,
) -> Option<SystemVariable> {
    match variable {
        LawVariable::Voltage(pin) => {
            let net = pin_net(pin, ctx)?;
            Some(SystemVariable::Net(net))
        }
        LawVariable::Current(pin) => Some(SystemVariable::Current {
            component: pin.component.clone(),
            pin: pin.pin.clone(),
        }),
    }
}

/// Pin → net name.
fn pin_net(pin: &PinRef, ctx: &DcContext<'_>) -> Option<String> {
    ctx.nets
        .iter()
        .find(|net| {
            net.pins
                .iter()
                .any(|member| member.component == pin.component && member.pin == pin.pin)
        })
        .map(|net| net.name.clone())
}

/// Pick the available row with the largest pivot magnitude. A deterministic
/// helper avoids `max_by`'s range-item ambiguity.
fn max_pivot_row(
    augmented: &[(Vec<f64>, f64)],
    start: usize,
    column: usize,
) -> Option<usize> {
    let mut choice = None;
    let mut best = 0.0f64;
    for row in start..augmented.len() {
        let magnitude = augmented[row].0[column].abs();
        if magnitude > best {
            best = magnitude;
            choice = Some(row);
        }
    }
    choice
}

/// Dense Gaussian elimination with partial pivoting.
/// Inconsistent rows mean no operating point. Free variables mean many
/// operating points; both are hard diagnostics for an unguarded group.
fn solve_linear_system(
    rows: Vec<SystemRow>,
    variables: &VariableTable,
    group: &ComponentGroup<'_>,
) -> Result<BTreeMap<SystemVariable, f64>, String> {
    let width = variables.len();
    let mut augmented: Vec<(Vec<f64>, f64)> =
        rows.iter().map(|row| dense_row(row, width)).collect();
    let pivot_columns = gauss_eliminate(&mut augmented, width);
    validate_system(&augmented, &pivot_columns, variables, width, group)?;
    let values = back_substitute(&augmented, &pivot_columns, width);
    Ok(variables.order.iter().cloned().zip(values).collect())
}

/// Reduce an augmented matrix and return its pivot columns.
fn gauss_eliminate(augmented: &mut [(Vec<f64>, f64)], width: usize) -> Vec<usize> {
    let mut pivot_columns = Vec::new();
    for column in 0..width {
        let Some(choice) = max_pivot_row(augmented, pivot_columns.len(), column) else {
            break;
        };
        if augmented[choice].0[column].abs() <= f64::EPSILON {
            continue;
        }
        let pivot_row = pivot_columns.len();
        augmented.swap(pivot_row, choice);
        normalize_pivot(augmented, pivot_row, column);
        pivot_columns.push(column);
    }
    pivot_columns
}

/// Normalize one pivot row and eliminate that column from every other row.
fn normalize_pivot(augmented: &mut [(Vec<f64>, f64)], pivot_row: usize, column: usize) {
    let pivot = augmented[pivot_row].0[column];
    augmented[pivot_row].0.iter_mut().for_each(|value| *value /= pivot);
    augmented[pivot_row].1 /= pivot;
    let pivot_values = augmented[pivot_row].0.clone();
    let pivot_constant = augmented[pivot_row].1;
    augmented
        .iter_mut()
        .enumerate()
        .filter(|(row, _)| *row != pivot_row)
        .for_each(|(_, row)| {
            eliminate_row(row, &pivot_values, pivot_constant, column)
        });
}

/// Subtract one scaled pivot row from a target row.
fn eliminate_row(
    row: &mut (Vec<f64>, f64),
    pivot_values: &[f64],
    pivot_constant: f64,
    column: usize,
) {
    let factor = row.0[column];
    if factor.abs() <= f64::EPSILON {
        return;
    }
    row.0
        .iter_mut()
        .zip(pivot_values)
        .for_each(|(value, pivot_value)| *value -= factor * pivot_value);
    row.1 -= factor * pivot_constant;
}

/// Reject inconsistent systems and systems with free variables.
fn validate_system(
    augmented: &[(Vec<f64>, f64)],
    pivot_columns: &[usize],
    variables: &VariableTable,
    width: usize,
    group: &ComponentGroup<'_>,
) -> Result<(), String> {
    let inconsistent = augmented.iter().any(|(coefficients, constant)| {
        coefficients.iter().all(|value| value.abs() <= f64::EPSILON)
            && constant.abs() > f64::EPSILON
    });
    if inconsistent {
        return Err(format!(
            "component-law group {:?} has no DC operating point — its equations and boundaries contradict",
            group_label(group)
        ));
    }
    let rank = pivot_columns.len();
    if rank < width {
        return Err(free_variable_error(pivot_columns, variables, width, group));
    }
    Ok(())
}

/// Name every non-pivoted variable in the multiple-solution diagnostic.
fn free_variable_error(
    pivot_columns: &[usize],
    variables: &VariableTable,
    width: usize,
    group: &ComponentGroup<'_>,
) -> String {
    let pivoted: std::collections::BTreeSet<usize> = pivot_columns.iter().copied().collect();
    let free = variables
        .order
        .iter()
        .enumerate()
        .filter(|(index, _)| !pivoted.contains(index))
        .map(|(_, variable)| variable_label(variable))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "component-law group {:?} has no unique DC operating point — free variable(s): {free}. Complete the boundaries, model the missing branch, or declare bistability when Slice 4 lands",
        group_label(group)
    )
}

/// Back-substitute using each row's actual pivot column.
fn back_substitute(
    augmented: &[(Vec<f64>, f64)],
    pivot_columns: &[usize],
    width: usize,
) -> Vec<f64> {
    let mut values = vec![0.0; width];
    for (row_index, &column) in pivot_columns.iter().enumerate().rev() {
        let row = &augmented[row_index];
        let known: f64 = row.0[column + 1..]
            .iter()
            .zip(values[column + 1..].iter())
            .map(|(coeff, value)| coeff * value)
            .sum();
        values[column] = row.1 - known;
    }
    values
}

/// Human-readable group label for diagnostics.
fn group_label(group: &ComponentGroup<'_>) -> Vec<String> {
    let mut names: Vec<String> =
        group.components.iter().map(|component| component.instance.clone()).collect();
    names.sort();
    names
}

/// Human-readable variable label for diagnostics.
fn variable_label(variable: &SystemVariable) -> String {
    match variable {
        SystemVariable::Net(net) => format!("V({net})"),
        SystemVariable::Current { component, pin } => format!("I({component}.{pin})"),
    }
}
