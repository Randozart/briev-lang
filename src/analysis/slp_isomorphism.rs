use crate::ast::{BinaryOpKind, Expr, Statement, Type, UnaryOpKind};
use std::collections::{HashMap, HashSet};

/// A group of expressions that can be promoted to a vector phi node.
/// Fields within a group share the same LLVM type and have structurally
/// isomorphic assignment expressions (same operator tree, same literal
/// values, but different variable names per lane).
#[derive(Debug, Clone)]
pub struct VectorPhiCandidate {
    /// Group name derived from common field prefix (descriptive only).
    pub group_name: String,
    /// The Briev type of each field in this group.
    pub element_ty: Type,
    /// Number of lanes in the group.
    pub width: usize,
    /// The field names in this group, in index order.
    pub fields: Vec<String>,
}

/// Check if two expressions are structurally isomorphic under a variable mapping.
/// The mapping maps variable names from expression `a` to expression `b`.
/// Returns true if the expressions have the same structure (same BinaryOp kinds,
/// same literals) up to the variable renaming defined by `mapping`.
pub fn exprs_isomorphic(
    a: &Expr,
    b: &Expr,
    mapping: &HashMap<String, String>,
) -> bool {
    match (a, b) {
        (Expr::BinaryOp(k1, l1, r1), Expr::BinaryOp(k2, l2, r2)) => {
            k1 == k2
                && exprs_isomorphic(l1, l2, mapping)
                && exprs_isomorphic(r1, r2, mapping)
        }
        (Expr::UnaryOp(k1, e1), Expr::UnaryOp(k2, e2)) => {
            k1 == k2 && exprs_isomorphic(e1, e2, mapping)
        }
        (Expr::Decimal(n1), Expr::Decimal(n2)) => n1 == n2,
        (Expr::Float(n1), Expr::Float(n2)) => (n1 - n2).abs() < 1e-10,
        (Expr::Bool(b1), Expr::Bool(b2)) => b1 == b2,
        (Expr::Identifier(n1), Expr::Identifier(n2)) => {
            match mapping.get(n1) {
                Some(mapped) => mapped == n2,
                None => n1 == n2,
            }
        }
        (Expr::Cast(e1, t1), Expr::Cast(e2, t2)) => {
            t1 == t2 && exprs_isomorphic(e1, e2, mapping)
        }
        (Expr::Field(obj1, f1), Expr::Field(obj2, f2)) => {
            f1 == f2 && exprs_isomorphic(obj1, obj2, mapping)
        }
        _ => false,
    }
}

/// Build a variable mapping from two expressions by finding the first identifier
/// difference and using it to create the mapping.
pub fn build_mapping(
    a: &Expr,
    b: &Expr,
) -> Option<HashMap<String, String>> {
    let mut mapping = HashMap::new();
    if try_build_mapping(a, b, &mut mapping) {
        Some(mapping)
    } else {
        None
    }
}

fn try_build_mapping(
    a: &Expr,
    b: &Expr,
    mapping: &mut HashMap<String, String>,
) -> bool {
    match (a, b) {
        (Expr::BinaryOp(k1, l1, r1), Expr::BinaryOp(k2, l2, r2)) => {
            k1 == k2
                && try_build_mapping(l1, l2, mapping)
                && try_build_mapping(r1, r2, mapping)
        }
        (Expr::UnaryOp(k1, e1), Expr::UnaryOp(k2, e2)) => {
            k1 == k2 && try_build_mapping(e1, e2, mapping)
        }
        (Expr::Decimal(n1), Expr::Decimal(n2)) => n1 == n2,
        (Expr::Float(n1), Expr::Float(n2)) => (n1 - n2).abs() < 1e-10,
        (Expr::Bool(b1), Expr::Bool(b2)) => b1 == b2,
        (Expr::Identifier(n1), Expr::Identifier(n2)) => {
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
        (Expr::Cast(e1, t1), Expr::Cast(e2, t2)) => {
            t1 == t2 && try_build_mapping(e1, e2, mapping)
        }
        (Expr::Field(obj1, f1), Expr::Field(obj2, f2)) => {
            f1 == f2 && try_build_mapping(obj1, obj2, mapping)
        }
        _ => false,
    }
}

/// Check if two statements are structurally isomorphic.
pub fn statements_isomorphic(
    a: &Statement,
    b: &Statement,
) -> Option<HashMap<String, String>> {
    match (a, b) {
        (Statement::Let { name: n1, ty: t1, expr: e1, .. },
         Statement::Let { name: n2, ty: t2, expr: e2, .. }) => {
            if t1 != t2 { return None; }
            let mapping = build_mapping(e1.as_ref()?, e2.as_ref()?)?;
            Some(mapping)
        }
        (Statement::Assign(l1, e1), Statement::Assign(l2, e2)) => {
            let mut mapping = HashMap::new();
            if !try_build_mapping_lhs(l1, l2, &mut mapping) {
                return None;
            }
            // 2026-07-29: Also build mapping from RHS expressions. The LHS-only
            // mapping misses identifiers in the assignment RHS (e.g., nvx0 → nvx1
            // in vx0 = nvx0 vs vx1 = nvx1), causing false negatives for nbody's
            // velocity assignments and position updates. build_mapping handles
            // recursive expression traversal — same function used for Let mapping.
            if let Some(rhs_map) = build_mapping(e1, e2) {
                mapping.extend(rhs_map);
            }
            if exprs_isomorphic(e1, e2, &mapping) {
                Some(mapping)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn try_build_mapping_lhs(
    a: &Expr,
    b: &Expr,
    mapping: &mut HashMap<String, String>,
) -> bool {
    match (a, b) {
        (Expr::Identifier(n1), Expr::Identifier(n2)) => {
            if n1 == n2 { return true; }
            if mapping.contains_key(n1) {
                mapping[n1] == *n2
            } else {
                mapping.insert(n1.clone(), n2.clone());
                true
            }
        }
        (Expr::Field(obj1, f1), Expr::Field(obj2, f2)) => {
            f1 == f2 && try_build_mapping_lhs(obj1, obj2, mapping)
        }
        _ => a == b,
    }
}

/// Find groups of isomorphic statements starting at start_idx.
fn find_isomorphic_groups(
    body: &[Statement],
    start_idx: usize,
) -> Vec<VectorPhiCandidate> {
    let mut groups = Vec::new();
    if start_idx >= body.len() {
        return groups;
    }

    let template = &body[start_idx];
    let mut lhs_names: Vec<String> = Vec::new();

    let template_lhs = match template {
        Statement::Let { name, .. } => Some(name.clone()),
        Statement::Assign(Expr::Identifier(n), _) => Some(n.clone()),
        _ => None,
    };
    let template_lhs = match template_lhs {
        Some(n) => n,
        None => return groups,
    };
    lhs_names.push(template_lhs.clone());

    for (offset, stmt) in body.iter().enumerate().skip(start_idx + 1) {
        if statements_isomorphic(template, stmt).is_some() {
            let lhs = match stmt {
                Statement::Let { name, .. } => name.clone(),
                Statement::Assign(Expr::Identifier(n), _) => n.clone(),
                _ => break,
            };
            lhs_names.push(lhs);
        } else {
            break;
        }
    }

    if lhs_names.len() >= 2 {
        let width = lhs_names.len();
        let element_type = match template {
            Statement::Let { ty: Some(t), .. } => t.clone(),
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

/// Guess a descriptive group name from a set of field names.
/// e.g., ["bx0", "bx1", "bx2", "bx3"] → "bx"
fn infer_group_name(fields: &[String]) -> String {
    if fields.is_empty() {
        return "g".to_string();
    }
    // Find the longest common prefix
    let first = &fields[0];
    for len in (1..=first.len()).rev() {
        let prefix = &first[..len];
        if fields.iter().all(|f| f.starts_with(prefix)) {
            // Require that all fields have digits (or nothing) after the prefix
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

fn expr_signature(expr: &Expr) -> Vec<u8> {
    let mut sig = Vec::new();
    expr_signature_inner(expr, &mut sig);
    sig
}

fn expr_signature_inner(expr: &Expr, sig: &mut Vec<u8>) {
    match expr {
        Expr::BinaryOp(kind, l, r) => {
            sig.push(1);
            sig.push(match kind {
                BinaryOpKind::Add => 1, BinaryOpKind::Sub => 2,
                BinaryOpKind::Mul => 3, BinaryOpKind::Div => 4,
                BinaryOpKind::Eq => 5, BinaryOpKind::Neq => 6,
                BinaryOpKind::Lt => 7, BinaryOpKind::Gt => 8,
                BinaryOpKind::Le => 9, BinaryOpKind::Ge => 10,
                _ => 0,
            });
            expr_signature_inner(l, sig);
            expr_signature_inner(r, sig);
        }
        Expr::UnaryOp(kind, e) => {
            sig.push(2);
            sig.push(match kind {
                UnaryOpKind::Neg => 1, UnaryOpKind::Not => 2,
                _ => 0,
            });
            expr_signature_inner(e, sig);
        }
        Expr::Identifier(_) => { sig.push(3); }
        Expr::Decimal(n) => {
            sig.push(4);
            for &b in &n.to_le_bytes() { sig.push(b); }
        }
        Expr::Float(f) => {
            sig.push(5);
            let bits = f.to_bits();
            for &b in &bits.to_le_bytes() { sig.push(b); }
        }
        Expr::Bool(b) => {
            sig.push(6);
            sig.push(if *b { 1 } else { 0 });
        }
        _ => { sig.push(0); }
    }
}

fn group_template_signature(body: &[Statement], group: &VectorPhiCandidate) -> Option<Vec<u8>> {
    // Find the first field in the body to get its expression
    let field_name = &group.fields[0];
    let pos = body.iter().position(|s| match s {
        Statement::Let { name, .. } => name == field_name,
        Statement::Assign(Expr::Identifier(n), _) => n == field_name,
        _ => false,
    })?;
    let stmt = body.get(pos)?;
    let expr = match stmt {
        Statement::Let { expr: Some(e), .. } => &*e,
        Statement::Assign(_, e) => &*e,
        _ => return None,
    };
    Some(expr_signature(expr))
}

fn build_def_sites(body: &[Statement]) -> HashMap<String, usize> {
    let mut defs = HashMap::new();
    for (i, stmt) in body.iter().enumerate() {
        match stmt {
            Statement::Let { name, .. } => { defs.insert(name.clone(), i); }
            Statement::Assign(lhs, _) => {
                if let Expr::Identifier(n) = &*lhs { defs.insert(n.clone(), i); }
            }
            _ => {}
        }
    }
    defs
}

fn collect_expr_vars(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Identifier(n) => out.push(n.clone()),
        Expr::BinaryOp(_, l, r) => { collect_expr_vars(l, out); collect_expr_vars(r, out); }
        Expr::UnaryOp(_, e) => collect_expr_vars(e, out),
        Expr::Field(obj, _) => collect_expr_vars(obj, out),
        Expr::Cast(e, _) => collect_expr_vars(e, out),
        _ => {}
    }
}

fn all_deps_available(
    body: &[Statement],
    field_name: &str,
    template_base_index: usize,
) -> bool {
    let def_sites = build_def_sites(body);
    let stmt = body.get(template_base_index);
    let Some(stmt) = stmt else { return false; };
    let rhs = match stmt {
        Statement::Let { expr: Some(e), .. } => &*e,
        Statement::Assign(_, e) => &*e,
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

/// Merge groups with the same template signature into wider cross-pair groups.
fn merge_groups(body: &[Statement], groups: Vec<VectorPhiCandidate>) -> Vec<VectorPhiCandidate> {
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

    // 2026-11-05 (house rule §4): HashMap iteration order varies with the
    // per-process SipHash seed; unsorted iteration randomized the merged
    // vector-group order, drifting loop IR run-to-run (nbody_sqrt). Iterate
    // signature keys in sorted order so the merge output is byte-stable.
    let mut sig_keys: Vec<Vec<u8>> = sig_buckets.keys().cloned().collect();
    sig_keys.sort();
    for sig in sig_keys {
        let bucket = sig_buckets.remove(&sig).unwrap();
        if bucket.len() < 2 {
            merged.extend(bucket);
            continue;
        }

        let mut by_width: HashMap<usize, Vec<VectorPhiCandidate>> = HashMap::new();
        for g in bucket {
            by_width.entry(g.width).or_default().push(g);
        }

        let mut width_keys: Vec<usize> = by_width.keys().copied().collect();
        width_keys.sort_unstable();
        for width_key in width_keys {
            let mut same_width_groups = by_width.remove(&width_key).unwrap();
            if same_width_groups.len() < 2 {
                merged.extend(same_width_groups);
                continue;
            }

            let mut sorted = same_width_groups;
            sorted.sort_by_key(|g| {
                // Find position of first field in body
                g.fields.first().cloned().unwrap_or_default()
            });
            // Keep only groups where dependencies are available at template position
            let template_base = sorted[0].fields.first().cloned().unwrap_or_default();
            let template_base_idx = body.iter().position(|s| match s {
                Statement::Let { name, .. } => *name == template_base,
                Statement::Assign(Expr::Identifier(n), _) => *n == template_base,
                _ => false,
            }).unwrap_or(0);
            sorted.retain(|g| {
                let first = g.fields.first().cloned().unwrap_or_default();
                let pos = body.iter().position(|s| match s {
                    Statement::Let { name, .. } => *name == first,
                    Statement::Assign(Expr::Identifier(n), _) => *n == first,
                    _ => false,
                }).unwrap_or(0);
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

/// Analyze a transaction body for vector phi opportunities.
/// Returns groups of fields that can share a single vector phi node.
/// A group is valid only when:
/// 1. All fields have the same LLVM type
/// 2. All fields are unconditionally written (not inside `when` guards)
/// 3. All fields have structurally isomorphic assignment expressions
///
/// Only groups of width >= `min_width` are returned — smaller groups don't
/// justify the insertelement/extractelement overhead.
///
/// 2026-07-31: Phase 3 (§8.1) — `min_width` is config-driven
/// (config/targets.dbvl `vector_min_width`, default 4) instead of a literal.
pub fn analyze_body(body: &[Statement], min_width: usize) -> Vec<VectorPhiCandidate> {
    let mut groups = Vec::new();
    let mut i = 0;
    while i < body.len() {
        match &body[i] {
            Statement::Assign(Expr::Identifier(_), _) => {
                let found = find_isomorphic_groups(body, i);
                for g in found {
                    if g.width >= 2 {
                        groups.push(g);
                    }
                }
                i += 1;
            }
            _ => { i += 1; }
        }
    }

    // Cross-pair merge: when many narrow groups share a signature,
    // combine them into wider groups (e.g., Newton-step-2 from pairs 01-10).
    if groups.len() >= 10 {
        let old_groups = std::mem::take(&mut groups);
        let merged = merge_groups(body, old_groups);
        groups = merged;
    }

    // Only return groups that are wide enough to justify vector phi overhead.
    groups.into_iter().filter(|g| g.width >= min_width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, Expr, Statement};

    fn float_expr(val: f64) -> Expr { Expr::Float(val) }
    fn ident(n: &str) -> Expr { Expr::Identifier(n.to_string()) }
    fn add_expr(a: Expr, b: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Add, Box::new(a), Box::new(b)) }
    fn sub_expr(a: Expr, b: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Sub, Box::new(a), Box::new(b)) }
    fn mul_expr(a: Expr, b: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Mul, Box::new(a), Box::new(b)) }

    #[test]
    fn test_isomorphic_simple_add() {
        let a = add_expr(ident("x"), float_expr(1.0));
        let b = add_expr(ident("y"), float_expr(1.0));
        let mapping = build_mapping(&a, &b);
        assert!(mapping.is_some(), "x+1 vs y+1 should be isomorphic");
        assert_eq!(mapping.unwrap().get("x"), Some(&"y".to_string()));
    }

    #[test]
    fn test_isomorphic_chain() {
        let a = mul_expr(add_expr(ident("x"), ident("y")), float_expr(0.5));
        let b = mul_expr(add_expr(ident("a"), ident("b")), float_expr(0.5));
        let mapping = build_mapping(&a, &b);
        assert!(mapping.is_some(), "0.5*(x+y) vs 0.5*(a+b) should be isomorphic");
    }

    #[test]
    fn test_not_isomorphic_different_op() {
        let a = add_expr(ident("x"), float_expr(1.0));
        let b = sub_expr(ident("x"), float_expr(1.0));
        assert!(!exprs_isomorphic(&a, &b, &HashMap::new()));
    }

    #[test]
    fn test_statement_isomorphic_let() {
        let a = Statement::Let {
            names: vec![],
            name: "dx01".to_string(),
            ty: Some(Type::float()),
            expr: Some(sub_expr(ident("bx0"), ident("bx1"))),
            modifiers: vec![],
        };
        let b = Statement::Let {
            names: vec![],
            name: "dx02".to_string(),
            ty: Some(Type::float()),
            expr: Some(sub_expr(ident("bx0"), ident("bx2"))),
            modifiers: vec![],
        };
        let mapping = statements_isomorphic(&a, &b);
        assert!(mapping.is_some(), "dx01 vs dx02 should be isomorphic");
    }

    #[test]
    fn test_nbody_distance_pattern() {
        let body = vec![
            Statement::Let {
                names: vec![],
                name: "dx01".to_string(),
                ty: Some(Type::float()),
                expr: Some(sub_expr(ident("bx0"), ident("bx1"))),
                modifiers: vec![],
            },
            Statement::Let {
                names: vec![],
                name: "dy01".to_string(),
                ty: Some(Type::float()),
                expr: Some(sub_expr(ident("by0"), ident("by1"))),
                modifiers: vec![],
            },
            Statement::Let {
                names: vec![],
                name: "dz01".to_string(),
                ty: Some(Type::float()),
                expr: Some(sub_expr(ident("bz0"), ident("bz1"))),
                modifiers: vec![],
            },
        ];
        let result = analyze_body(&body, 4);
        assert!(result.is_empty(),
            "dx01/dy01/dz01 width=3 should be filtered (width < 4)");

        // Now test with 4 lanes (bx0..bx3)
        let body4 = vec![
            Statement::Let { names: vec![], name: "bx0".to_string(), ty: Some(Type::float()),
                expr: Some(add_expr(ident("ax"), ident("ay"))), modifiers: vec![] },
            Statement::Let { names: vec![], name: "bx1".to_string(), ty: Some(Type::float()),
                expr: Some(add_expr(ident("ax"), ident("ay"))), modifiers: vec![] },
            Statement::Let { names: vec![], name: "bx2".to_string(), ty: Some(Type::float()),
                expr: Some(add_expr(ident("ax"), ident("ay"))), modifiers: vec![] },
            Statement::Let { names: vec![], name: "bx3".to_string(), ty: Some(Type::float()),
                expr: Some(add_expr(ident("ax"), ident("ay"))), modifiers: vec![] },
        ];
        let result4 = analyze_body(&body4, 4);
        // 2026-07-29: analyze_body no longer processes Statement::Let (temporary
        // locals are not loop-carried state). Only Statement::Assign groups are
        // returned. Let-based groups are skipped.
        assert!(result4.is_empty(),
            "analyze_body skips Statement::Let — no groups expected");
    }

    #[test]
    fn test_assign_isomorphic_rhs_rename() {
        // vx0 = nvx0 vs vx1 = nvx1 — RHS identifiers differ but should be
        // isomorphic via the RHS mapping (build_mapping from RHS expressions).
        // Before the fix, the LHS-only mapping missed nvx0→nvx1.
        let a = Statement::Assign(
            Expr::Identifier("vx0".to_string()),
            Expr::Identifier("nvx0".to_string()),
        );
        let b = Statement::Assign(
            Expr::Identifier("vx1".to_string()),
            Expr::Identifier("nvx1".to_string()),
        );
        let mapping = statements_isomorphic(&a, &b);
        assert!(mapping.is_some(),
            "vx0=nvx0 vs vx1=nvx1 should be isomorphic");
        assert_eq!(mapping.unwrap().get("vx0"), Some(&"vx1".to_string()));
    }

    #[test]
    fn test_nbody_velocity_assign_pattern() {
        // 5 consecutive scalar velocity assignments: vx0=nvx0..vx4=nvx4
        // Should form a width-5 group via analyze_body.
        let body = vec![
            Statement::Assign(Expr::Identifier("vx0".to_string()),
                Expr::Identifier("nvx0".to_string())),
            Statement::Assign(Expr::Identifier("vx1".to_string()),
                Expr::Identifier("nvx1".to_string())),
            Statement::Assign(Expr::Identifier("vx2".to_string()),
                Expr::Identifier("nvx2".to_string())),
            Statement::Assign(Expr::Identifier("vx3".to_string()),
                Expr::Identifier("nvx3".to_string())),
            Statement::Assign(Expr::Identifier("vx4".to_string()),
                Expr::Identifier("nvx4".to_string())),
        ];
        let result = analyze_body(&body, 4);
        assert!(!result.is_empty(),
            "5 consecutive isomorphic scalar assigns should form groups");
        assert_eq!(result[0].width, 5,
            "first group should have width 5");
        assert_eq!(result[0].fields, vec!["vx0", "vx1", "vx2", "vx3", "vx4"]);
    }

    #[test]
    fn test_infer_group_name() {
        assert_eq!(infer_group_name(&["bx0".into(), "bx1".into(), "bx2".into()]), "bx");
        assert_eq!(infer_group_name(&["vx".into(), "vy".into(), "vz".into()]), "g");
        assert_eq!(infer_group_name(&["a".into(), "b".into()]), "g");
    }
}
