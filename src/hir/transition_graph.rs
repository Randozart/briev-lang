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
        let mut nodes = Vec::new();

        for txn in program
            .callables
            .iter()
            .filter(|c| matches!(c.kind, HirCallableKind::Transaction) && c.owner.is_none())
        {
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
            let (bounded_pre, increments) = {
                let bp = extract_valid_bounded_pre(&precondition, &increments);
                if let Some(bp) = bp {
                    (Some(bp), increments)
                } else {
                    let all_incs = detect_all_increments(&simplified_body);
                    let matched = all_incs.iter().find_map(|inc| {
                        extract_valid_bounded_pre(&precondition, &Some(inc.clone()))
                            .map(|b| (b, inc.clone()))
                    });
                    match matched {
                        Some((bp, inc)) => (Some(bp), Some(inc)),
                        None => (None, increments),
                    }
                }
            };
            let is_pure = is_pure_body(&simplified_body, &state_fields, &increments);
            let write_set = extract_write_set(&simplified_body, &state_fields);
            let lexicographic_vars = detect_lexicographic_ranking(&precondition, &simplified_body);

            nodes.push(HirReactorNode {
                name: txn.name.clone(),
                is_reactive: txn.is_reactive,
                precondition,
                body: simplified_body,
                bounded_pre,
                increments,
                is_pure_body: is_pure,
                write_set,
                is_effectively_pure: false,
                lexicographic_vars,
            });
        }

        let mut live_fields = compute_live_fields(exit_condition, out_pragmas, &nodes);
        for g in &program.globals {
            if g.modifiers.iter().any(|m| m == "out" || m == "vol") {
                live_fields.insert(g.name.clone());
            }
        }
        for node in &mut nodes {
            compute_effectively_pure(node, &live_fields);
        }

        let mut has_unguarded_ffi = HashSet::new();
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
                has_unguarded_ffi.insert(txn.name.clone());
            }
        }

        Self {
            nodes,
            has_triggers: program.has_triggers,
            live_fields,
            has_unguarded_ffi,
        }
    }
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
            match (&left.kind, &right.kind) {
                (HirExprKind::Identifier(var), HirExprKind::Identifier(bound)) => Some(BoundedPre {
                    var: var.clone(),
                    bound_var: bound.clone(),
                    direction: ConvergeDirection::Increasing,
                    bound_literal: None,
                }),
                (HirExprKind::Identifier(var), HirExprKind::Literal(HirLiteral::Int(n))) => {
                    Some(BoundedPre {
                        var: var.clone(),
                        bound_var: format!("__lit__{}", var),
                        direction: ConvergeDirection::Increasing,
                        bound_literal: Some(*n),
                    })
                }
                (HirExprKind::Identifier(var), HirExprKind::Unary { op: UnaryOpKind::Neg, inner })
                    if matches!(inner.kind, HirExprKind::Literal(HirLiteral::Int(_))) =>
                {
                    let n = lit_int(inner).map(|n| -n).unwrap_or(0);
                    Some(BoundedPre {
                        var: var.clone(),
                        bound_var: format!("__lit__{}", var),
                        direction: ConvergeDirection::Increasing,
                        bound_literal: Some(n),
                    })
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
        HirExprKind::Binary { op: BinaryOpKind::Gt | BinaryOpKind::Ge, left, right } => {
            match (&left.kind, &right.kind) {
                (HirExprKind::Identifier(var), HirExprKind::Identifier(bound)) => Some(BoundedPre {
                    var: var.clone(),
                    bound_var: bound.clone(),
                    direction: ConvergeDirection::Decreasing,
                    bound_literal: None,
                }),
                (HirExprKind::Identifier(var), HirExprKind::Literal(HirLiteral::Int(n))) => {
                    Some(BoundedPre {
                        var: var.clone(),
                        bound_var: format!("__lit__{}", var),
                        direction: ConvergeDirection::Decreasing,
                        bound_literal: Some(*n),
                    })
                }
                (HirExprKind::Identifier(var), HirExprKind::Unary { op: UnaryOpKind::Neg, inner })
                    if matches!(inner.kind, HirExprKind::Literal(HirLiteral::Int(_))) =>
                {
                    let n = lit_int(inner).map(|n| -n).unwrap_or(0);
                    Some(BoundedPre {
                        var: var.clone(),
                        bound_var: format!("__lit__{}", var),
                        direction: ConvergeDirection::Decreasing,
                        bound_literal: Some(n),
                    })
                }
                (HirExprKind::Field { receiver, name }, HirExprKind::Literal(HirLiteral::Int(0)))
                    if name == "Size" =>
                {
                    expr_name(receiver).map(|name| BoundedPre {
                        var: name.clone(),
                        bound_var: format!("__lit__{}", name),
                        direction: ConvergeDirection::Decreasing,
                        bound_literal: Some(0),
                    })
                }
                (HirExprKind::Field { receiver, name }, HirExprKind::Literal(HirLiteral::Int(n)))
                    if name == "Size" && *n > 0 =>
                {
                    expr_name(receiver).map(|name| BoundedPre {
                        var: name.clone(),
                        bound_var: format!("__lit__{}", name),
                        direction: ConvergeDirection::Decreasing,
                        bound_literal: Some(*n),
                    })
                }
                _ => None,
            }
        }
        HirExprKind::Binary { op: BinaryOpKind::Neq, left, right } => {
            match (&left.kind, &right.kind) {
                (HirExprKind::Identifier(var), HirExprKind::Literal(HirLiteral::Int(n))) => {
                    Some(BoundedPre {
                        var: var.clone(),
                        bound_var: format!("__lit__{}", var),
                        direction: ConvergeDirection::Decreasing,
                        bound_literal: Some(*n),
                    })
                }
                _ => None,
            }
        }
        HirExprKind::Binary { op: BinaryOpKind::And, left, right } => {
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
        _ => None,
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
    let simplified = match &expr.kind {
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
        HirExprKind::List(items) => rebuild(
            expr,
            HirExprKind::List(items.iter().map(simplify_expr).collect()),
        ),
        HirExprKind::Tuple(items) => rebuild(
            expr,
            HirExprKind::Tuple(items.iter().map(simplify_expr).collect()),
        ),
        HirExprKind::Block(stmts) => rebuild(
            expr,
            HirExprKind::Block(stmts.iter().map(simplify_stmt).collect()),
        ),
        _ => expr.clone(),
    };

    match &simplified.kind {
        HirExprKind::Binary { op: BinaryOpKind::Sub, left: sub_lhs, right: sub_rhs } => {
            if let HirExprKind::Binary { op: BinaryOpKind::Add, left: add_lhs, right: add_rhs } =
                &sub_lhs.kind
            {
                if vars_match(add_lhs, sub_rhs) {
                    return (**add_rhs).clone();
                }
                if vars_match(add_rhs, sub_rhs) {
                    return (**add_lhs).clone();
                }
            }
            if let HirExprKind::Binary { op: BinaryOpKind::Sub, left: inner_a, right: inner_b } =
                &sub_rhs.kind
            {
                if vars_match(sub_lhs, inner_a) {
                    return (**inner_b).clone();
                }
            }
            if let (
                HirExprKind::Binary { op: BinaryOpKind::Add, left: a1, right: b1 },
                HirExprKind::Binary { op: BinaryOpKind::Add, left: a2, right: b2 },
            ) = (&sub_lhs.kind, &sub_rhs.kind)
            {
                if vars_match(a1, a2) && !vars_match(b1, b2) {
                    return rebuild(
                        expr,
                        HirExprKind::Binary {
                            op: BinaryOpKind::Sub,
                            left: Box::new((**b1).clone()),
                            right: Box::new((**b2).clone()),
                        },
                    );
                }
            }
            if let (
                HirExprKind::Binary { op: BinaryOpKind::Sub, left: sa, right: sb1 },
                HirExprKind::Binary { op: BinaryOpKind::Sub, left: sc, right: sb2 },
            ) = (&sub_lhs.kind, &sub_rhs.kind)
            {
                if vars_match(sb1, sb2) {
                    return rebuild(
                        expr,
                        HirExprKind::Binary {
                            op: BinaryOpKind::Sub,
                            left: Box::new((**sa).clone()),
                            right: Box::new((**sc).clone()),
                        },
                    );
                }
            }
            if let HirExprKind::Literal(HirLiteral::Int(0)) = sub_rhs.kind {
                return (**sub_lhs).clone();
            }
            simplified
        }
        HirExprKind::Binary { op: BinaryOpKind::Add, left: add_lhs, right: add_rhs } => {
            if let HirExprKind::Binary { op: BinaryOpKind::Sub, left: sub_a, right: sub_b } =
                &add_lhs.kind
            {
                if vars_match(sub_b, add_rhs) {
                    return (**sub_a).clone();
                }
            }
            if let HirExprKind::Literal(HirLiteral::Int(0)) = add_rhs.kind {
                return (**add_lhs).clone();
            }
            if let HirExprKind::Literal(HirLiteral::Int(0)) = add_lhs.kind {
                return (**add_rhs).clone();
            }
            simplified
        }
        HirExprKind::Binary { op: BinaryOpKind::Mul, left: mul_lhs, right: mul_rhs } => {
            if let HirExprKind::Literal(HirLiteral::Int(1)) = mul_rhs.kind {
                return (**mul_lhs).clone();
            }
            if let HirExprKind::Literal(HirLiteral::Int(1)) = mul_lhs.kind {
                return (**mul_rhs).clone();
            }
            if let HirExprKind::Literal(HirLiteral::Int(0)) = mul_rhs.kind {
                return rebuild(expr, HirExprKind::Literal(HirLiteral::Int(0)));
            }
            if let HirExprKind::Literal(HirLiteral::Int(0)) = mul_lhs.kind {
                return rebuild(expr, HirExprKind::Literal(HirLiteral::Int(0)));
            }
            simplified
        }
        HirExprKind::Binary { op: BinaryOpKind::Div, left: div_lhs, right: div_rhs } => {
            if let HirExprKind::Literal(HirLiteral::Int(1)) = div_rhs.kind {
                return (**div_lhs).clone();
            }
            simplified
        }
        _ => simplified,
    }
}

fn simplify_stmt(stmt: &HirStmt) -> HirStmt {
    let kind = match &stmt.kind {
        HirStmtKind::Assign(lhs, rhs) => {
            HirStmtKind::Assign(lhs.clone(), simplify_expr(rhs))
        }
        HirStmtKind::Let { name, ty, expr, modifiers } => HirStmtKind::Let {
            name: name.clone(),
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
        if let HirExprKind::Binary { op: BinaryOpKind::Add, left: a, right: b } = &expr.kind {
            if let (HirExprKind::Identifier(var), Some(delta)) = (&a.kind, lit_int(b)) {
                if *var == name && delta > 0 {
                    return Some(IncrementInfo { var: name, delta });
                }
            }
            if let (HirExprKind::Identifier(var), Some(delta)) = (&b.kind, lit_int(a)) {
                if *var == name && delta > 0 {
                    return Some(IncrementInfo { var: name, delta });
                }
            }
        }
        if let HirExprKind::Binary { op: BinaryOpKind::Sub, left: a, right: b } = &expr.kind {
            if let (HirExprKind::Identifier(var), Some(delta)) = (&a.kind, lit_int(b)) {
                if *var == name && delta > 0 {
                    return Some(IncrementInfo { var: name, delta });
                }
            }
        }
        if let HirExprKind::Binary { op: BinaryOpKind::Sub, left: inner, right: rhs } = &expr.kind {
            if let HirExprKind::Binary { op: BinaryOpKind::Add, left: lhs2, right: rhs2 } = &inner.kind
            {
                let is_self_add =
                    matches!(&lhs2.kind, HirExprKind::Identifier(v) if *v == name);
                if is_self_add {
                    let r1 = lit_int(rhs2);
                    let r2 = lit_int(rhs);
                    if let (Some(r1_val), Some(r2_val)) = (r1, r2) {
                        let net = r1_val - r2_val;
                        if net > 0 {
                            return Some(IncrementInfo { var: name, delta: net });
                        }
                    }
                    if r1.map_or(false, |v| v >= 1) && r2.is_none() {
                        return Some(IncrementInfo { var: name, delta: 1 });
                    }
                }
            }
        }
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
        let name = name.clone();
        if let HirExprKind::Binary { op: BinaryOpKind::BitAnd, left: a, right: b } = &expr.kind {
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
            if a_is_self && self_minus_one(b) {
                return Some(IncrementInfo { var: name, delta: 1 });
            }
            let b_is_self = matches!(&b.kind, HirExprKind::Identifier(v) if *v == name);
            if b_is_self && self_minus_one(a) {
                return Some(IncrementInfo { var: name, delta: 1 });
            }
        }
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
        if let HirExprKind::Binary { op: BinaryOpKind::Add, left: a, right: b } = &expr.kind {
            if let (HirExprKind::Identifier(var), Some(delta)) = (&a.kind, lit_int(b)) {
                if *var == name && delta > 0 {
                    results.push(IncrementInfo { var: name, delta });
                    continue;
                }
            }
            if let (HirExprKind::Identifier(var), Some(delta)) = (&b.kind, lit_int(a)) {
                if *var == name && delta > 0 {
                    results.push(IncrementInfo { var: name, delta });
                    continue;
                }
            }
        }
        if let HirExprKind::Binary { op: BinaryOpKind::Sub, left: a, right: b } = &expr.kind {
            if let (HirExprKind::Identifier(var), Some(delta)) = (&a.kind, lit_int(b)) {
                if *var == name && delta > 0 {
                    results.push(IncrementInfo { var: name, delta });
                }
            }
        }
    }
    results
}

fn detect_lexicographic_ranking(pre: &HirExpr, body: &[HirStmt]) -> Vec<String> {
    fn collect_or_vars(expr: &HirExpr, out: &mut Vec<String>) {
        if let HirExprKind::Binary { op: BinaryOpKind::Or, left, right } = &expr.kind {
            collect_or_vars(left, out);
            collect_or_vars(right, out);
            return;
        }
        if let HirExprKind::Binary { op: BinaryOpKind::Gt, left: inner, right: val } = &expr.kind {
            if let (HirExprKind::Identifier(var), HirExprKind::Literal(HirLiteral::Int(n))) =
                (&inner.kind, &val.kind)
            {
                if *n >= 0 && !out.contains(var) {
                    out.push(var.clone());
                }
            }
        }
        if let HirExprKind::Binary { op: BinaryOpKind::Ge, left: inner, right: val } = &expr.kind {
            if let (HirExprKind::Identifier(var), HirExprKind::Literal(HirLiteral::Int(n))) =
                (&inner.kind, &val.kind)
            {
                if *n > 0 && !out.contains(var) {
                    out.push(var.clone());
                }
            }
        }
    }

    let mut vars = Vec::new();
    collect_or_vars(pre, &mut vars);
    if vars.len() < 2 {
        return Vec::new();
    }
    let decremented: HashSet<String> = body
        .iter()
        .filter_map(|stmt| {
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
        })
        .collect();
    vars.retain(|v| decremented.contains(v));
    if vars.len() < 2 {
        Vec::new()
    } else {
        vars
    }
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
                let Some(name) = expr_name(lhs) else {
                    return false;
                };
                if Some(&name) == inc_var {
                    if !matches!(
                        &expr.kind,
                        HirExprKind::Binary {
                            op: BinaryOpKind::Add | BinaryOpKind::BitAnd,
                            ..
                        }
                    ) {
                        return false;
                    }
                    continue;
                }
                if state_fields.contains(&name) {
                    return false;
                }
            }
            HirStmtKind::Let { expr, .. } => {
                if let Some(e) = expr {
                    if references_triggers_or_ffi_with_decls(e) {
                        return false;
                    }
                }
            }
            HirStmtKind::Assign(_, expr) => {
                if references_triggers_or_ffi_with_decls(expr) {
                    return false;
                }
            }
            HirStmtKind::Expression(e) => {
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

fn compute_live_fields(
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
        for stmt in &node.body {
            scan_for_ffi_args(stmt, &mut live);
        }
    }
    loop {
        let mut changed = false;
        for node in nodes {
            let mut stmts: Vec<&HirStmt> = node.body.iter().collect();
            let mut i = 0;
            while i < stmts.len() {
                if let HirStmtKind::Guarded(_, statements) = &stmts[i].kind {
                    stmts.extend(statements);
                }
                i += 1;
            }
            for stmt in stmts {
                let (target, expr) = match &stmt.kind {
                    HirStmtKind::Assign(lhs, expr) => (expr_name(lhs), expr),
                    HirStmtKind::Let { name, expr: Some(e), .. } => (Some(name.clone()), e),
                    _ => continue,
                };
                if let Some(ref t) = target {
                    if live.contains(t) {
                        let mut idents = HashSet::new();
                        collect_identifiers(expr, &mut idents);
                        for ident in idents {
                            if live.insert(ident) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    live
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
