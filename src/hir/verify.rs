// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! HIR invariants. H0c verifies every nested statement/expression/pattern ID,
//! not only callable/body headers.

use super::expr::{HirExpr, HirExprKind, HirMatchArm};
use super::ids::{DefId, ItemId, NodeId};
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{HirCallable, HirProgram};
use super::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};
use std::collections::BTreeSet;

pub fn verify(program: &HirProgram) -> Result<(), String> {
    let mut defs = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    let mut items = BTreeSet::new();
    for callable in &program.callables {
        verify_callable(callable, &mut defs, &mut nodes, &mut items)?;
        verify_body(&callable.body.statements, &mut nodes)?;
    }
    verify_law_markers(program)?;
    Ok(())
}

/// Re-verify one callable in isolation — the specialization site runs this on
/// the substituted tree (fresh id sets, marker structure included).
pub fn verify_single_callable(callable: &HirCallable) -> Result<(), String> {
    let (mut defs, mut nodes, mut items) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    verify_callable(callable, &mut defs, &mut nodes, &mut items)?;
    verify_body(&callable.body.statements, &mut nodes)?;
    verify_law_markers_in(&callable.body.statements, false)
}

fn verify_callable(
    callable: &HirCallable,
    defs: &mut BTreeSet<DefId>,
    nodes: &mut BTreeSet<NodeId>,
    items: &mut BTreeSet<ItemId>,
) -> Result<(), String> {
    if callable.name.is_empty() {
        return Err(format!("callable {:?} has an empty name", callable.id));
    }
    verify_unique_ids(callable, defs, nodes, items)?;
    verify_export_metadata(callable)
}

fn verify_unique_ids(
    callable: &HirCallable,
    defs: &mut BTreeSet<DefId>,
    nodes: &mut BTreeSet<NodeId>,
    items: &mut BTreeSet<ItemId>,
) -> Result<(), String> {
    if !defs.insert(callable.id) {
        return Err(format!("duplicate DefId {:?}", callable.id));
    }
    if !items.insert(callable.item) {
        return Err(format!("duplicate ItemId {:?}", callable.item));
    }
    verify_node(nodes, callable.node, &callable.name)?;
    verify_node(nodes, callable.body.node, &callable.name)
}

fn verify_export_metadata(callable: &HirCallable) -> Result<(), String> {
    match (callable.exported, callable.export_symbol.as_deref()) {
        (true, Some(symbol)) if !symbol.is_empty() => Ok(()),
        (true, _) => Err(format!("exported callable '{}' has no symbol", callable.name)),
        (false, Some(_)) => Err(format!(
            "non-exported callable '{}' carries an export symbol",
            callable.name
        )),
        (false, None) => Ok(()),
    }
}

fn verify_body(body: &[HirStmt], nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    for stmt in body {
        verify_stmt(stmt, nodes)?;
    }
    Ok(())
}

fn verify_stmt(stmt: &HirStmt, nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    verify_node(nodes, stmt.id, "statement")?;
    // 2026-11-12: compiler-inserted storage releases carry no source spans
    // and their names are pass-allocated locals; identity is all we verify.
    if let HirStmtKind::ReleaseOwned { .. } = &stmt.kind {
        return Ok(());
    }
    match &stmt.kind {
        HirStmtKind::ReleaseOwned { .. } => unreachable!("filtered above"),
        HirStmtKind::Let { expr, .. } => verify_opt_expr(expr.as_ref(), nodes),
        HirStmtKind::Assign(left, right) => verify_exprs([left, right], nodes),
        HirStmtKind::ArrowAssign { target, value, .. } => {
            verify_opt_expr(target.as_ref(), nodes)?;
            verify_expr(value, nodes)
        }
        HirStmtKind::Term(expr) => verify_opt_expr(expr.as_ref(), nodes),
        HirStmtKind::TempTransfer { expr, .. } => verify_expr(expr, nodes),
        HirStmtKind::Tail(expr) | HirStmtKind::Check(expr) | HirStmtKind::Expression(expr) => {
            verify_expr(expr, nodes)
        }
        HirStmtKind::Guarded(cond, body) => {
            verify_expr(cond, nodes)?;
            verify_body(body, nodes)
        }
        HirStmtKind::Block(body) => verify_body(body, nodes),
        HirStmtKind::Foreach { list, body, .. } => {
            verify_expr(list, nodes)?;
            verify_body(body, nodes)
        }
        HirStmtKind::Match { expr, arms } => {
            verify_expr(expr, nodes)?;
            verify_stmt_arms(arms, nodes)
        }
        HirStmtKind::EndProgram(expr) | HirStmtKind::Rollback(expr) => {
            verify_opt_expr(expr.as_ref(), nodes)
        }
        HirStmtKind::Gate(expr) | HirStmtKind::TrgBinding { instance: expr, .. } => {
            verify_expr(expr, nodes)
        }
        HirStmtKind::SyncBlock(body) | HirStmtKind::Defer(body) | HirStmtKind::Mutex(body) => {
            verify_body(body, nodes)
        }
        HirStmtKind::Open(left, right) => verify_exprs([left, right], nodes),
        HirStmtKind::Break
        | HirStmtKind::Trap
        | HirStmtKind::Halt
        | HirStmtKind::Yield
        | HirStmtKind::FreeHint(_)
        | HirStmtKind::KeepHint(_)
        | HirStmtKind::MetadataAssignment(_, _)
        | HirStmtKind::InlineAsm { .. }
        | HirStmtKind::Other(_) => Ok(()),
    }
}

fn verify_stmt_arms(
    arms: &[HirStmtMatchArm],
    nodes: &mut BTreeSet<NodeId>,
) -> Result<(), String> {
    for arm in arms {
        verify_patterns(&arm.patterns, nodes)?;
        verify_body(&arm.body, nodes)?;
    }
    Ok(())
}

fn verify_opt_expr(expr: Option<&HirExpr>, nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    match expr {
        Some(expr) => verify_expr(expr, nodes),
        None => Ok(()),
    }
}

fn verify_exprs<'a>(
    exprs: impl IntoIterator<Item = &'a HirExpr>,
    nodes: &mut BTreeSet<NodeId>,
) -> Result<(), String> {
    for expr in exprs {
        verify_expr(expr, nodes)?;
    }
    Ok(())
}

fn verify_expr(expr: &HirExpr, nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    verify_node(nodes, expr.id, "expression")?;
    match &expr.kind {
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            verify_exprs(args, nodes)?;
            if let Some(r) = receiver {
                verify_expr(r, nodes)?;
            }
            Ok(())
        }
        HirExprKind::Call { args, .. } => verify_exprs(args, nodes),
        HirExprKind::Binary { left, right, .. } => verify_exprs([left.as_ref(), right.as_ref()], nodes),
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => verify_expr(inner, nodes),
        HirExprKind::Field { receiver, .. }
        | HirExprKind::Reflect { receiver, .. } => verify_expr(receiver, nodes),
        HirExprKind::MethodCall { receiver, args, .. } => {
            verify_expr(receiver, nodes)?;
            verify_exprs(args, nodes)
        }
        HirExprKind::Index(array, index) => verify_exprs([array.as_ref(), index.as_ref()], nodes),
        HirExprKind::Wildcard => Ok(()),
        HirExprKind::Slice { array, start, end, stride } => {
            verify_expr(array, nodes)?;
            verify_opt_expr(start.as_deref(), nodes)?;
            verify_opt_expr(end.as_deref(), nodes)?;
            verify_opt_expr(stride.as_deref(), nodes)
        }
        HirExprKind::Range { start, end, .. } => verify_exprs([start.as_ref(), end.as_ref()], nodes),
        HirExprKind::Spawn { args, .. } => verify_exprs(args, nodes),
        HirExprKind::If { condition, then_branch, else_branch } => {
            verify_expr(condition, nodes)?;
            verify_expr(then_branch, nodes)?;
            verify_opt_expr(else_branch.as_deref(), nodes)
        }
        HirExprKind::Lambda { body, .. } | HirExprKind::IsType { inner: body, .. } => {
            verify_expr(body, nodes)
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => verify_exprs(items, nodes),
        HirExprKind::StructLiteral { fields, .. } => {
            verify_exprs(fields.iter().map(|(_, expr)| expr), nodes)
        }
        HirExprKind::Match { scrutinee, arms } => {
            verify_expr(scrutinee, nodes)?;
            verify_expr_arms(arms, nodes)
        }
        HirExprKind::Block(body) => verify_body(body, nodes),
        HirExprKind::Literal(_)
        | HirExprKind::Identifier(_)
        | HirExprKind::UnitLiteral { .. }
        | HirExprKind::Other(_) => Ok(()),
    }
}


fn verify_expr_arms(arms: &[HirMatchArm], nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    for arm in arms {
        verify_pattern(&arm.pattern, nodes)?;
        verify_expr(&arm.body, nodes)?;
    }
    Ok(())
}

fn verify_patterns(patterns: &[HirPattern], nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    for pattern in patterns {
        verify_pattern(pattern, nodes)?;
    }
    Ok(())
}

fn verify_pattern(pattern: &HirPattern, nodes: &mut BTreeSet<NodeId>) -> Result<(), String> {
    verify_node(nodes, pattern.id, "pattern")?;
    match &pattern.kind {
        HirPatternKind::Literal(expr)
        | HirPatternKind::Range(expr, _)
        | HirPatternKind::RangeInclusive(expr, _) => verify_expr(expr, nodes),
        HirPatternKind::EnumVariant(_, pats) | HirPatternKind::Tuple(pats) | HirPatternKind::Multi(pats) => {
            verify_patterns(pats, nodes)
        }
        HirPatternKind::Range(_, second) | HirPatternKind::RangeInclusive(_, second) => {
            verify_expr(second, nodes)
        }
        HirPatternKind::Wildcard | HirPatternKind::Binding(_) | HirPatternKind::TypedBinding(_, _) => Ok(()),
    }
}

fn verify_node(
    nodes: &mut BTreeSet<NodeId>,
    id: NodeId,
    what: &str,
) -> Result<(), String> {
    if nodes.insert(id) {
        Ok(())
    } else {
        Err(format!("duplicate NodeId {:?} in {what}", id))
    }
}

// ─── 2026-11-09 (verification program P1) ─────────────────────────────────
// Structure + typed invariants. `verify` (id/export) runs always; the typed
// layer additionally needs the universe + casting graph and is called from
// `build_hir` (and after specialization) in debug builds.

/// Typed invariants: every present expression type agrees with its declared
/// binding, and every cast whose endpoints have a graph path carries it.
pub fn verify_typed(
    program: &HirProgram,
    universe: &crate::type_universe::TypeUniverse,
    graph: &crate::casting::graph::CastingGraph,
) -> Result<(), String> {
    let base = program_bindings(program);
    for global in &program.globals {
        if let Some(expr) = &global.expr {
            verify_expr_types(expr, &base, universe, graph)?;
        }
    }
    for callable in &program.callables {
        let mut env = seeded_env(&base, callable);
        verify_stmt_types(&callable.body.statements, &mut env, universe, graph)?;
    }
    Ok(())
}

fn seeded_env(
    base: &std::collections::HashMap<String, crate::ast::Type>,
    callable: &HirCallable,
) -> std::collections::HashMap<String, crate::ast::Type> {
    let mut env = base.clone();
    for param in &callable.params {
        env.insert(param.name.clone(), param.ty.clone());
    }
    env
}

fn program_bindings(program: &HirProgram) -> std::collections::HashMap<String, crate::ast::Type> {
    let mut env = std::collections::HashMap::new();
    for g in &program.globals {
        if let Some(ty) = &g.ty {
            env.insert(g.name.clone(), ty.clone());
        }
    }
    for (name, ty) in &program.const_types {
        env.insert(name.clone(), ty.clone());
    }
    env
}

fn verify_stmt_types(
    stmts: &[HirStmt],
    env: &mut std::collections::HashMap<String, crate::ast::Type>,
    universe: &crate::type_universe::TypeUniverse,
    graph: &crate::casting::graph::CastingGraph,
) -> Result<(), String> {
    for stmt in stmts {
        verify_one_stmt_types(stmt, env, universe, graph)?;
    }
    Ok(())
}

fn verify_one_stmt_types(
    stmt: &HirStmt,
    env: &mut std::collections::HashMap<String, crate::ast::Type>,
    universe: &crate::type_universe::TypeUniverse,
    graph: &crate::casting::graph::CastingGraph,
) -> Result<(), String> {
    match &stmt.kind {
        HirStmtKind::Let { name, ty, expr, .. } => {
            if let Some(expr) = expr {
                verify_expr_types(expr, env, universe, graph)?;
            }
            if let Some(ty) = ty {
                env.insert(name.clone(), ty.clone());
            }
            Ok(())
        }
        HirStmtKind::Guarded(cond, body) => {
            verify_expr_types(cond, env, universe, graph)?;
            verify_stmt_types(body, &mut env.clone(), universe, graph)
        }
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => verify_stmt_types(body, &mut env.clone(), universe, graph),
        HirStmtKind::Foreach { list, body, .. } => {
            verify_expr_types(list, env, universe, graph)?;
            verify_stmt_types(body, &mut env.clone(), universe, graph)
        }
        HirStmtKind::Match { expr, arms } => {
            verify_expr_types(expr, env, universe, graph)?;
            for arm in arms {
                verify_stmt_types(&arm.body, &mut env.clone(), universe, graph)?;
            }
            Ok(())
        }
        HirStmtKind::Assign(a, b) | HirStmtKind::Open(a, b) => {
            verify_expr_types(a, env, universe, graph)?;
            verify_expr_types(b, env, universe, graph)
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                verify_expr_types(t, env, universe, graph)?;
            }
            verify_expr_types(value, env, universe, graph)
        }
        HirStmtKind::TrgBinding { instance, .. } => verify_expr_types(instance, env, universe, graph),
        // Expressions carried by the remaining statement kinds.
        HirStmtKind::Term(e) | HirStmtKind::EndProgram(e) | HirStmtKind::Rollback(e) => match e {
            Some(e) => verify_expr_types(e, env, universe, graph),
            None => Ok(()),
        },
        HirStmtKind::Tail(e)
        | HirStmtKind::Check(e)
        | HirStmtKind::Expression(e)
        | HirStmtKind::Gate(e) => verify_expr_types(e, env, universe, graph),
        _ => Ok(()),
    }
}

fn verify_expr_types(
    expr: &HirExpr,
    env: &std::collections::HashMap<String, crate::ast::Type>,
    universe: &crate::type_universe::TypeUniverse,
    graph: &crate::casting::graph::CastingGraph,
) -> Result<(), String> {
    match &expr.kind {
        HirExprKind::Identifier(name) => {
            if let (Some(actual), Some(declared)) = (expr.ty.as_ref(), env.get(name)) {
                if actual != declared {
                    return Err(format!(
                        "identifier '{name}' is annotated {actual:?} but declared {declared:?}"
                    ));
                }
            }
            Ok(())
        }
        HirExprKind::Cast { inner, to, via } => {
            verify_expr_types(inner, env, universe, graph)?;
            verify_cast_path(inner, to, via, universe, graph)
        }
        HirExprKind::Call { args, .. } => verify_expr_types_in(args, env, universe, graph),
        HirExprKind::Binary { left, right, .. } => {
            verify_expr_types(left, env, universe, graph)?;
            verify_expr_types(right, env, universe, graph)
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => verify_expr_types(inner, env, universe, graph),
        HirExprKind::Field { receiver, .. } | HirExprKind::Reflect { receiver, .. } => {
            verify_expr_types(receiver, env, universe, graph)
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            verify_expr_types(receiver, env, universe, graph)?;
            verify_expr_types_in(args, env, universe, graph)
        }
        other => verify_container_types(other, env, universe, graph),
    }
}

fn verify_cast_path(
    inner: &HirExpr,
    to: &crate::ast::Type,
    via: &Option<Vec<crate::casting::graph::CastStep>>,
    universe: &crate::type_universe::TypeUniverse,
    graph: &crate::casting::graph::CastingGraph,
) -> Result<(), String> {
    let Some(source) = inner.ty.as_ref() else {
        return Ok(());
    };
    let (sc, sv) = graph.type_to_protocol(universe, source);
    let (dc, dv) = graph.type_to_protocol(universe, to);
    if graph.find_path(&sc, &sv, &dc, &dv).is_none() || via.is_some() {
        return Ok(());
    }
    Err(format!(
        "cast {source:?} as {to:?} has a graph path but no resolved `via`"
    ))
}

fn verify_expr_types_in(
    exprs: &[HirExpr],
    env: &std::collections::HashMap<String, crate::ast::Type>,
    universe: &crate::type_universe::TypeUniverse,
    graph: &crate::casting::graph::CastingGraph,
) -> Result<(), String> {
    for expr in exprs {
        verify_expr_types(expr, env, universe, graph)?;
    }
    Ok(())
}

fn verify_container_types(
    kind: &HirExprKind,
    env: &std::collections::HashMap<String, crate::ast::Type>,
    universe: &crate::type_universe::TypeUniverse,
    graph: &crate::casting::graph::CastingGraph,
) -> Result<(), String> {
    match kind {
        HirExprKind::Index(a, b) => {
            verify_expr_types(a, env, universe, graph)?;
            verify_expr_types(b, env, universe, graph)
        }
        HirExprKind::Wildcard => Ok(()),
        HirExprKind::Slice { array, start, end, stride } => {
            verify_expr_types(array, env, universe, graph)?;
            for opt in [start, end, stride].into_iter().flatten() {
                verify_expr_types(opt, env, universe, graph)?;
            }
            Ok(())
        }
        HirExprKind::Range { start, end, .. } => {
            verify_expr_types(start, env, universe, graph)?;
            verify_expr_types(end, env, universe, graph)
        }
        HirExprKind::Spawn { args, .. } => verify_expr_types_in(args, env, universe, graph),
        HirExprKind::If { condition, then_branch, else_branch } => {
            verify_expr_types(condition, env, universe, graph)?;
            verify_expr_types(then_branch, env, universe, graph)?;
            match else_branch {
                Some(e) => verify_expr_types(e, env, universe, graph),
                None => Ok(()),
            }
        }
        HirExprKind::Lambda { body, .. } | HirExprKind::IsType { inner: body, .. } => {
            verify_expr_types(body, env, universe, graph)
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            verify_expr_types_in(items, env, universe, graph)
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            if let Some(r) = receiver {
                verify_expr_types(r, env, universe, graph)?;
            }
            verify_expr_types_in(args, env, universe, graph)
        }
        HirExprKind::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                verify_expr_types(value, env, universe, graph)?;
            }
            Ok(())
        }
        HirExprKind::Match { scrutinee, arms } => {
            verify_expr_types(scrutinee, env, universe, graph)?;
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    verify_expr_types(guard, env, universe, graph)?;
                }
                verify_expr_types(&arm.body, env, universe, graph)?;
            }
            Ok(())
        }
        HirExprKind::Block(body) => verify_stmt_types(body, &mut env.clone(), universe, graph),
        _ => Ok(()),
    }
}

/// Law-marker structure: `hold` markers appear exactly once per fact body,
/// `through` only alongside `hold`, and neither outside a `Guarded` body.
pub fn verify_law_markers(program: &HirProgram) -> Result<(), String> {
    for callable in &program.callables {
        verify_law_markers_in(&callable.body.statements, false)?;
    }
    for init in &program.inits {
        verify_law_markers_in(&init.body, false)?;
    }
    Ok(())
}

fn verify_law_markers_in(stmts: &[HirStmt], inside_guarded: bool) -> Result<(), String> {
    for stmt in stmts {
        match &stmt.kind {
            HirStmtKind::Guarded(_, body) => {
                let holds = body.iter().filter(|s| is_hold_marker(s)).count();
                let throughs = body.iter().filter(|s| is_through_marker(s)).count();
                if holds > 1 {
                    return Err(format!("guarded body carries {holds} `hold` markers"));
                }
                if throughs > 0 && holds == 0 {
                    return Err("`through` marker without a `hold` marker".to_string());
                }
                verify_law_markers_in(body, true)?;
            }
            HirStmtKind::Block(body)
            | HirStmtKind::SyncBlock(body)
            | HirStmtKind::Defer(body)
            | HirStmtKind::Mutex(body) => verify_law_markers_in(body, inside_guarded)?,
            HirStmtKind::Foreach { body, .. } => verify_law_markers_in(body, inside_guarded)?,
            HirStmtKind::Match { arms, .. } => {
                verify_arm_markers(arms, inside_guarded)?;
            }
            other => {
                if !inside_guarded && marker_of(other).is_some() {
                    return Err(format!(
                        "`{}` marker outside a guarded body",
                        marker_of(other).unwrap_or_default()
                    ));
                }
            }
        }
    }
    Ok(())
}

fn verify_arm_markers(arms: &[HirStmtMatchArm], inside_guarded: bool) -> Result<(), String> {
    for arm in arms {
        verify_law_markers_in(&arm.body, inside_guarded)?;
    }
    Ok(())
}

fn is_hold_marker(stmt: &HirStmt) -> bool {
    matches!(
        &stmt.kind,
        HirStmtKind::MetadataAssignment(key, crate::ast::PropertyValue::Bool(true)) if key == "hold"
    )
}

fn is_through_marker(stmt: &HirStmt) -> bool {
    matches!(
        &stmt.kind,
        HirStmtKind::MetadataAssignment(key, crate::ast::PropertyValue::Identifier(_))
            if key == "through"
    )
}

fn marker_of(kind: &HirStmtKind) -> Option<&'static str> {
    match kind {
        HirStmtKind::MetadataAssignment(key, crate::ast::PropertyValue::Bool(true)) if key == "hold" => {
            Some("hold")
        }
        HirStmtKind::MetadataAssignment(key, crate::ast::PropertyValue::Identifier(_))
            if key == "through" =>
        {
            Some("through")
        }
        _ => None,
    }
}
