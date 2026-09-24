// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Piecewise-linear constitutive-law DC solve (2026-09-24 component laws).
//!
//! Always-active laws solve directly. Guarded laws are solved by deterministic
//! branch-mode enumeration: every mode supplies its active equations, a
//! candidate is accepted only if every guard has its selected truth value, and
//! distinct valid candidates are multiple operating points (never silently
//! reduced to one). Law-bearing components are grouped by connected nets so an
//! incomplete group cannot make an unrelated group look underdetermined.

use crate::analysis::electronics::{ComponentInstance, Net, PinRef, TypeInfo};
use crate::analysis::electronics_laws::{ComponentLaws, LawGuard, LawVariable};
use std::collections::{BTreeMap, BTreeSet};

/// One solved DC operating point.
#[derive(Debug, Clone, Default)]
pub struct DcSolution {
    /// Net name → solved or boundary voltage.
    pub net_voltage: BTreeMap<String, f64>,
    /// Instance + expanded pin → solved branch current.
    pub pin_current: BTreeMap<(String, String), f64>,
    /// Deterministic participation/branch labels for proof provenance.
    pub states: BTreeSet<String>,
}

/// Inputs for one solve pass. Bundled to keep the public entry under the
/// parameter gate.
pub struct DcContext<'a> {
    pub nets: &'a [Net],
    pub components: &'a [ComponentLaws],
    pub instances: &'a BTreeMap<String, &'a ComponentInstance>,
    pub type_pins: &'a BTreeMap<String, Vec<(String, u64)>>,
    pub type_info: &'a BTreeMap<String, TypeInfo>,
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

/// Deterministic branch budget: exponential enumeration must stay explicit.
const MAX_GUARDED_MODES: usize = 12;

/// Deterministic whole-board state budget: independent group modes multiply,
/// so the total must be bounded too.
const MAX_OPERATING_STATES: usize = 64;

/// One active/inactive assignment of the group's guarded laws.
struct BranchMode<'a> {
    laws: Vec<&'a crate::analysis::electronics_laws::ElaboratedLaw>,
    mask: usize,
}

impl<'a> BranchMode<'a> {
    /// Is `law` active in this mode?
    fn is_active(&self, law: &crate::analysis::electronics_laws::ElaboratedLaw) -> bool {
        law.guard == LawGuard::Always
            || self
                .laws
                .iter()
                .position(|guard| std::ptr::eq(*guard, law))
                .map(|index| self.mask & (1 << index) != 0)
                .unwrap_or(false)
    }
}

/// Solve every law group and combine group candidates into global operating
/// states. Any group failure suppresses all states: a partial board state is
/// never presented as a proof.
pub fn solve_dc_laws(ctx: &DcContext<'_>) -> (Vec<DcSolution>, Vec<String>) {
    let groups = component_groups(ctx);
    let mut group_candidates: Vec<Vec<DcSolution>> = Vec::new();
    let mut errors = Vec::new();
    for group in groups {
        match solve_group(&group, ctx) {
            Ok(candidates) => group_candidates.push(candidates),
            Err(error) => errors.push(error),
        }
    }
    if !errors.is_empty() {
        return (Vec::new(), errors);
    }
    match global_states(group_candidates) {
        Ok(states) => (states, Vec::new()),
        Err(error) => (Vec::new(), vec![error]),
    }
}

/// Combine independent group candidates into deterministic global states.
fn global_states(group_candidates: Vec<Vec<DcSolution>>) -> Result<Vec<DcSolution>, String> {
    group_candidates.into_iter().try_fold(
        vec![DcSolution::default()],
        |states, candidates| {
            let product = states.len().saturating_mul(candidates.len());
            if product > MAX_OPERATING_STATES {
                return Err(format!(
                    "component-law solve has {product} operating states — the deterministic state \
                     budget is {MAX_OPERATING_STATES}. Split independent law groups, simplify \
                     guarded laws, or remove unused branch modes"
                ));
            }
            let mut combined = Vec::with_capacity(product);
            for state in &states {
                combine_state_candidates(state, &candidates, &mut combined);
            }
            Ok(combined)
        },
    )
}

/// Cross one existing global state with every candidate for the next group.
fn combine_state_candidates(
    state: &DcSolution,
    candidates: &[DcSolution],
    combined: &mut Vec<DcSolution>,
) {
    for candidate in candidates {
        let mut merged = state.clone();
        merged.net_voltage.extend(candidate.net_voltage.clone());
        merged.pin_current.extend(candidate.pin_current.clone());
        merged.states.extend(candidate.states.clone());
        combined.push(merged);
    }
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

/// Build and solve one connected law group across all guarded branch modes.
fn solve_group(
    group: &ComponentGroup<'_>,
    ctx: &DcContext<'_>,
) -> Result<Vec<DcSolution>, String> {
    let laws = guarded_laws(group);
    let guards = BranchMode { laws, mask: 0 };
    if guards.laws.len() > MAX_GUARDED_MODES {
        return Err(format!(
            "component-law group {:?} has {} guarded laws — split or simplify it; the DC substrate enumerates at most {} branch modes",
            group_label(group),
            guards.laws.len(),
            MAX_GUARDED_MODES
        ));
    }
    let mask_count = 1usize << guards.laws.len();
    let mut candidates: Vec<DcSolution> = Vec::new();
    let mut rejections = Vec::new();
    let mut seen = BTreeSet::new();
    for mask in 0..mask_count {
        let branch = BranchMode { laws: guards.laws.clone(), mask };
        match solve_branch_mode(group, ctx, &branch) {
            Ok(mut solution) => {
                // Mode `00` in an unguarded group is the ordinary direct law
                // path; numbered labels stay uniform for state provenance.
                solution.states.insert(format!("mode={mask:02x}"));
                let key = format!("{solution:?}");
                if seen.insert(key) {
                    candidates.push(solution);
                }
            }
            Err(error) => rejections.push(error),
        }
    }
    if candidates.is_empty() {
        if guards.laws.is_empty() {
            return Err(rejections.remove(0));
        }
        if rejections.iter().any(|error| error.contains("no unique")) {
            return Err(rejections
                .iter()
                .find(|error| error.contains("no unique"))
                .cloned()
                .unwrap_or_default());
        }
        return Err(format!(
            "component-law group {:?} has no DC operating point — no guarded branch mode is consistent with its boundaries",
            group_label(group)
        ));
    }
    if candidates.len() > 1 && !group_bistable(group, ctx) {
        let states = candidates.len();
        return Err(format!(
            "component-law group {:?} has {states} DC operating points — every guarded-law contributor must declare `spec Bistable: true;` before the board is checked in all states",
            group_label(group)
        ));
    }
    Ok(candidates)
}

/// Authority check for retaining multiple group modes. Every component that
/// contributes a guarded law must acknowledge bistability; always-active
/// components may share the connected group without making it ambiguous.
fn group_bistable(group: &ComponentGroup<'_>, ctx: &DcContext<'_>) -> bool {
    group.components.iter().all(|component| {
        let has_guarded = component.laws.iter().any(|law| law.guard != LawGuard::Always);
        !has_guarded
            || ctx
                .instances
                .get(&component.instance)
                .and_then(|inst| ctx.type_info.get(&inst.type_name))
                .is_some_and(|info| info.bistable)
    })
}

/// Guarded laws in deterministic original order.
fn guarded_laws<'a>(group: &'a ComponentGroup<'a>) -> Vec<&'a crate::analysis::electronics_laws::ElaboratedLaw> {
    group
        .components
        .iter()
        .flat_map(|component| component.laws.iter())
        .filter(|law| law.guard != LawGuard::Always)
        .collect()
}

/// Solve one active/inactive assignment of every guarded law. `None` means
/// that this branch mode is not a candidate, not a whole-group failure.
fn solve_branch_mode(
    group: &ComponentGroup<'_>,
    ctx: &DcContext<'_>,
    branch: &BranchMode<'_>,
) -> Result<DcSolution, String> {
    let mut rows = Vec::new();
    let mut variables = VariableTable::default();
    append_law_equations(group, ctx, &mut variables, &mut rows, branch)?;
    append_kcl(group, ctx, &mut variables, &mut rows);
    let values = solve_linear_system(rows, &variables, group)?;
    if !mode_guards_hold(group, branch, &values, ctx) {
        return Err("guard does not hold".to_string());
    }
    Ok(values_to_solution(group, ctx, values))
}

/// Does every law's guard have exactly the truth value selected by `mask`?
fn mode_guards_hold(
    group: &ComponentGroup<'_>,
    branch: &BranchMode<'_>,
    values: &BTreeMap<SystemVariable, f64>,
    ctx: &DcContext<'_>,
) -> bool {
    branch
        .laws
        .iter()
        .all(|law| {
            let holds = law_guard_holds(group, law, values, ctx);
            holds == branch.is_active(law)
        })
}

/// Evaluate one guard as a linear comparison.
fn law_guard_holds(
    group: &ComponentGroup<'_>,
    law: &crate::analysis::electronics_laws::ElaboratedLaw,
    values: &BTreeMap<SystemVariable, f64>,
    ctx: &DcContext<'_>,
) -> bool {
    let LawGuard::Comparison { left, op, right } = &law.guard else { return true };
    let left = linear_value(left, group, values, ctx);
    let right = linear_value(right, group, values, ctx);
    let difference = left - right;
    match op {
        crate::ast::BinaryOpKind::Eq => difference.abs() <= f64::EPSILON,
        crate::ast::BinaryOpKind::Neq => difference.abs() > f64::EPSILON,
        crate::ast::BinaryOpKind::Lt => difference < -f64::EPSILON,
        crate::ast::BinaryOpKind::Gt => difference > f64::EPSILON,
        crate::ast::BinaryOpKind::Le => difference <= f64::EPSILON,
        crate::ast::BinaryOpKind::Ge => difference >= -f64::EPSILON,
        _ => false,
    }
}

/// Evaluate a linear expression from solved values plus fixed boundaries.
fn linear_value(
    expression: &crate::analysis::electronics_laws::LinearExpression,
    group: &ComponentGroup<'_>,
    values: &BTreeMap<SystemVariable, f64>,
    ctx: &DcContext<'_>,
) -> f64 {
    let mut total = expression.constant;
    for (variable, coefficient) in &expression.terms {
        let Some(system_variable) = system_variable(variable, ctx) else { continue };
        let value = match &system_variable {
            SystemVariable::Net(net) => ctx.drives.get(net).copied().unwrap_or_else(|| {
                values
                    .get(&system_variable)
                    .copied()
                    .unwrap_or_else(|| unknown_boundary(group))
            }),
            _ => values.get(&system_variable).copied().unwrap_or(0.0),
        };
        total += coefficient * value;
    }
    total
}

/// Deterministic missing-value fallback (should be unreachable after solve).
fn unknown_boundary(group: &ComponentGroup<'_>) -> f64 {
    let _ = group_label(group);
    0.0
}

/// Convert solved system variables plus fixed boundaries to a solution.
fn values_to_solution(
    group: &ComponentGroup<'_>,
    ctx: &DcContext<'_>,
    values: BTreeMap<SystemVariable, f64>,
) -> DcSolution {
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
    solution
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
    branch: &BranchMode<'_>,
) -> Result<(), String> {
    for component in &group.components {
        component_law_rows(component, ctx, variables, rows, branch)?;
    }
    Ok(())
}

/// Add one component's always-active equations to the system.
fn component_law_rows(
    component: &ComponentLaws,
    ctx: &DcContext<'_>,
    variables: &mut VariableTable,
    rows: &mut Vec<SystemRow>,
    branch: &BranchMode<'_>,
) -> Result<(), String> {
    for law in &component.laws {
        if branch.is_active(law) {
            law_equation_rows(law, ctx, variables, rows)?;
        }
    }
    Ok(())
}

/// Add the equations of one already-selected law. Mode selection (not this
/// function) decides whether a guarded branch is active.
fn law_equation_rows(
    law: &crate::analysis::electronics_laws::ElaboratedLaw,
    ctx: &DcContext<'_>,
    variables: &mut VariableTable,
    rows: &mut Vec<SystemRow>,
) -> Result<(), String> {
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
