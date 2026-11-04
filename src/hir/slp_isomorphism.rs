// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.4b — SLP isomorphism / vector-phi candidates over HIR.
//! Mirror of `analysis::slp_isomorphism`, reusing `VectorPhiCandidate`.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::slp_isomorphism::VectorPhiCandidate;
use crate::ast::{BinaryOpKind, Type, UnaryOpKind};
use std::collections::HashMap;

pub fn exprs_isomorphic(a: &HirExpr, b: &HirExpr, mapping: &HashMap<String, String>) -> bool {
    match (&a.kind, &b.kind) {
        (
            HirExprKind::Binary { op: k1, left: l1, right: r1 },
            HirExprKind::Binary { op: k2, left: l2, right: r2 },
        ) => {
            k1 == k2 && exprs_isomorphic(l1, l2, mapping) && exprs_isomorphic(r1, r2, mapping)
        }
        (
            HirExprKind::Unary { op: k1, inner: e1 },
            HirExprKind::Unary { op: k2, inner: e2 },
        ) => k1 == k2 && exprs_isomorphic(e1, e2, mapping),
        (HirExprKind::Literal(HirLiteral::Int(n1)), HirExprKind::Literal(HirLiteral::Int(n2))) => {
            n1 == n2
        }
        (
            HirExprKind::Literal(HirLiteral::Float(n1)),
            HirExprKind::Literal(HirLiteral::Float(n2)),
        ) => (n1 - n2).abs() < 1e-10,
        (
            HirExprKind::Literal(HirLiteral::Bool(b1)),
            HirExprKind::Literal(HirLiteral::Bool(b2)),
        ) => b1 == b2,
        (HirExprKind::Identifier(n1), HirExprKind::Identifier(n2)) => match mapping.get(n1) {
            Some(mapped) => mapped == n2,
            None => n1 == n2,
        },
        (
            HirExprKind::Cast { inner: e1, to: t1, .. },
            HirExprKind::Cast { inner: e2, to: t2, .. },
        ) => t1 == t2 && exprs_isomorphic(e1, e2, mapping),
        (
            HirExprKind::Field { receiver: obj1, name: f1 },
            HirExprKind::Field { receiver: obj2, name: f2 },
        ) => f1 == f2 && exprs_isomorphic(obj1, obj2, mapping),
        _ => false,
    }
}

pub fn build_mapping(a: &HirExpr, b: &HirExpr) -> Option<HashMap<String, String>> {
    let mut mapping = HashMap::new();
    try_build_mapping(a, b, &mut mapping).then_some(mapping)
}

fn try_build_mapping(a: &HirExpr, b: &HirExpr, mapping: &mut HashMap<String, String>) -> bool {
    match (&a.kind, &b.kind) {
        (
            HirExprKind::Binary { op: k1, left: l1, right: r1 },
            HirExprKind::Binary { op: k2, left: l2, right: r2 },
        ) => {
            k1 == k2 && try_build_mapping(l1, l2, mapping) && try_build_mapping(r1, r2, mapping)
        }
        (
            HirExprKind::Unary { op: k1, inner: e1 },
            HirExprKind::Unary { op: k2, inner: e2 },
        ) => k1 == k2 && try_build_mapping(e1, e2, mapping),
        (HirExprKind::Literal(HirLiteral::Int(n1)), HirExprKind::Literal(HirLiteral::Int(n2))) => {
            n1 == n2
        }
        (
            HirExprKind::Literal(HirLiteral::Float(n1)),
            HirExprKind::Literal(HirLiteral::Float(n2)),
        ) => (n1 - n2).abs() < 1e-10,
        (
            HirExprKind::Literal(HirLiteral::Bool(b1)),
            HirExprKind::Literal(HirLiteral::Bool(b2)),
        ) => b1 == b2,
        (HirExprKind::Identifier(n1), HirExprKind::Identifier(n2)) => {
            if n1 == n2 {
                return true;
            }
            match mapping.get(n1) {
                Some(mapped) => mapped == n2,
                None => {
                    if mapping.values().any(|v| v == n2) {
                        return false;
                    }
                    mapping.insert(n1.clone(), n2.clone());
                    true
                }
            }
        }
        (
            HirExprKind::Cast { inner: e1, to: t1, .. },
            HirExprKind::Cast { inner: e2, to: t2, .. },
        ) => t1 == t2 && try_build_mapping(e1, e2, mapping),
        (
            HirExprKind::Field { receiver: obj1, name: f1 },
            HirExprKind::Field { receiver: obj2, name: f2 },
        ) => f1 == f2 && try_build_mapping(obj1, obj2, mapping),
        _ => false,
    }
}

pub fn statements_isomorphic(a: &HirStmt, b: &HirStmt) -> Option<HashMap<String, String>> {
    match (&a.kind, &b.kind) {
        (
            HirStmtKind::Let { ty: t1, expr: e1, .. },
            HirStmtKind::Let { ty: t2, expr: e2, .. },
        ) => {
            if t1 != t2 {
                return None;
            }
            build_mapping(e1.as_ref()?, e2.as_ref()?)
        }
        (HirStmtKind::Assign(l1, e1), HirStmtKind::Assign(l2, e2)) => {
            let mut mapping = HashMap::new();
            if !try_build_mapping_lhs(l1, l2, &mut mapping) {
                return None;
            }
            if let Some(rhs_map) = build_mapping(e1, e2) {
                mapping.extend(rhs_map);
            }
            exprs_isomorphic(e1, e2, &mapping).then_some(mapping)
        }
        _ => None,
    }
}

fn try_build_mapping_lhs(a: &HirExpr, b: &HirExpr, mapping: &mut HashMap<String, String>) -> bool {
    match (&a.kind, &b.kind) {
        (HirExprKind::Identifier(n1), HirExprKind::Identifier(n2)) => {
            if n1 == n2 {
                return true;
            }
            if mapping.contains_key(n1) {
                mapping[n1] == *n2
            } else {
                mapping.insert(n1.clone(), n2.clone());
                true
            }
        }
        (
            HirExprKind::Field { receiver: obj1, name: f1 },
            HirExprKind::Field { receiver: obj2, name: f2 },
        ) => f1 == f2 && try_build_mapping_lhs(obj1, obj2, mapping),
        _ => structural_eq(a, b),
    }
}

/// Structural equality ignoring HIR ids/spans (AST `Expr` equality analog).
fn structural_eq(a: &HirExpr, b: &HirExpr) -> bool {
    match (&a.kind, &b.kind) {
        (HirExprKind::Identifier(x), HirExprKind::Identifier(y)) => x == y,
        (HirExprKind::Literal(x), HirExprKind::Literal(y)) => x == y,
        (HirExprKind::UnitLiteral { value: v1, unit: u1 }, HirExprKind::UnitLiteral { value: v2, unit: u2 }) => {
            v1 == v2 && u1 == u2
        }
        (
            HirExprKind::Binary { op: k1, left: l1, right: r1 },
            HirExprKind::Binary { op: k2, left: l2, right: r2 },
        ) => k1 == k2 && structural_eq(l1, l2) && structural_eq(r1, r2),
        (
            HirExprKind::Unary { op: k1, inner: e1 },
            HirExprKind::Unary { op: k2, inner: e2 },
        ) => k1 == k2 && structural_eq(e1, e2),
        (
            HirExprKind::Field { receiver: o1, name: n1 },
            HirExprKind::Field { receiver: o2, name: n2 },
        ) => n1 == n2 && structural_eq(o1, o2),
        (
            HirExprKind::Index(a1, b1),
            HirExprKind::Index(a2, b2),
        ) => structural_eq(a1, a2) && structural_eq(b1, b2),
        (
            HirExprKind::Cast { inner: e1, to: t1, .. },
            HirExprKind::Cast { inner: e2, to: t2, .. },
        ) => t1 == t2 && structural_eq(e1, e2),
        (HirExprKind::Tuple(x), HirExprKind::Tuple(y))
        | (HirExprKind::List(x), HirExprKind::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| structural_eq(a, b))
        }
        _ => false,
    }
}

fn find_isomorphic_groups(body: &[HirStmt], start_idx: usize) -> Vec<VectorPhiCandidate> {
    let mut groups = Vec::new();
    if start_idx >= body.len() {
        return groups;
    }
    let template = &body[start_idx];
    let mut lhs_names: Vec<String> = Vec::new();
    let template_lhs = match &template.kind {
        HirStmtKind::Let { name, .. } => Some(name.clone()),
        HirStmtKind::Assign(HirExpr { kind: HirExprKind::Identifier(n), .. }, _) => Some(n.clone()),
        _ => None,
    };
    let Some(template_lhs) = template_lhs else {
        return groups;
    };
    lhs_names.push(template_lhs);

    for stmt in body.iter().skip(start_idx + 1) {
        if statements_isomorphic(template, stmt).is_some() {
            let lhs = match &stmt.kind {
                HirStmtKind::Let { name, .. } => name.clone(),
                HirStmtKind::Assign(HirExpr { kind: HirExprKind::Identifier(n), .. }, _) => {
                    n.clone()
                }
                _ => break,
            };
            lhs_names.push(lhs);
        } else {
            break;
        }
    }

    if lhs_names.len() >= 2 {
        let width = lhs_names.len();
        let element_type = match &template.kind {
            HirStmtKind::Let { ty: Some(t), .. } => t.clone(),
            _ => Type::float(),
        };
        let group_name = infer_group_name(&lhs_names);
        groups.push(VectorPhiCandidate {
            group_name,
            element_ty: element_type,
            width,
            fields: lhs_names,
        });
    }
    groups
}

fn infer_group_name(fields: &[String]) -> String {
    if fields.is_empty() {
        return "g".to_string();
    }
    let first = &fields[0];
    for len in (1..=first.len()).rev() {
        let prefix = &first[..len];
        if fields.iter().all(|f| f.starts_with(prefix)) {
            let rest_valid = fields.iter().all(|f| {
                let rest = &f[len..];
                rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit())
            });
            if rest_valid {
                return prefix.to_string();
            }
        }
    }
    "g".to_string()
}

fn expr_signature(expr: &HirExpr) -> Vec<u8> {
    let mut sig = Vec::new();
    expr_signature_inner(expr, &mut sig);
    sig
}

fn expr_signature_inner(expr: &HirExpr, sig: &mut Vec<u8>) {
    match &expr.kind {
        HirExprKind::Binary { op, left, right } => {
            sig.push(1);
            sig.push(match op {
                BinaryOpKind::Add => 1,
                BinaryOpKind::Sub => 2,
                BinaryOpKind::Mul => 3,
                BinaryOpKind::Div => 4,
                BinaryOpKind::Eq => 5,
                BinaryOpKind::Neq => 6,
                BinaryOpKind::Lt => 7,
                BinaryOpKind::Gt => 8,
                BinaryOpKind::Le => 9,
                BinaryOpKind::Ge => 10,
                _ => 0,
            });
            expr_signature_inner(left, sig);
            expr_signature_inner(right, sig);
        }
        HirExprKind::Unary { op, inner } => {
            sig.push(2);
            sig.push(match op {
                UnaryOpKind::Neg => 1,
                UnaryOpKind::Not => 2,
                _ => 0,
            });
            expr_signature_inner(inner, sig);
        }
        HirExprKind::Identifier(_) => sig.push(3),
        HirExprKind::Literal(HirLiteral::Int(n)) => {
            sig.push(4);
            sig.extend_from_slice(&n.to_le_bytes());
        }
        HirExprKind::Literal(HirLiteral::Float(f)) => {
            sig.push(5);
            sig.extend_from_slice(&f.to_bits().to_le_bytes());
        }
        HirExprKind::Literal(HirLiteral::Bool(b)) => {
            sig.push(6);
            sig.push(u8::from(*b));
        }
        _ => sig.push(0),
    }
}

fn group_template_signature(body: &[HirStmt], group: &VectorPhiCandidate) -> Option<Vec<u8>> {
    let field_name = &group.fields[0];
    let pos = body.iter().position(|s| lhs_name(s) == Some(field_name.as_str()))?;
    let expr = match &body.get(pos)?.kind {
        HirStmtKind::Let { expr: Some(e), .. } => e,
        HirStmtKind::Assign(_, e) => e,
        _ => return None,
    };
    Some(expr_signature(expr))
}

fn lhs_name(stmt: &HirStmt) -> Option<&str> {
    match &stmt.kind {
        HirStmtKind::Let { name, .. } => Some(name),
        HirStmtKind::Assign(HirExpr { kind: HirExprKind::Identifier(n), .. }, _) => Some(n),
        _ => None,
    }
}

fn build_def_sites(body: &[HirStmt]) -> HashMap<String, usize> {
    let mut defs = HashMap::new();
    for (i, stmt) in body.iter().enumerate() {
        match &stmt.kind {
            HirStmtKind::Let { name, .. } => {
                defs.insert(name.clone(), i);
            }
            HirStmtKind::Assign(HirExpr { kind: HirExprKind::Identifier(n), .. }, _) => {
                defs.insert(n.clone(), i);
            }
            _ => {}
        }
    }
    defs
}

fn collect_expr_vars(expr: &HirExpr, out: &mut Vec<String>) {
    match &expr.kind {
        HirExprKind::Identifier(n) => out.push(n.clone()),
        HirExprKind::Binary { left, right, .. } => {
            collect_expr_vars(left, out);
            collect_expr_vars(right, out);
        }
        HirExprKind::Unary { inner, .. } => collect_expr_vars(inner, out),
        HirExprKind::Field { receiver, .. } => collect_expr_vars(receiver, out),
        HirExprKind::Cast { inner, .. } => collect_expr_vars(inner, out),
        _ => {}
    }
}

fn all_deps_available(body: &[HirStmt], field_name: &str, template_base_index: usize) -> bool {
    let _ = field_name;
    let def_sites = build_def_sites(body);
    let Some(stmt) = body.get(template_base_index) else {
        return false;
    };
    let rhs = match &stmt.kind {
        HirStmtKind::Let { expr: Some(e), .. } => e,
        HirStmtKind::Assign(_, e) => e,
        _ => return true,
    };
    let mut vars = Vec::new();
    collect_expr_vars(rhs, &mut vars);
    for var in &vars {
        match def_sites.get(var) {
            Some(&pos) if pos < template_base_index => continue,
            Some(_) => return false,
            None => continue,
        }
    }
    true
}

fn merge_groups(body: &[HirStmt], groups: Vec<VectorPhiCandidate>) -> Vec<VectorPhiCandidate> {
    if groups.len() < 2 {
        return groups;
    }
    let mut sig_buckets: HashMap<Vec<u8>, Vec<VectorPhiCandidate>> = HashMap::new();
    for g in &groups {
        if let Some(sig) = group_template_signature(body, g) {
            sig_buckets.entry(sig).or_default().push(g.clone());
        }
    }

    let mut merged = Vec::new();
    for (_, bucket) in sig_buckets {
        if bucket.len() < 2 {
            merged.extend(bucket);
            continue;
        }
        let mut by_width: HashMap<usize, Vec<VectorPhiCandidate>> = HashMap::new();
        for g in bucket {
            by_width.entry(g.width).or_default().push(g);
        }
        for (_, same_width_groups) in by_width {
            if same_width_groups.len() < 2 {
                merged.extend(same_width_groups);
                continue;
            }
            let mut sorted = same_width_groups;
            sorted.sort_by_key(|g| g.fields.first().cloned().unwrap_or_default());
            let template_base = sorted[0].fields.first().cloned().unwrap_or_default();
            let template_base_idx = body
                .iter()
                .position(|s| lhs_name(s) == Some(template_base.as_str()))
                .unwrap_or(0);
            sorted.retain(|g| {
                let first = g.fields.first().cloned().unwrap_or_default();
                let pos = body
                    .iter()
                    .position(|s| lhs_name(s) == Some(first.as_str()))
                    .unwrap_or(0);
                pos == template_base_idx || all_deps_available(body, &first, pos)
            });
            if sorted.len() < 2 {
                merged.extend(sorted);
                continue;
            }
            let template_group = sorted[0].clone();
            let mut merged_fields = template_group.fields.clone();
            for src in sorted.iter().skip(1) {
                merged_fields.extend(src.fields.clone());
            }
            let merged_width = template_group.width * sorted.len();
            let group_name = infer_group_name(&merged_fields);
            merged.push(VectorPhiCandidate {
                group_name,
                element_ty: template_group.element_ty.clone(),
                width: merged_width,
                fields: merged_fields,
            });
        }
    }
    merged
}

pub fn analyze_body(body: &[HirStmt], min_width: usize) -> Vec<VectorPhiCandidate> {
    let mut groups = Vec::new();
    for i in 0..body.len() {
        if let HirStmtKind::Assign(HirExpr { kind: HirExprKind::Identifier(_), .. }, _) =
            &body[i].kind
        {
            for g in find_isomorphic_groups(body, i) {
                if g.width >= 2 {
                    groups.push(g);
                }
            }
        }
    }

    if groups.len() >= 10 {
        let old = std::mem::take(&mut groups);
        groups = merge_groups(body, old);
    }

    groups.into_iter().filter(|g| g.width >= min_width).collect()
}
