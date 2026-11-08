// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b — reactor transition graph over typed HIR.
//!
//! Mirror of `analysis::transition_graph` (AST, still authoritative until
//! parity). Reuses the AST `BoundedPre` / `ConvergeDirection` /
//! `IncrementInfo` value types so consumers cannot drift.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::ids::NodeId;
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::transition_graph::{BoundedPre, ConvergeDirection, IncrementInfo};
use crate::ast::{BinaryOpKind, UnaryOpKind};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct HirReactorNode {
    pub name: String,
    pub is_reactive: bool,
    pub precondition: HirExpr,
    pub body: Vec<HirStmt>,
    pub bounded_pre: Option<BoundedPre>,
    pub increments: Option<IncrementInfo>,
    pub is_pure_body: bool,
    pub write_set: HashSet<String>,
    pub is_effectively_pure: bool,
    pub lexicographic_vars: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HirReactorTransitionGraph {
    pub nodes: Vec<HirReactorNode>,
    pub has_triggers: bool,
    pub live_fields: HashSet<String>,
    /// Reactive txns whose RAW body has an unguarded FFI statement.
    pub has_unguarded_ffi: HashSet<String>,
}

impl HirReactorTransitionGraph {
    pub fn build(
        program: &HirProgram,
        exit_condition: Option<&HirExpr>,
        out_pragmas: &[String],
    ) -> Self {
        let state_fields: HashSet<String> = program.globals.iter().map(|g| g.name.clone()).collect();
        let mut nodes: Vec<HirReactorNode> = program
            .callables
            .iter()
            .filter(|c| matches!(c.kind, HirCallableKind::Transaction) && c.owner.is_none())
            .map(|txn| build_node(txn, &state_fields))
            .collect();

        let mut live_fields = compute_live_fields(exit_condition, out_pragmas, &nodes);
        pin_live_modifier_fields(program, &mut live_fields);
        for node in &mut nodes {
            compute_effectively_pure(node, &live_fields);
        }
        let has_unguarded_ffi = collect_unguarded_ffi(program);

        Self {
            nodes,
            has_triggers: program.has_triggers,
            live_fields,
            has_unguarded_ffi,
        }
    }
}

/// One reactor node's facts (precondition, simplified body, bounded-pre /
/// increment proof, purity, write set, lexicographic ranking).
fn build_node(txn: &HirCallable, state_fields: &HashSet<String>) -> HirReactorNode {
    let precondition = txn
        .contract
        .as_ref()
        .map(|c| c.pre.clone())
        .unwrap_or_else(truth);
    let body_no_term: Vec<HirStmt> = txn
        .body
        .statements
        .iter()
        .filter(|s| {
            !matches!(
                s.kind,
                HirStmtKind::Term(None)
                    | HirStmtKind::EndProgram(None)
                    | HirStmtKind::Tail(_)
            )
        })
        .cloned()
        .collect();
    let simplified_body = simplify_body(&body_no_term);
    let increments = detect_increments(&simplified_body)
        .or_else(|| detect_popcount_decay(&simplified_body));
    let (bounded_pre, increments) = resolve_bounded_pre(&precondition, &simplified_body, increments);
    let is_pure = is_pure_body(&simplified_body, state_fields, &increments);
    let write_set = extract_write_set(&simplified_body, state_fields);
    let lexicographic_vars = detect_lexicographic_ranking(&precondition, &simplified_body);
    HirReactorNode {
        name: txn.name.clone(),
        is_reactive: txn.is_reactive,
        precondition,
        body: simplified_body,
        bounded_pre,
        increments,
        is_pure_body: is_pure,
        is_effectively_pure: false,
        write_set,
        lexicographic_vars,
    }
}

/// Bounded-pre proof: prefer the detected increment, else scan all increments
/// for one the precondition pins.
fn resolve_bounded_pre(
    precondition: &HirExpr,
    body: &[HirStmt],
    increments: Option<IncrementInfo>,
) -> (Option<BoundedPre>, Option<IncrementInfo>) {
    if let Some(bp) = extract_valid_bounded_pre(precondition, &increments) {
        return (Some(bp), increments);
    }
    let matched = detect_all_increments(body).into_iter().find_map(|inc| {
        extract_valid_bounded_pre(precondition, &Some(inc.clone()))
            .map(|b| (b, inc))
    });
    match matched {
        Some((bp, inc)) => (Some(bp), Some(inc)),
        None => (None, increments),
    }
}

/// `out`/`vol` fields are live by declaration regardless of analysis.
fn pin_live_modifier_fields(program: &HirProgram, live_fields: &mut HashSet<String>) {
    for g in &program.globals {
        if g.modifiers.iter().any(|m| m == "out" || m == "vol") {
            live_fields.insert(g.name.clone());
        }
    }
}

/// Reactive txns whose RAW body performs an FFI call outside a guard.
fn collect_unguarded_ffi(program: &HirProgram) -> HashSet<String> {
    let mut out = HashSet::new();
    for txn in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Transaction)
            && c.owner.is_none()
            && !c.exported
            && c.sync_domains.is_empty()
    }) {
        if txn.is_reactive
            && txn.body.statements.iter().any(|stmt| {
                !matches!(stmt.kind, HirStmtKind::Guarded(_, _))
                    && statement_contains_ffi(stmt)
            })
        {
            out.insert(txn.name.clone());
        }
    }
    out
}

fn truth() -> HirExpr {
    HirExpr {
        id: NodeId(0),
        span: None,
        ty: None,
        kind: HirExprKind::Literal(HirLiteral::Bool(true)),
    }
}

fn rebuild(expr: &HirExpr, kind: HirExprKind) -> HirExpr {
    HirExpr { id: expr.id, span: expr.span, ty: expr.ty.clone(), kind }
}

fn lit_int(e: &HirExpr) -> Option<i64> {
    match &e.kind {
        HirExprKind::Literal(HirLiteral::Int(n)) => Some(*n),
        _ => None,
    }
}

fn expr_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Identifier(n) => Some(n.clone()),
        _ => None,
    }
}

fn vars_match(a: &HirExpr, b: &HirExpr) -> bool {
    matches!((&a.kind, &b.kind), (HirExprKind::Identifier(an), HirExprKind::Identifier(bn)) if an == bn)
}

fn is_self_identity(a: &HirExpr, b: &HirExpr) -> bool {
    matches!((&a.kind, &b.kind), (HirExprKind::Identifier(n1), HirExprKind::Identifier(n2)) if n1 == n2)
}

fn extract_bounded_pre(pre: &HirExpr) -> Option<BoundedPre> {
    match &pre.kind {
        HirExprKind::Binary { op: BinaryOpKind::Lt | BinaryOpKind::Le, left, right } => {
            bounded_increasing(left, right)
        }
        HirExprKind::Binary { op: BinaryOpKind::Gt | BinaryOpKind::Ge, left, right } => {
            bounded_decreasing(left, right)
        }
        HirExprKind::Binary { op: BinaryOpKind::Neq, left, right } => bounded_neq(left, right),
        HirExprKind::Binary { op: BinaryOpKind::And, left, right } => bounded_and(left, right),
        _ => None,
    }
}

fn lit_bound(var: &str, n: i64, direction: ConvergeDirection) -> BoundedPre {
    BoundedPre {
        var: var.to_string(),
        bound_var: format!("__lit__{}", var),
        direction,
        bound_literal: Some(n),
    }
}

/// `x < y` / `x <= y` / `x < N` / `x < -N` / `list.Size < …`.
fn bounded_increasing(left: &HirExpr, right: &HirExpr) -> Option<BoundedPre> {
    match (&left.kind, &right.kind) {
        (HirExprKind::Identifier(var), HirExprKind::Identifier(bound)) => Some(BoundedPre {
            var: var.clone(),
            bound_var: bound.clone(),
            direction: ConvergeDirection::Increasing,
            bound_literal: None,
        }),
        (HirExprKind::Identifier(var), HirExprKind::Literal(HirLiteral::Int(n))) => {
            Some(lit_bound(var, *n, ConvergeDirection::Increasing))
        }
        (HirExprKind::Identifier(var), HirExprKind::Unary { op: UnaryOpKind::Neg, inner })
            if matches!(inner.kind, HirExprKind::Literal(HirLiteral::Int(_))) =>
        {
            let n = lit_int(inner).map(|n| -n).unwrap_or(0);
            Some(lit_bound(var, n, ConvergeDirection::Increasing))
        }
        (HirExprKind::Field { receiver, name }, _) if name == "Size" => {
            expr_name(receiver).map(|name| BoundedPre {
                var: name,
                bound_var: "__len_bound".to_string(),
                direction: ConvergeDirection::Increasing,
                bound_literal: None,
            })
        }
        _ => None,
    }
}

/// `x > y` / `x >= y` / `x > N` / `list.Size > 0|N`.
fn bounded_decreasing(left: &HirExpr, right: &HirExpr) -> Option<BoundedPre> {
    match (&left.kind, &right.kind) {
        (HirExprKind::Identifier(var), HirExprKind::Identifier(bound)) => Some(BoundedPre {
            var: var.clone(),
            bound_var: bound.clone(),
            direction: ConvergeDirection::Decreasing,
            bound_literal: None,
        }),
        (HirExprKind::Identifier(var), HirExprKind::Literal(HirLiteral::Int(n))) => {
            Some(lit_bound(var, *n, ConvergeDirection::Decreasing))
        }
        (HirExprKind::Identifier(var), HirExprKind::Unary { op: UnaryOpKind::Neg, inner })
            if matches!(inner.kind, HirExprKind::Literal(HirLiteral::Int(_))) =>
        {
            let n = lit_int(inner).map(|n| -n).unwrap_or(0);
            Some(lit_bound(var, n, ConvergeDirection::Decreasing))
        }
        (HirExprKind::Field { receiver, name }, HirExprKind::Literal(HirLiteral::Int(0)))
            if name == "Size" =>
        {
            expr_name(receiver).map(|name| lit_bound(&name, 0, ConvergeDirection::Decreasing))
        }
        (HirExprKind::Field { receiver, name }, HirExprKind::Literal(HirLiteral::Int(n)))
            if name == "Size" && *n > 0 =>
        {
            expr_name(receiver).map(|name| lit_bound(&name, *n, ConvergeDirection::Decreasing))
        }
        _ => None,
    }
}

/// `x != N` — decreasing bound toward the literal.
fn bounded_neq(left: &HirExpr, right: &HirExpr) -> Option<BoundedPre> {
    let (HirExprKind::Identifier(var), HirExprKind::Literal(HirLiteral::Int(n))) =
        (&left.kind, &right.kind)
    else {
        return None;
    };
    Some(lit_bound(var, *n, ConvergeDirection::Decreasing))
}

/// Conjunction: prefer a literal-bound side.
fn bounded_and(left: &HirExpr, right: &HirExpr) -> Option<BoundedPre> {
    match (extract_bounded_pre(left), extract_bounded_pre(right)) {
        (Some(lp), Some(rp)) => {
            if lp.bound_literal.is_some() {
                Some(rp)
            } else {
                Some(lp)
            }
        }
        (Some(lp), None) => Some(lp),
        (None, Some(rp)) => Some(rp),
        (None, None) => None,
    }
}
fn extract_valid_bounded_pre(pre: &HirExpr, inc: &Option<IncrementInfo>) -> Option<BoundedPre> {
    let bp = extract_bounded_pre(pre)?;
    if inc.as_ref().map_or(false, |i| i.var == bp.var) {
        Some(bp)
    } else {
        None
    }
}

fn simplify_expr(expr: &HirExpr) -> HirExpr {
    let simplified = simplify_children(expr);
    apply_algebraic(expr, simplified)
}

/// Rebuild the expression with simplified children (depth-first).
fn simplify_children(expr: &HirExpr) -> HirExpr {
    match &expr.kind {
        HirExprKind::Binary { op, left, right } => rebuild(
            expr,
            HirExprKind::Binary {
                op: *op,
                left: Box::new(simplify_expr(left)),
                right: Box::new(simplify_expr(right)),
            },
        ),
        HirExprKind::Unary { op, inner } => rebuild(
            expr,
            HirExprKind::Unary { op: *op, inner: Box::new(simplify_expr(inner)) },
        ),
        HirExprKind::Cast { inner, to, via } => rebuild(
            expr,
            HirExprKind::Cast {
                inner: Box::new(simplify_expr(inner)),
                to: to.clone(),
                via: via.clone(),
            },
        ),
        HirExprKind::Index(a, b) => rebuild(
            expr,
            HirExprKind::Index(Box::new(simplify_expr(a)), Box::new(simplify_expr(b))),
        ),
        HirExprKind::Field { receiver, name } => rebuild(
            expr,
            HirExprKind::Field {
                receiver: Box::new(simplify_expr(receiver)),
                name: name.clone(),
            },
        ),
        HirExprKind::List(items) => {
            rebuild(expr, HirExprKind::List(items.iter().map(simplify_expr).collect()))
        }
        HirExprKind::Tuple(items) => {
            rebuild(expr, HirExprKind::Tuple(items.iter().map(simplify_expr).collect()))
        }
        HirExprKind::Block(stmts) => {
            rebuild(expr, HirExprKind::Block(stmts.iter().map(simplify_stmt).collect()))
        }
        _ => expr.clone(),
    }
}

/// Apply the algebraic rewrite for the node's operator (rules are the AST
/// mirror's).
fn apply_algebraic(expr: &HirExpr, simplified: HirExpr) -> HirExpr {
    match &simplified.kind {
        HirExprKind::Binary { op: BinaryOpKind::Sub, left, right } => {
            simplify_sub(expr, left, right, &simplified)
        }
        HirExprKind::Binary { op: BinaryOpKind::Add, left, right } => {
            simplify_add(expr, left, right, &simplified)
        }
        HirExprKind::Binary { op: BinaryOpKind::Mul, left, right } => {
            simplify_mul(expr, left, right, &simplified)
        }
        HirExprKind::Binary { op: BinaryOpKind::Div, left, right } => {
            simplify_div(expr, left, right, &simplified)
        }
        _ => simplified,
    }
}

fn int_lit_is(e: &HirExpr, v: i64) -> bool {
    matches!(&e.kind, HirExprKind::Literal(HirLiteral::Int(n)) if *n == v)
}

fn simplify_sub(expr: &HirExpr, lhs: &HirExpr, rhs: &HirExpr, fallback: &HirExpr) -> HirExpr {
    if let HirExprKind::Binary { op: BinaryOpKind::Add, left: a, right: b } = &lhs.kind {
        if vars_match(a, rhs) {
            return (**b).clone();
        }
        if vars_match(b, rhs) {
            return (**a).clone();
        }
    }
    if let HirExprKind::Binary { op: BinaryOpKind::Sub, left: a, right: b } = &rhs.kind {
        if vars_match(lhs, a) {
            return (**b).clone();
        }
    }
    if let Some(r) = simplify_sub_add_pair(expr, lhs, rhs) {
        return r;
    }
    if let Some(r) = simplify_sub_sub_pair(expr, lhs, rhs) {
        return r;
    }
    if int_lit_is(rhs, 0) {
        return lhs.clone();
    }
    fallback.clone()
}


/// `(a + b1) - (a + b2) -> b1 - b2` (shared prefix cancels).
fn simplify_sub_add_pair(
    expr: &HirExpr,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<HirExpr> {
    if let (
        HirExprKind::Binary { op: BinaryOpKind::Add, left: a1, right: b1 },
        HirExprKind::Binary { op: BinaryOpKind::Add, left: a2, right: b2 },
    ) = (&lhs.kind, &rhs.kind)
    {
        if vars_match(a1, a2) && !vars_match(b1, b2) {
            return Some(rebuild(
                expr,
                HirExprKind::Binary {
                    op: BinaryOpKind::Sub,
                    left: Box::new((**b1).clone()),
                    right: Box::new((**b2).clone()),
                },
            ));
        }
    }
    None
}

/// `(sa - sb) - (sc - sb) -> sa - sc` (shared subtrahend cancels).
fn simplify_sub_sub_pair(
    expr: &HirExpr,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<HirExpr> {
    if let (
        HirExprKind::Binary { op: BinaryOpKind::Sub, left: sa, right: sb1 },
        HirExprKind::Binary { op: BinaryOpKind::Sub, left: sc, right: sb2 },
    ) = (&lhs.kind, &rhs.kind)
    {
        if vars_match(sb1, sb2) {
            return Some(rebuild(
                expr,
                HirExprKind::Binary {
                    op: BinaryOpKind::Sub,
                    left: Box::new((**sa).clone()),
                    right: Box::new((**sc).clone()),
                },
            ));
        }
    }
    None
}

fn simplify_add(expr: &HirExpr, lhs: &HirExpr, rhs: &HirExpr, fallback: &HirExpr) -> HirExpr {
    if let HirExprKind::Binary { op: BinaryOpKind::Sub, left: a, right: b } = &lhs.kind {
        if vars_match(b, rhs) {
            return (**a).clone();
        }
    }
    if int_lit_is(rhs, 0) {
        return lhs.clone();
    }
    if int_lit_is(lhs, 0) {
        return rhs.clone();
    }
    fallback.clone()
}

fn simplify_mul(expr: &HirExpr, lhs: &HirExpr, rhs: &HirExpr, fallback: &HirExpr) -> HirExpr {
    if int_lit_is(rhs, 1) {
        return lhs.clone();
    }
    if int_lit_is(lhs, 1) {
        return rhs.clone();
    }
    if int_lit_is(rhs, 0) || int_lit_is(lhs, 0) {
        return rebuild(expr, HirExprKind::Literal(HirLiteral::Int(0)));
    }
    fallback.clone()
}

fn simplify_div(_expr: &HirExpr, lhs: &HirExpr, rhs: &HirExpr, fallback: &HirExpr) -> HirExpr {
    if int_lit_is(rhs, 1) {
        return lhs.clone();
    }
    fallback.clone()
}

fn simplify_stmt(stmt: &HirStmt) -> HirStmt {
    let kind = match &stmt.kind {
        HirStmtKind::Assign(lhs, rhs) => {
            HirStmtKind::Assign(lhs.clone(), simplify_expr(rhs))
        }
        HirStmtKind::Let { name, names, ty, expr, modifiers } => HirStmtKind::Let {
            name: name.clone(),
            names: names.clone(),
            ty: ty.clone(),
            expr: expr.as_ref().map(simplify_expr),
            modifiers: modifiers.clone(),
        },
        HirStmtKind::Expression(e) => HirStmtKind::Expression(simplify_expr(e)),
        HirStmtKind::Guarded(cond, stmts) => HirStmtKind::Guarded(
            simplify_expr(cond),
            stmts.iter().map(simplify_stmt).collect(),
        ),
        other => other.clone(),
    };
    HirStmt { id: stmt.id, span: stmt.span, kind }
}

pub fn simplify_body(body: &[HirStmt]) -> Vec<HirStmt> {
    let mut current = body.to_vec();
    for _ in 0..5 {
        let next: Vec<HirStmt> = current.iter().map(simplify_stmt).collect();
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn detect_increments(body: &[HirStmt]) -> Option<IncrementInfo> {
    for stmt in body {
        let HirStmtKind::Assign(lhs, expr) = &stmt.kind else {
            continue;
        };
        let Some(name) = expr_name(lhs) else {
            continue;
        };
        if let Some(info) = self_increment_of(&name, expr) {
            return Some(info);
        }
        if let Some(info) = net_increment_of(&name, expr) {
            return Some(info);
        }
    }
    None
}

/// `x = x + K` / `x = K + x` / `x = x - K` with K > 0.
fn self_increment_of(name: &str, expr: &HirExpr) -> Option<IncrementInfo> {
    let HirExprKind::Binary { op, left: a, right: b } = &expr.kind else {
        return None;
    };
    let (var, delta) = match op {
        BinaryOpKind::Add => {
            if let (HirExprKind::Identifier(var), Some(delta)) = (&a.kind, lit_int(b)) {
                (var, delta)
            } else if let (HirExprKind::Identifier(var), Some(delta)) = (&b.kind, lit_int(a)) {
                (var, delta)
            } else {
                return None;
            }
        }
        BinaryOpKind::Sub => {
            if let (HirExprKind::Identifier(var), Some(delta)) = (&a.kind, lit_int(b)) {
                (var, delta)
            } else {
                return None;
            }
        }
        _ => return None,
    };
    (var == name && delta > 0).then(|| IncrementInfo { var: name.to_string(), delta })
}

/// `x = (x + K1) - K2` (net positive) or `x = (x + K1) - K2` with K2 absent
/// (single-step). Mirrors the AST detector's nested-shape arm.
fn net_increment_of(name: &str, expr: &HirExpr) -> Option<IncrementInfo> {
    let HirExprKind::Binary { op: BinaryOpKind::Sub, left: inner, right: rhs } = &expr.kind else {
        return None;
    };
    let HirExprKind::Binary { op: BinaryOpKind::Add, left: lhs2, right: rhs2 } = &inner.kind else {
        return None;
    };
    if !matches!(&lhs2.kind, HirExprKind::Identifier(v) if *v == name) {
        return None;
    }
    let r1 = lit_int(rhs2);
    let r2 = lit_int(rhs);
    if let (Some(r1_val), Some(r2_val)) = (r1, r2) {
        let net = r1_val - r2_val;
        if net > 0 {
            return Some(IncrementInfo { var: name.to_string(), delta: net });
        }
    }
    if r1.map_or(false, |v| v >= 1) && r2.is_none() {
        return Some(IncrementInfo { var: name.to_string(), delta: 1 });
    }
    None
}

fn detect_popcount_decay(body: &[HirStmt]) -> Option<IncrementInfo> {
    for stmt in body {
        let HirStmtKind::Assign(lhs, expr) = &stmt.kind else {
            continue;
        };
        let HirExprKind::Identifier(name) = &lhs.kind else {
            continue;
        };
        if let Some(info) = popcount_of(name, expr) {
            return Some(info);
        }
    }
    None
}

/// `x = x & (x - 1)` in either operand order.
fn popcount_of(name: &str, expr: &HirExpr) -> Option<IncrementInfo> {
    let HirExprKind::Binary { op: BinaryOpKind::BitAnd, left: a, right: b } = &expr.kind else {
        return None;
    };
    let self_minus_one = |e: &HirExpr| {
        matches!(
            &e.kind,
            HirExprKind::Binary {
                op: BinaryOpKind::Sub,
                left,
                right
            } if matches!(&left.kind, HirExprKind::Identifier(v) if *v == name)
                && matches!(right.kind, HirExprKind::Literal(HirLiteral::Int(1)))
        )
    };
    let a_is_self = matches!(&a.kind, HirExprKind::Identifier(v) if *v == name);
    let b_is_self = matches!(&b.kind, HirExprKind::Identifier(v) if *v == name);
    if (a_is_self && self_minus_one(b)) || (b_is_self && self_minus_one(a)) {
        return Some(IncrementInfo { var: name.to_string(), delta: 1 });
    }
    None
}

fn detect_all_increments(body: &[HirStmt]) -> Vec<IncrementInfo> {
    let mut results = Vec::new();
    for stmt in body {
        let HirStmtKind::Assign(lhs, expr) = &stmt.kind else {
            continue;
        };
        let Some(name) = expr_name(lhs) else {
            continue;
        };
        if let Some(info) = self_increment_of(&name, expr) {
            results.push(info);
        }
    }
    results
}

fn detect_lexicographic_ranking(pre: &HirExpr, body: &[HirStmt]) -> Vec<String> {
    let mut vars = Vec::new();
    collect_or_vars(pre, &mut vars);
    if vars.len() < 2 {
        return Vec::new();
    }
    let decremented: HashSet<String> = body.iter().filter_map(decrement_name).collect();
    vars.retain(|v| decremented.contains(v));
    if vars.len() < 2 {
        Vec::new()
    } else {
        vars
    }
}

/// Collect `var > N` / `var >= N` threshold variables across an Or chain.
fn collect_or_vars(expr: &HirExpr, out: &mut Vec<String>) {
    if let HirExprKind::Binary { op: BinaryOpKind::Or, left, right } = &expr.kind {
        collect_or_vars(left, out);
        collect_or_vars(right, out);
        return;
    }
    collect_threshold_var(expr, BinaryOpKind::Gt, 0, out);
    collect_threshold_var(expr, BinaryOpKind::Ge, 1, out);
}

/// Push the variable of `var <op> N` when `N` meets the threshold minimum.
fn collect_threshold_var(
    expr: &HirExpr,
    want: BinaryOpKind,
    min: i64,
    out: &mut Vec<String>,
) {
    let HirExprKind::Binary { op, left, right } = &expr.kind else {
        return;
    };
    if *op != want {
        return;
    }
    if let (HirExprKind::Identifier(var), HirExprKind::Literal(HirLiteral::Int(n))) =
        (&left.kind, &right.kind)
    {
        if *n >= min && !out.contains(var) {
            out.push(var.clone());
        }
    }
}

/// `x = x - K` with K >= 1 — a lexicographic-ranking decrement.
fn decrement_name(stmt: &HirStmt) -> Option<String> {
    let HirStmtKind::Assign(lhs, expr) = &stmt.kind else {
        return None;
    };
    let name = expr_name(lhs)?;
    let is_decrement = matches!(
        &expr.kind,
        HirExprKind::Binary { op: BinaryOpKind::Sub, left, right }
            if matches!(&left.kind, HirExprKind::Identifier(v) if *v == name)
                && matches!(right.kind, HirExprKind::Literal(HirLiteral::Int(v)) if v >= 1)
    );
    is_decrement.then_some(name)
}

fn statement_contains_ffi_with_decls(stmt: &HirStmt) -> bool {
    match &stmt.kind {
        HirStmtKind::Assign(_, expr) => references_triggers_or_ffi_with_decls(expr),
        HirStmtKind::Let { expr, .. } => {
            expr.as_ref().map_or(false, references_triggers_or_ffi_with_decls)
        }
        HirStmtKind::Expression(e) => references_triggers_or_ffi_with_decls(e),
        HirStmtKind::Term(Some(e)) | HirStmtKind::Tail(e) => {
            references_triggers_or_ffi_with_decls(e)
        }
        HirStmtKind::EndProgram(Some(e)) => references_triggers_or_ffi_with_decls(e),
        HirStmtKind::Guarded(condition, statements) => {
            references_triggers_or_ffi_with_decls(condition)
                || statements.iter().any(statement_contains_ffi_with_decls)
        }
        _ => false,
    }
}

fn statement_contains_ffi(stmt: &HirStmt) -> bool {
    statement_contains_ffi_with_decls(stmt)
}

fn references_triggers_or_ffi_with_decls(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Call { .. } => true,
        HirExprKind::Identifier(_) | HirExprKind::Literal(_) | HirExprKind::UnitLiteral { .. } => {
            false
        }
        HirExprKind::Binary { left, right, .. } => {
            references_triggers_or_ffi_with_decls(left)
                || references_triggers_or_ffi_with_decls(right)
        }
        HirExprKind::Unary { inner, .. } => references_triggers_or_ffi_with_decls(inner),
        HirExprKind::Cast { inner, .. } => references_triggers_or_ffi_with_decls(inner),
        HirExprKind::Block(stmts) => stmts.iter().any(statement_contains_ffi_with_decls),
        HirExprKind::List(items) => items.iter().any(references_triggers_or_ffi_with_decls),
        HirExprKind::Index(a, b) => {
            references_triggers_or_ffi_with_decls(a) || references_triggers_or_ffi_with_decls(b)
        }
        HirExprKind::Field { receiver, .. } => references_triggers_or_ffi_with_decls(receiver),
        HirExprKind::Tuple(items) => items.iter().any(references_triggers_or_ffi_with_decls),
        _ => false,
    }
}

fn is_pure_body(
    body: &[HirStmt],
    state_fields: &HashSet<String>,
    increments: &Option<IncrementInfo>,
) -> bool {
    let inc_var = increments.as_ref().map(|i| &i.var);
    for stmt in body {
        match &stmt.kind {
            // AST's counter arm and the FFI-check arm are mutually exclusive:
            // when an increment exists, a non-counter assignment is NOT
            // FFI-scanned. Mirror that exactly.
            HirStmtKind::Assign(lhs, expr) if inc_var.is_some() => {
                if !counter_assign_is_pure(lhs, expr, inc_var.unwrap(), state_fields) {
                    return false;
                }
            }
            HirStmtKind::Let { expr: Some(e), .. }
            | HirStmtKind::Assign(_, e)
            | HirStmtKind::Expression(e) => {
                if references_triggers_or_ffi_with_decls(e) {
                    return false;
                }
            }
            HirStmtKind::Term(_) | HirStmtKind::EndProgram(_) | HirStmtKind::Tail(_) => {}
            HirStmtKind::Rollback(_) => return false,
            HirStmtKind::Guarded(condition, statements) => {
                if references_triggers_or_ffi_with_decls(condition) {
                    return false;
                }
                if statements.iter().any(statement_contains_ffi_with_decls) {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// Counter-assignment purity: the counter itself may only add or mask; any
/// other assignment must not target a state field.
fn counter_assign_is_pure(
    lhs: &HirExpr,
    expr: &HirExpr,
    inc_var: &str,
    state_fields: &HashSet<String>,
) -> bool {
    let Some(name) = expr_name(lhs) else {
        return false;
    };
    if name == inc_var {
        return matches!(
            &expr.kind,
            HirExprKind::Binary {
                op: BinaryOpKind::Add | BinaryOpKind::BitAnd,
                ..
            }
        );
    }
    !state_fields.contains(&name)
}

fn compute_live_fields(
    exit_condition: Option<&HirExpr>,
    out_pragmas: &[String],
    nodes: &[HirReactorNode],
) -> HashSet<String> {
    let mut live = seed_live_fields(exit_condition, out_pragmas, nodes);
    while propagate_live_once(nodes, &mut live) {}
    live
}

/// Seeds: exit condition, out pragmas, preconditions, FFI args.
fn seed_live_fields(
    exit_condition: Option<&HirExpr>,
    out_pragmas: &[String],
    nodes: &[HirReactorNode],
) -> HashSet<String> {
    let mut live = HashSet::new();
    if let Some(ec) = exit_condition {
        collect_identifiers(ec, &mut live);
    }
    for name in out_pragmas {
        live.insert(name.clone());
    }
    for node in nodes {
        collect_identifiers(&node.precondition, &mut live);
    }
    for node in nodes {
        scan_node_ffi_args(node, &mut live);
    }
    live
}

/// FFI-argument identifiers of one node body.
fn scan_node_ffi_args(node: &HirReactorNode, live: &mut HashSet<String>) {
    for stmt in &node.body {
        scan_for_ffi_args(stmt, live);
    }
}

/// One liveness fixpoint round over assign/let reaches. Returns whether the
/// set grew.
fn propagate_live_once(nodes: &[HirReactorNode], live: &mut HashSet<String>) -> bool {
    let mut changed = false;
    for node in nodes {
        if propagate_node(node, live) {
            changed = true;
        }
    }
    changed
}

/// Reaches-back propagation across one node's reachable statements.
fn propagate_node(node: &HirReactorNode, live: &mut HashSet<String>) -> bool {
    let mut changed = false;
    for stmt in reachable_stmts(node) {
        if propagate_stmt(stmt, live) {
            changed = true;
        }
    }
    changed
}

/// One reaches-back propagation step for a statement's assigned expression.
fn propagate_stmt(stmt: &HirStmt, live: &mut HashSet<String>) -> bool {
    let (target, expr) = match &stmt.kind {
        HirStmtKind::Assign(lhs, expr) => (expr_name(lhs), expr),
        HirStmtKind::Let { name, expr: Some(e), .. } => (Some(name.clone()), e),
        _ => return false,
    };
    let Some(t) = target else {
        return false;
    };
    if !live.contains(&t) {
        return false;
    }
    let mut changed = false;
    let mut idents = HashSet::new();
    collect_identifiers(expr, &mut idents);
    for ident in idents {
        if live.insert(ident) {
            changed = true;
        }
    }
    changed
}

/// The node body plus every statement nested under a guard (transitively).
fn reachable_stmts(node: &HirReactorNode) -> Vec<&HirStmt> {
    let mut stmts: Vec<&HirStmt> = node.body.iter().collect();
    let mut i = 0;
    while i < stmts.len() {
        if let HirStmtKind::Guarded(_, statements) = &stmts[i].kind {
            stmts.extend(statements);
        }
        i += 1;
    }
    stmts
}

fn scan_for_ffi_args(stmt: &HirStmt, out: &mut HashSet<String>) {
    match &stmt.kind {
        HirStmtKind::Expression(expr) | HirStmtKind::Let { expr: Some(expr), .. } => {
            collect_ffi_identifiers(expr, out);
        }
        HirStmtKind::Guarded(_, statements) => {
            for s in statements {
                scan_for_ffi_args(s, out);
            }
        }
        _ => {}
    }
}

fn collect_ffi_identifiers(expr: &HirExpr, out: &mut HashSet<String>) {
    match &expr.kind {
        HirExprKind::Call { args, .. } => {
            for arg in args {
                collect_identifiers(arg, out);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_ffi_identifiers(left, out);
            collect_ffi_identifiers(right, out);
        }
        _ => collect_identifiers(expr, out),
    }
}

fn compute_effectively_pure(node: &mut HirReactorNode, live_fields: &HashSet<String>) {
    if node.body.iter().any(statement_contains_ffi_with_decls) {
        return;
    }
    if let (Some(bp), Some(inc)) = (&node.bounded_pre, &node.increments) {
        if inc.var == bp.var && inc.delta > 0 && live_fields.contains(&inc.var) {
            let non_counter_writes: Vec<&String> =
                node.write_set.iter().filter(|f| *f != &inc.var).collect();
            if non_counter_writes.iter().all(|f| !live_fields.contains(*f)) {
                node.is_effectively_pure = true;
            }
        }
    }
}

fn collect_identifiers(expr: &HirExpr, out: &mut HashSet<String>) {
    match &expr.kind {
        HirExprKind::Identifier(name) => {
            out.insert(name.clone());
        }
        HirExprKind::Binary { op: BinaryOpKind::Eq | BinaryOpKind::Ge | BinaryOpKind::Le, left, right }
            if is_self_identity(left, right) => {}
        HirExprKind::Binary { left, right, .. } => {
            collect_identifiers(left, out);
            collect_identifiers(right, out);
        }
        HirExprKind::Unary { inner, .. } => collect_identifiers(inner, out),
        HirExprKind::Cast { inner, .. } => collect_identifiers(inner, out),
        HirExprKind::Call { args, .. } => {
            for arg in args {
                collect_identifiers(arg, out);
            }
        }
        HirExprKind::List(items) => {
            for item in items {
                collect_identifiers(item, out);
            }
        }
        HirExprKind::Index(list, idx) => {
            collect_identifiers(list, out);
            collect_identifiers(idx, out);
        }
        HirExprKind::Field { receiver, .. } => collect_identifiers(receiver, out),
        HirExprKind::Reflect { receiver, .. } => collect_identifiers(receiver, out),
        HirExprKind::Block(stmts) => {
            for stmt in stmts {
                if let HirStmtKind::Expression(e) = &stmt.kind {
                    collect_identifiers(e, out);
                }
            }
        }
        HirExprKind::Tuple(items) => {
            for item in items {
                collect_identifiers(item, out);
            }
        }
        HirExprKind::Match { arms, .. } => {
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_identifiers(guard, out);
                }
                collect_identifiers(&arm.body, out);
            }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            collect_identifiers(condition, out);
            collect_identifiers(then_branch, out);
            if let Some(e) = else_branch {
                collect_identifiers(e, out);
            }
        }
        HirExprKind::Lambda { body, .. } => collect_identifiers(body, out),
        _ => {}
    }
}

fn extract_write_set(body: &[HirStmt], state_fields: &HashSet<String>) -> HashSet<String> {
    let mut writes = HashSet::new();
    for stmt in body {
        match &stmt.kind {
            HirStmtKind::Assign(lhs, _) => {
                if let Some(name) = extract_root_via_provenance(lhs) {
                    if state_fields.contains(&name) {
                        writes.insert(name);
                    }
                }
            }
            HirStmtKind::Guarded(_, body) | HirStmtKind::Block(body) => {
                writes.extend(extract_write_set(body, state_fields));
            }
            HirStmtKind::Foreach { body, .. } => {
                writes.extend(extract_write_set(body, state_fields));
            }
            _ => {}
        }
    }
    writes
}

fn extract_root_via_provenance(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Identifier(n) => Some(n.clone()),
        HirExprKind::Field { receiver, .. } => extract_root_via_provenance(receiver),
        _ => None,
    }
}
