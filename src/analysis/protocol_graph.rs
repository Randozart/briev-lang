// 2026-07-30: Protocol round-trip and cross-op equivalence verification.
// The ProtocolGraph struct was merged into CastingGraph (src/casting/graph.rs).
// Only verification functions remain here.

use crate::ast::top::{CastDirection, CastEdge, ProtocolDef, TopLevel};
use crate::ast::{Contract, Expr, PropertyValue, Statement, Type};

/// Find a defn body by name in the top-level items, returning the first
/// parameter's name alongside the body expression.
/// 2026-08-28 (P1.5): the caller must key the symbolic input by the defn's
/// ACTUAL parameter name — the bodies reference their declared param (`x`),
/// not the CastBinding's `#Lh`. Keying by "#Lh" made every identifier resolve
/// to Unknown, so add/sub inverse pairs were never proven (the SMT fallback
/// then failed without z3).
pub fn find_defn_param_body<'a>(name: &str, items: &'a [TopLevel]) -> Option<(String, &'a Expr)> {
    for item in items {
        if let TopLevel::Definition(d) = item {
            if d.name == name {
                let param = d.parameters.first().map(|(p, _)| p.clone()).unwrap_or_else(|| "#Lh".to_string());
                for stmt in &d.body {
                    if let Statement::Term(val) = stmt {
                        if let Some(expr) = val {
                            return Some((param, expr));
                        }
                    }
                }
                return None;
            }
        }
    }
    None
}

/// Find a defn body by name in the top-level items.
/// Returns the body expression if it's a single-term defn.
pub fn find_defn_body<'a>(name: &str, items: &'a [TopLevel]) -> Option<&'a Expr> {
    find_defn_param_body(name, items).map(|(_, e)| e)
}

/// 2026-08-03 (P1.5): Prove that `inverse_body(forward_body(x)) == x` via
/// symbolic evaluation, falling back to SMT (linear ops like `<<1`/`>>1`
/// prove cleanly). Returns false when the composition is NOT provably
/// identity — the caller then simply doesn't collapse the pair (the cast is
/// emitted correctly, just not for free). Never a guess.
fn prove_composition_inverse(param: &str, forward_body: &Expr, inverse_body: &Expr) -> bool {
    use crate::symbolic::{eval_symbolic_expr, SymbolicValue};
    let sym_input = {
        let mut m = std::collections::HashMap::new();
        m.insert(param.to_string(), SymbolicValue::Identifier("__x".into()));
        m
    };
    let mid = eval_symbolic_expr(forward_body, &sym_input);
    let mut inv_input = std::collections::HashMap::new();
    inv_input.insert(param.to_string(), mid);
    let output = eval_symbolic_expr(inverse_body, &inv_input);

    if symbolic_deep_equals(&output, &SymbolicValue::Identifier("__x".into())) {
        return true;
    }

    let formula = build_roundtrip_smt(forward_body, inverse_body);
    matches!(
        crate::proof_engine::smt::prove_smt_formula(&formula, 1000),
        crate::proof_engine::smt::SmtResult::Unsat
    )
}

/// 2026-08-03 (P1.5): find cross-type PROVEN-INVERSE pairs among the
/// program's `proto` declarations. For each pair of distinct protos in the
/// same category sharing a base target, prove
/// `b.CastFrom(base)(a.CastTo(base)(x)) == x`. Returns
/// `(category, variant_a, variant_b)` triples — casting a → b through the
/// base is a ZERO delta (identity), the sub-types are 1-to-1. Non-provable
/// pairs are skipped (correct, not free).
pub fn find_inverse_pairs(items: &[TopLevel]) -> Vec<(String, String, String)> {
    let protos: Vec<&ProtocolDef> = items
        .iter()
        .filter_map(|i| match i {
            TopLevel::ProtocolDef(pd) => Some(pd),
            _ => None,
        })
        .collect();

    let mut pairs = Vec::new();
    for (i, a) in protos.iter().enumerate() {
        for b in &protos[i + 1..] {
            if a.category != b.category {
                continue;
            }
            let a_to = a.cast_edges.iter().find(|e| {
                matches!(e.direction, CastDirection::CastTo) && e.binding.is_some()
            });
            let b_from = b.cast_edges.iter().find(|e| {
                matches!(e.direction, CastDirection::CastFrom) && e.binding.is_some()
            });
            let (Some(at), Some(bf)) = (a_to, b_from) else { continue };
            if at.target_category != bf.target_category || at.target_variant != bf.target_variant {
                continue;
            }
            let Some(at_fn) = at.binding.as_ref().map(|x| &x.fn_name) else { continue };
            let Some(bf_fn) = bf.binding.as_ref().map(|x| &x.fn_name) else { continue };
            let (Some((param, fwd)), Some((_, inv))) =
                (find_defn_param_body(at_fn, items), find_defn_param_body(bf_fn, items)) else {
                continue;
            };
            if prove_composition_inverse(&param, fwd, inv) {
                pairs.push((a.category.clone(), a.name.clone(), b.name.clone()));
            }
        }
    }
    pairs
}

/// Verify round-trip identity for a protocol declaration.
/// For matching CastTo/CastFrom pairs, proves that
/// CastFrom(CastTo(x)) == x via symbolic evaluation or SMT.
pub fn verify_protocol_roundtrip(
    pd: &ProtocolDef,
    items: &[TopLevel],
) -> Result<(), String> {
    let to = pd.cast_edges.iter().find(|e| {
        matches!(e.direction, CastDirection::CastTo) && e.binding.is_some()
    });
    let from = pd.cast_edges.iter().find(|e| {
        matches!(e.direction, CastDirection::CastFrom) && e.binding.is_some()
    });

    let (to_edge, from_edge) = match (to, from) {
        (Some(t), Some(f)) if t.target_category == f.target_category => (t, f),
        _ => return Ok(()),
    };

    // 2026-08-28 (axiom facility): a pair DECLARED trusted (`axiom` prefix)
    // skips the round-trip proof — the implementations live across the FFI
    // boundary (no Briev body to evaluate) and the trust enters the
    // authority ledger instead. Provable pairs stay provable; axioms are
    // declared, never assumed.
    if to_edge.trusted_axiom || from_edge.trusted_axiom {
        return Ok(());
    }

    let to_fn = to_edge.binding.as_ref().map(|b| &b.fn_name).unwrap();
    let from_fn = from_edge.binding.as_ref().map(|b| &b.fn_name).unwrap();

    let forward_body = find_defn_body(to_fn, items);
    let inverse_body = find_defn_body(from_fn, items);

    match (forward_body, inverse_body) {
        (Some(fwd), Some(inv)) => {
            let sym_input = {
                let mut m = std::collections::HashMap::new();
                m.insert("#Lh".to_string(), crate::symbolic::SymbolicValue::Identifier("__x".into()));
                m
            };

            let mid = crate::symbolic::eval_symbolic_expr(fwd, &sym_input);
            let mut inv_input = std::collections::HashMap::new();
            inv_input.insert("#Lh".to_string(), mid.clone());
            let output = crate::symbolic::eval_symbolic_expr(inv, &inv_input);

            if symbolic_deep_equals(&output, &crate::symbolic::SymbolicValue::Identifier("__x".into())) {
                return Ok(());
            }

            eprintln!("warning: symbolic round-trip inconclusive for '{}', trying SMT", pd.name);
            let formula = build_roundtrip_smt(fwd, inv);
            let result = crate::proof_engine::smt::prove_smt_formula(&formula, 1000);
            match result {
                crate::proof_engine::smt::SmtResult::Unsat => Ok(()),
                _ => Err(format!(
                    "round-trip proof failed for protocol '{}': {} and {} are not inverses",
                    pd.name, to_fn, from_fn
                )),
            }
        }
        _ => {
            // 2026-08-27 (bug sweep B1.2): a declared CastTo/CastFrom pair
            // whose binding bodies are missing is a HARD ERROR — the old
            // warning+Ok shipped unprovable conversions to every consumer.
            // The round-trip gate only has teeth when both functions exist.
            let missing = [
                find_defn_body(to_fn, items).is_none().then(|| to_fn.clone()),
                find_defn_body(from_fn, items).is_none().then(|| from_fn.clone()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ");
            Err(format!(
                "round-trip proof for protocol '{}': conversion function(s) {} \
                 have no body\n  why: a CastTo/CastFrom pair must be provable — \
                 an inverse without an implementation cannot be checked and may \
                 silently corrupt data\n  fix: define {} in this module (or a \
                 module it imports), or remove the corresponding CastTo/CastFrom \
                 binding from 'proto {}'",
                pd.name,
                missing,
                missing,
                pd.name
            ))
        }
    }
}

/// Verify cross-op equivalence for a protocol declaration.
/// For each cross-op override, proves that the custom implementation
/// matches the default CastTo -> op -> CastFrom round-trip.
pub fn verify_crossop_equivalence(
    pd: &ProtocolDef,
    items: &[TopLevel],
) -> Result<(), String> {
    let to = pd.cast_edges.iter().find(|e| {
        matches!(e.direction, CastDirection::CastTo) && e.binding.is_some()
    });
    let from = pd.cast_edges.iter().find(|e| {
        matches!(e.direction, CastDirection::CastFrom) && e.binding.is_some()
    });

    let (to_edge, from_edge) = match (to, from) {
        (Some(t), Some(f)) => (t, f),
        _ => return Ok(()),
    };

    for op in &pd.cross_ops {
        let Some(ref impl_args) = op.impl_args else { continue };

        let custom_fn = format!("{:?}", impl_args);
        let Some(custom_body) = find_defn_body(&custom_fn.trim_matches('"'), items) else {
            eprintln!("warning: cross-op '{}' body not found, skipping equivalence check", custom_fn);
            continue;
        };

        let to_fn = to_edge.binding.as_ref().map(|b| &b.fn_name).unwrap();
        let from_fn = from_edge.binding.as_ref().map(|b| &b.fn_name).unwrap();
        let Some(forward_body) = find_defn_body(to_fn, items) else { continue; };
        let Some(inverse_body) = find_defn_body(from_fn, items) else { continue; };

        let formula = build_crossop_smt(forward_body, inverse_body, custom_body);
        let result = crate::proof_engine::smt::prove_smt_formula(&formula, 1000);
        match result {
            crate::proof_engine::smt::SmtResult::Unsat => {},
            _ => return Err(format!(
                "cross-op equivalence proof failed for protocol '{}': op '{}' not equivalent to round-trip",
                pd.name, op.op
            )),
        }
    }

    Ok(())
}

/// Build an SMT-LIB formula proving the round-trip identity.
fn build_roundtrip_smt(forward: &Expr, inverse: &Expr) -> String {
    let fwd_smt = crate::proof_engine::smt::encode_expr_smt_for_proof(forward);
    let inv_smt = crate::proof_engine::smt::encode_expr_smt_for_proof(inverse);
    format!(
        "(set-logic QF_BV)\n\
         (declare-const x (_ BitVec 64))\n\
         (define-fun forward ((x (_ BitVec 64))) (_ BitVec 64) {})\n\
         (define-fun inverse ((x (_ BitVec 64))) (_ BitVec 64) {})\n\
         (assert (not (= (inverse (forward x)) x)))\n\
         (check-sat)\n",
        fwd_smt, inv_smt
    )
}

/// Build an SMT-LIB formula proving cross-op equivalence.
fn build_crossop_smt(forward: &Expr, inverse: &Expr, custom: &Expr) -> String {
    let fwd_smt = crate::proof_engine::smt::encode_expr_smt_for_proof(forward);
    let inv_smt = crate::proof_engine::smt::encode_expr_smt_for_proof(inverse);
    let custom_smt = crate::proof_engine::smt::encode_expr_smt_for_proof(custom);
    format!(
        "(set-logic QF_BV)\n\
         (declare-const x (_ BitVec 64))\n\
         (declare-const y (_ BitVec 64))\n\
         (define-fun forward ((x (_ BitVec 64))) (_ BitVec 64) {})\n\
         (define-fun inverse ((x (_ BitVec 64))) (_ BitVec 64) {})\n\
         (define-fun default_path ((x (_ BitVec 64)) (y (_ BitVec 64))) (_ BitVec 64)\n\
           (inverse (bvadd (forward x) y)))\n\
         (define-fun custom_path ((x (_ BitVec 64)) (y (_ BitVec 64))) (_ BitVec 64) {})\n\
         (assert (not (= (default_path x y) (custom_path x y))))\n\
         (check-sat)\n",
        fwd_smt, inv_smt, custom_smt
    )
}

/// Simple symbolic comparison (deep equals).
fn symbolic_deep_equals(a: &crate::symbolic::SymbolicValue, b: &crate::symbolic::SymbolicValue) -> bool {
    use crate::symbolic::SymbolicValue;
    match (a, b) {
        (SymbolicValue::Identifier(an), SymbolicValue::Identifier(bn)) => an == bn,
        (SymbolicValue::Literal(av, _), SymbolicValue::Literal(bv, _)) => av == bv,
        (SymbolicValue::Binary(op_a, la, ra), SymbolicValue::Binary(op_b, lb, rb)) => {
            op_a == op_b && symbolic_deep_equals(la, lb) && symbolic_deep_equals(ra, rb)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_edge(dir: CastDirection, cat: &str, var: &str) -> CastEdge {
        CastEdge {
            direction: dir,
            target_category: cat.to_string(),
            target_variant: var.to_string(),
            binding: None,
            trusted_axiom: false,
        }
    }

    #[test]
    fn test_find_defn_body_not_found() {
        let items: Vec<TopLevel> = vec![];
        assert!(find_defn_body("nonexistent", &items).is_none());
    }

    /// 2026-08-27 (bug sweep B1.2): a declared CastTo/CastFrom pair whose
    /// binding bodies are MISSING is a hard error naming the functions —
    /// the old warning+Ok shipped unprovable conversions silently.
    #[test]
    fn test_roundtrip_missing_bodies_is_hard_error() {
        let to = proto_with_binding("Test", CastDirection::CastTo, "test_to_utf8");
        let from =
            proto_with_binding("Test", CastDirection::CastFrom, "utf8_to_test");
        // The declaring proto carries BOTH edges (the helpers above each
        // model ONE edge for the pair-proving path; round-trip needs both).
        let mut pd = match &to {
            TopLevel::ProtocolDef(p) => p.clone(),
            other => panic!("expected protocol, got {other:?}"),
        };
        if let TopLevel::ProtocolDef(from_pd) = &from {
            pd.cast_edges.extend(from_pd.cast_edges.iter().cloned());
        }
        let items: Vec<TopLevel> = vec![to, from];
        // Same category on both edges → pair exists → bodies-missing arm.
        let err = verify_protocol_roundtrip(&pd, &items)
            .expect_err("missing bodies must fail compilation");
        assert!(err.contains("test_to_utf8"), "{err}");
        assert!(err.contains("utf8_to_test"), "{err}");
        assert!(err.contains("no body"), "{err}");
        // House style: what/why/fix.
        assert!(err.contains("why:") && err.contains("fix:"), "{err}");
    }

    #[test]
    fn test_verify_skip_no_pair() {
        let pd = ProtocolDef {
            name: "Test".to_string(),
            category: "String".to_string(),
            contract: None,
            cast_edges: vec![make_edge(CastDirection::CastTo, "String", "UTF8")],
            cross_ops: vec![],
            span: None,
        };
        let items: Vec<TopLevel> = vec![];
        assert!(verify_protocol_roundtrip(&pd, &items).is_ok());
    }

    /// Build `defn <name>(x) -> Int { term x <op> 1; }`.
    fn shift_defn(name: &str, op: crate::ast::BinaryOpKind) -> TopLevel {
        use crate::ast::Definition;
        TopLevel::Definition(Definition {
                variadic_param: None,
            name: name.to_string(),
            type_params: vec![],
            parameters: vec![("x".to_string(), crate::ast::Type::int())],
            output_type: Some(crate::ast::OutputType::Single(crate::ast::Type::int())),
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
            post_authority: false},
            body: vec![Statement::Term(Some(Expr::BinaryOp(
                op,
                Box::new(Expr::Identifier("x".to_string())),
                Box::new(Expr::Decimal(1)),
            )))],
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })
    }

    fn proto_with_binding(name: &str, dir: CastDirection, fn_name: &str) -> TopLevel {
        TopLevel::ProtocolDef(ProtocolDef {
            name: name.to_string(),
            category: "String".to_string(),
            contract: None,
            cast_edges: vec![CastEdge {
                direction: dir,
                target_category: "String".to_string(),
                target_variant: "UTF8".to_string(),
                binding: Some(crate::ast::top::CastBinding {
                    fn_name: fn_name.to_string(),
                    param: "#Lh".to_string(),
                }),
            trusted_axiom: false}],
            cross_ops: vec![],
            span: None,
        })
    }

    #[test]
    fn test_find_inverse_pairs() {
        // 2026-08-03 (P1.5): A.CastTo = `x + 1`, B.CastFrom = `x - 1` — the
        // composition is identity on the full range, so the pair is proven
        // 1-to-1 and the graph collapses the A→B cast to a zero delta.
        // (Note: `x << 1` / `x >> 1` is NOT universal — it only cancels when
        // bit 63 is 0 — so the SMT proof correctly declines that pair.)
        use crate::ast::BinaryOpKind;
        let items: Vec<TopLevel> = vec![
            shift_defn("add_one", BinaryOpKind::Add),
            shift_defn("sub_one", BinaryOpKind::Sub),
            proto_with_binding("A", CastDirection::CastTo, "add_one"),
            proto_with_binding("B", CastDirection::CastFrom, "sub_one"),
        ];
        let pairs = find_inverse_pairs(&items);
        assert!(
            pairs.iter().any(|(cat, a, b)| cat == "String" && a == "A" && b == "B"),
            "add/sub pair should be proven inverse, got {:?}",
            pairs
        );
    }
}