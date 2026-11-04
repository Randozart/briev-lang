// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b — loop-shape analysis over HIR.
//!
//! Mirror of `analysis::loop_shape` (AST, still authoritative until parity).
//! Reuses `Bound` / `VectorGroup`; defines HIR-typed `HirLoopShape` /
//! `HirConvergence` / `HirProgramConvergence` because the AST versions embed
//! AST expressions.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::expr_walk::walk_stmts;
use super::ids::NodeId;
use super::loop_carried::classify_fields;
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::slp_isomorphism::analyze_body;
use super::stmt::{HirStmt, HirStmtKind};
use super::storage_class::collect_raw_idents;
use super::swan_song::{has_swan_song, hoist_swan_song};
use super::transition_graph::{HirReactorNode, HirReactorTransitionGraph};
use crate::analysis::loop_carried::FieldClass;
use crate::analysis::loop_shape::{Bound, VectorGroup};
use crate::analysis::transition_graph::{BoundedPre, ConvergeDirection};
use crate::ast::Type;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub enum HirConvergence {
    CounterGeBound { counter: String, bound: Bound },
    Explicit(HirExpr),
    Unprovable,
}

#[derive(Debug, Clone)]
pub struct HirLoopShape {
    pub txn_name: String,
    pub counter: String,
    pub bound: Bound,
    pub direction: ConvergeDirection,
    pub counter_only_writes: bool,
    pub field_classes: BTreeMap<String, FieldClass>,
    pub carried_fields: Vec<String>,
    pub local_fields: Vec<String>,
    pub vector_groups: Vec<VectorGroup>,
    pub carried_float_count: usize,
    pub carried_scalar_count: usize,
    pub carried_aggregate_count: usize,
    pub temp_mediated_float_count: usize,
    pub has_swan_song: bool,
    pub is_pure: bool,
    pub convergence: HirConvergence,
}

#[derive(Debug, Clone)]
pub struct HirProgramConvergence {
    pub counter_ge_bounds: Vec<(String, HirExpr)>,
    pub has_natural_exit: bool,
}

struct ItemNames {
    state_fields: HashSet<String>,
    consts: HashSet<String>,
    inits: HashSet<String>,
}

impl ItemNames {
    fn collect(program: &HirProgram) -> ItemNames {
        ItemNames {
            state_fields: program.globals.iter().map(|g| g.name.clone()).collect(),
            consts: program.constants.iter().cloned().collect(),
            inits: program.inits.iter().map(|i| i.name.clone()).collect(),
        }
    }
}

pub fn build_loop_shapes(
    program: &HirProgram,
    graph: &HirReactorTransitionGraph,
    min_width: usize,
) -> HashMap<String, HirLoopShape> {
    let names = ItemNames::collect(program);
    let txns = collect_txns(program);
    let mut shapes = HashMap::new();
    for node in &graph.nodes {
        if !node.is_reactive {
            continue;
        }
        let (Some(bp), Some(inc)) = (&node.bounded_pre, &node.increments) else {
            continue;
        };
        if bp.var != inc.var {
            continue;
        }
        let Some(txn) = txns.get(&node.name) else {
            continue;
        };
        shapes.insert(
            node.name.clone(),
            build_shape(node, txn, &names, min_width, program),
        );
    }
    shapes
}

pub fn program_convergence(
    program: &HirProgram,
    graph: &HirReactorTransitionGraph,
    has_explicit_exit: bool,
) -> HirProgramConvergence {
    if has_explicit_exit {
        return HirProgramConvergence {
            counter_ge_bounds: Vec::new(),
            has_natural_exit: false,
        };
    }
    let txns = collect_txns(program);
    let has_persistent_txn = txns.values().any(|t| {
        t.is_reactive
            && !graph.nodes.iter().any(|n| {
                n.name == t.name && n.bounded_pre.is_some() && n.increments.is_some()
            })
    });
    if has_persistent_txn {
        return HirProgramConvergence {
            counter_ge_bounds: Vec::new(),
            has_natural_exit: false,
        };
    }
    let mut counter_ge_bounds: Vec<(String, HirExpr)> = Vec::new();
    for (name, t) in &txns {
        if !t.is_reactive {
            continue;
        }
        let Some(node) = graph.nodes.iter().find(|n| n.name == *name) else {
            continue;
        };
        let (Some(bp), Some(inc)) = (&node.bounded_pre, &node.increments) else {
            continue;
        };
        if bp.var == inc.var {
            let bound_expr = match bp.bound_literal {
                Some(n) => int_lit(n),
                None => HirExpr {
                    id: NodeId(0),
                    span: None,
                    ty: None,
                    kind: HirExprKind::Identifier(bp.bound_var.clone()),
                },
            };
            counter_ge_bounds.push((bp.var.clone(), bound_expr));
        }
    }
    counter_ge_bounds.sort_by(|a, b| a.0.cmp(&b.0));
    let has_natural_exit = !counter_ge_bounds.is_empty();
    HirProgramConvergence {
        counter_ge_bounds,
        has_natural_exit,
    }
}

fn int_lit(n: i64) -> HirExpr {
    HirExpr {
        id: NodeId(0),
        span: None,
        ty: None,
        kind: HirExprKind::Literal(HirLiteral::Int(n)),
    }
}

fn build_shape(
    node: &HirReactorNode,
    txn: &HirCallable,
    names: &ItemNames,
    min_width: usize,
    program: &HirProgram,
) -> HirLoopShape {
    let bp = node.bounded_pre.as_ref().unwrap();
    let bound = resolve_bound(bp, names);
    let counter_only_writes = node.write_set.len() == 1 && node.write_set.contains(&bp.var);
    let field_classes = classify_write_fields(node.write_set.clone(), txn);
    let carried_fields = field_classes
        .iter()
        .filter(|(_, class)| **class == FieldClass::LoopCarried)
        .map(|(field, _)| field.clone())
        .collect::<Vec<_>>();
    let local_fields = classify_local_fields(node, txn, program);
    let field_pressure = classify_field_pressure(&carried_fields, program);
    let vector_groups = detect_vector_groups_structural(
        txn,
        node.write_set.clone(),
        &names.state_fields,
        min_width,
    );
    let temp_mediated_float_count =
        count_temp_mediated_float_writes(txn, &carried_fields, program);
    let has_swan = has_swan_song(&txn.body.statements);
    let is_pure = node.is_pure_body || node.is_effectively_pure;
    HirLoopShape {
        txn_name: node.name.clone(),
        counter: bp.var.clone(),
        bound: bound.clone(),
        direction: bp.direction,
        counter_only_writes,
        field_classes,
        carried_fields,
        local_fields,
        vector_groups,
        carried_float_count: field_pressure.float,
        carried_scalar_count: field_pressure.scalar,
        carried_aggregate_count: field_pressure.aggregate,
        temp_mediated_float_count,
        has_swan_song: has_swan,
        is_pure,
        convergence: HirConvergence::CounterGeBound {
            counter: bp.var.clone(),
            bound,
        },
    }
}

fn resolve_bound(bp: &BoundedPre, names: &ItemNames) -> Bound {
    if let Some(lit) = bp.bound_literal {
        return Bound::Literal(lit);
    }
    if names.state_fields.contains(&bp.bound_var) {
        return Bound::Field(bp.bound_var.clone());
    }
    if names.consts.contains(&bp.bound_var) {
        return Bound::Const(bp.bound_var.clone());
    }
    if names.inits.contains(&bp.bound_var) {
        return Bound::Init(bp.bound_var.clone());
    }
    Bound::Unknown(bp.bound_var.clone())
}

fn is_same_txn(a: &HirCallable, b: &HirCallable) -> bool {
    a.id == b.id
}

fn classify_local_fields(
    node: &HirReactorNode,
    txn: &HirCallable,
    program: &HirProgram,
) -> Vec<String> {
    let mut external: HashSet<String> = HashSet::new();
    if let Some(ct) = &txn.contract {
        collect_raw_idents(&ct.pre, &mut external);
        collect_raw_idents(&ct.post, &mut external);
    }
    for c in &program.callables {
        if is_same_txn(c, txn) {
            continue;
        }
        collect_callable_surface(c, &mut external);
    }
    for g in &program.globals {
        if let Some(e) = &g.expr {
            collect_raw_idents(e, &mut external);
        }
    }
    for (_, e) in &program.const_exprs {
        collect_raw_idents(e, &mut external);
    }
    for init in &program.inits {
        if let Some(e) = &init.value {
            collect_raw_idents(e, &mut external);
        }
        walk_stmts(&init.body, &mut |e| collect_raw_idents(e, &mut external));
    }
    let mut local: Vec<String> = node
        .write_set
        .iter()
        .filter(|f| !external.contains(*f))
        .cloned()
        .collect();
    local.sort();
    local
}

fn collect_callable_surface(c: &HirCallable, out: &mut HashSet<String>) {
    if let Some(ct) = &c.contract {
        collect_raw_idents(&ct.pre, out);
        collect_raw_idents(&ct.post, out);
        if let Some(wd) = &ct.watchdog {
            collect_raw_idents(&wd.condition, out);
            if let Some(fb) = &wd.fallback {
                collect_raw_idents(fb, out);
            }
            if let Some(arg) = wd.on_fire.as_ref().and_then(|f| f.arg.as_deref()) {
                out.insert(arg.to_string());
            }
        }
    }
    walk_stmts(&c.body.statements, &mut |e| collect_raw_idents(e, out));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PressureClass {
    Float,
    Scalar,
    Aggregate,
}

#[derive(Debug, Clone, Copy, Default)]
struct FieldPressure {
    float: usize,
    scalar: usize,
    aggregate: usize,
}

fn classify_field_pressure(fields: &[String], program: &HirProgram) -> FieldPressure {
    let (types, exprs) = field_decls(program);
    let mut out = FieldPressure::default();
    for f in fields {
        match pressure_class(types.get(f).and_then(|t| t.as_ref()), exprs.get(f)) {
            PressureClass::Float => out.float += 1,
            PressureClass::Scalar => out.scalar += 1,
            PressureClass::Aggregate => out.aggregate += 1,
        }
    }
    out.scalar += out.float;
    out
}

fn field_decls(
    program: &HirProgram,
) -> (HashMap<String, Option<Type>>, HashMap<String, HirExpr>) {
    let mut types: HashMap<String, Option<Type>> = HashMap::new();
    let mut exprs: HashMap<String, HirExpr> = HashMap::new();
    for g in &program.globals {
        types.insert(g.name.clone(), g.ty.clone());
        if let Some(e) = &g.expr {
            exprs.insert(g.name.clone(), e.clone());
        }
    }
    (types, exprs)
}

fn pressure_class(ty: Option<&Type>, expr: Option<&HirExpr>) -> PressureClass {
    if let Some(ty) = ty {
        return type_pressure_class(ty);
    }
    match expr.map(|e| &e.kind) {
        Some(HirExprKind::Literal(HirLiteral::Float(_))) => PressureClass::Float,
        Some(HirExprKind::Literal(
            HirLiteral::Int(_) | HirLiteral::Bool(_) | HirLiteral::Char(_),
        )) => PressureClass::Scalar,
        _ => PressureClass::Aggregate,
    }
}

fn type_pressure_class(ty: &Type) -> PressureClass {
    match ty {
        Type::Bits(_) => PressureClass::Scalar,
        Type::Ptr(_) | Type::PtrConst(_) => PressureClass::Scalar,
        Type::Custom(name) => match name.as_str() {
            "Float" | "Float16" | "Float32" | "Float64" | "Half" | "Double" => {
                PressureClass::Float
            }
            "Int" | "UInt" | "Int8" | "Int16" | "Int32" | "Int64" | "Int128" | "UInt8"
            | "UInt16" | "UInt32" | "UInt64" | "UInt128" | "Bool" | "Char" | "String"
            | "Blob" | "Data" => PressureClass::Scalar,
            _ => PressureClass::Aggregate,
        },
        _ => PressureClass::Aggregate,
    }
}

fn count_temp_mediated_float_writes(
    txn: &HirCallable,
    carried: &[String],
    program: &HirProgram,
) -> usize {
    let lets = body_let_names(&txn.body.statements);
    let carried_float = carried_float_names(carried, program);
    let mut seen: HashSet<&str> = HashSet::new();
    for stmt in &txn.body.statements {
        let HirStmtKind::Assign(lhs, HirExpr {
            kind: HirExprKind::Identifier(rhs),
            ..
        }) = &stmt.kind
        else {
            continue;
        };
        let HirExprKind::Identifier(field) = &lhs.kind else {
            continue;
        };
        if carried_float.contains(field) && lets.contains(rhs) {
            seen.insert(field);
        }
    }
    seen.len()
}

fn body_let_names(body: &[HirStmt]) -> HashSet<String> {
    body.iter()
        .filter_map(|stmt| match &stmt.kind {
            HirStmtKind::Let { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn carried_float_names(carried: &[String], program: &HirProgram) -> HashSet<String> {
    let (types, exprs) = field_decls(program);
    carried
        .iter()
        .filter(|f| {
            matches!(
                pressure_class(types.get(*f).and_then(|t| t.as_ref()), exprs.get(*f)),
                PressureClass::Float
            )
        })
        .cloned()
        .collect()
}

pub fn prefer_effect_carry(
    shape: &HirLoopShape,
    float_registers: usize,
    vector_min_width: usize,
    temp_min: usize,
) -> bool {
    if shape.carried_float_count == 0
        || shape.temp_mediated_float_count >= shape.carried_float_count
        || shape.carried_float_count > float_registers
    {
        return false;
    }
    let partial_capture = shape.temp_mediated_float_count >= temp_min.max(1);
    let min_width_group = shape
        .vector_groups
        .iter()
        .any(|g| g.width <= vector_min_width.max(1));
    partial_capture || min_width_group
}

pub fn all_direct_loops_prefer_carry(
    shapes: &HashMap<String, HirLoopShape>,
    float_registers: usize,
    vector_min_width: usize,
    temp_min: usize,
) -> bool {
    let mut any = false;
    for shape in shapes.values() {
        if shape.carried_float_count == 0 {
            continue;
        }
        any = true;
        if !prefer_effect_carry(shape, float_registers, vector_min_width, temp_min) {
            return false;
        }
    }
    any
}

pub fn program_carry_mode(
    shapes: &HashMap<String, HirLoopShape>,
    batch_txns: &HashSet<String>,
    float_registers: usize,
    vector_min_width: usize,
    temp_min: usize,
) -> bool {
    if !all_direct_loops_prefer_carry(shapes, float_registers, vector_min_width, temp_min) {
        return false;
    }
    shapes
        .iter()
        .all(|(name, shape)| shape.carried_float_count == 0 || batch_txns.contains(name))
}

pub fn escaped_effect_cause(
    shapes: &HashMap<String, HirLoopShape>,
    float_registers: usize,
    fp_reserved_temps: usize,
) -> Option<String> {
    if float_registers >= u32::MAX as usize {
        return None;
    }
    if float_registers.saturating_sub(fp_reserved_temps) == 0 {
        return None;
    }
    let mut names: Vec<&String> = shapes.keys().collect();
    names.sort();
    for name in names {
        let shape = &shapes[name];
        if shape.carried_aggregate_count != 0 || shape.carried_float_count == 0 {
            continue;
        }
        let fully_temp_mediated =
            shape.temp_mediated_float_count == shape.carried_float_count;
        if !fully_temp_mediated {
            return Some(name.clone());
        }
    }
    None
}

fn classify_write_fields(
    write_set: HashSet<String>,
    txn: &HirCallable,
) -> BTreeMap<String, FieldClass> {
    let (pre, post) = match &txn.contract {
        Some(ct) => (&ct.pre, &ct.post),
        None => return BTreeMap::new(),
    };
    let observables = collect_observable_bodies(&txn.body.statements);
    classify_fields(&write_set, &txn.body.statements, &[pre, post], &observables)
        .into_iter()
        .collect()
}

fn collect_observable_bodies(body: &[HirStmt]) -> Vec<&[HirStmt]> {
    body.iter()
        .filter_map(|stmt| match &stmt.kind {
            HirStmtKind::Guarded(_, stmts) => Some(stmts.as_slice()),
            _ => None,
        })
        .collect()
}

fn detect_vector_groups_structural(
    txn: &HirCallable,
    write_set: HashSet<String>,
    state_fields: &HashSet<String>,
    min_width: usize,
) -> Vec<VectorGroup> {
    let (stripped, _hoist) = hoist_swan_song(&txn.body.statements, state_fields);
    let mut candidates = analyze_body(&stripped, min_width);
    candidates.sort_by_key(|c| std::cmp::Reverse(c.width));
    let mut accepted_fields: HashSet<String> = HashSet::new();
    let mut groups: Vec<VectorGroup> = Vec::new();
    for c in candidates {
        if !c.fields.iter().all(|f| write_set.contains(f)) {
            continue;
        }
        if c.width.count_ones() != 1 {
            continue;
        }
        let mut seen_fields: HashSet<&str> = HashSet::new();
        if c.fields.iter().any(|f| !seen_fields.insert(f)) {
            continue;
        }
        if c.fields.iter().any(|f| accepted_fields.contains(f)) {
            continue;
        }
        for f in &c.fields {
            accepted_fields.insert(f.clone());
        }
        groups.push(VectorGroup {
            name: c.group_name.clone(),
            width: c.width,
            fields: c.fields.clone(),
        });
    }
    groups
}

fn collect_txns(program: &HirProgram) -> HashMap<String, &HirCallable> {
    let mut txns = HashMap::new();
    for c in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Transaction) && c.owner.is_none()
    }) {
        txns.insert(c.name.clone(), c);
    }
    txns
}
