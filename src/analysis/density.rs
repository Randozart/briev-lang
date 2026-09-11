// ── Float Computation Density Measurement ───────────────────────────
//
// 2026-07-31: Phase 2 (plan §7.1) — per-txn float computation density,
// computed ONCE in the frontend and consumed by the LLVM backend's
// `#11 → #0` memory-attribute downgrade. This replaces the backend's
// re-derived metric at emit_toplevel.rs:1820-1849, which counted cross
// ops with a `_all_idents` set that was never used.
//
// The metric answers: "is this txn a dense float matrix computation?"
// Dense matrices (kalman ~5.0 ops/field) make LLVM's auto-vectorizer
// emit wide <12 x float> vectors that spill registers (the kalman 3.5x
// regression); sparse force-pair loops (nbody ~3.7 ops/field) do not.
//
// The analysis version counts a BinaryOp only when each operand side
// references ≥1 identifier in the txn's FLOAT set — int-only counter
// arithmetic no longer inflates the count (the `_all_idents` gap). For
// all-float txns (kalman, nbody) this reproduces the old count exactly.
//
// Float determination is a structural fixpoint over float literals and
// float-typed bindings. TEMP: 2026-07-31 — this uses type names for the
// float check because the analysis layer has no TypeUniverse. Plan §8.4
// (D4) replaces this with `is_protocol_member(ty, "Float")` via the
// casting graph in Phase 3.

use crate::ast::{Expr, Statement, TopLevel, Type};
use std::collections::{HashMap, HashSet};

/// Per-txn float computation density measurement (plan §7.1).
#[derive(Debug, Clone, PartialEq)]
pub struct ComputeDensity {
    /// Distinct float let-bindings in the txn body (top-level only).
    /// Matches the backend's prior `float_body_idents` denominator.
    pub float_idents: usize,
    /// BinaryOps whose operands both reference a float identifier.
    pub cross_ops: u32,
    /// cross_ops / float_idents. NaN-safe: 0 when float_idents == 0.
    pub per_field: f64,
}

/// Compute the float density for every reactive txn in the program.
///
/// 2026-07-31: Keyed by txn name, matching `AnalysisResults.loop_shapes`.
/// Only reactive txns are measured — callable txns never reach the
/// `#11 → #0` memory-attribute downgrade (that path is per reactive txn).
pub fn compute_densities(items: &[TopLevel]) -> HashMap<String, ComputeDensity> {
    let program_float = collect_program_float_bindings(items);
    let mut out = HashMap::new();
    for item in items {
        if let TopLevel::Transaction(t) = item {
            if t.is_reactive {
                out.insert(t.name.clone(), density_of_body(&t.body, &program_float));
            }
        }
    }
    out
}

/// Build the set of float identifiers declared at program scope.
///
/// 2026-07-31: Covers top-level state fields (`let x: Float = ...`),
/// `StateDecl`, and `const` declarations. Uses a fixpoint: a binding whose
/// initializer references a float binding is itself float (e.g. nbody's
/// `const m0 = 1.0f32 * solar_mass`).
fn collect_program_float_bindings(items: &[TopLevel]) -> HashSet<String> {
    let mut bindings: Vec<(String, Option<Type>, &Expr)> = Vec::new();
    for item in items {
        match item {
            TopLevel::Statement(s) => {
                if let Statement::Let { name, ty, expr, .. } = s.as_ref() {
                    if let Some(e) = expr {
                        bindings.push((name.clone(), ty.clone(), e));
                    }
                }
            }
            TopLevel::Constant(c) => bindings.push((c.name.clone(), Some(c.ty.clone()), &c.expr)),
            TopLevel::StateDecl(d) => {
                bindings.push((d.name.clone(), Some(d.ty.clone()), &Expr::Bool(false)));
            }
            _ => {}
        }
    }
    float_fixpoint(&bindings)
}

/// Compute the float identifier set of the txn body (top-level lets) seeded
/// with the program float bindings, then measure density over those lets.
///
/// 2026-07-31: Only TOP-LEVEL `Statement::Let` bodies are scanned — nested
/// guard lets (e.g. kalman's `when count % 5000000 == 0 { let energy ... }`)
/// were excluded by the backend metric and are excluded here to keep the
/// measurement identical. See emit_toplevel.rs:1832-1841.
fn density_of_body(body: &[Statement], program_float: &HashSet<String>) -> ComputeDensity {
    let top_lets: Vec<&Statement> = body.iter().filter(|s| matches!(s, Statement::Let { .. })).collect();
    // Fixpoint: a top-level let is float if its RHS references a float
    // identifier or contains a float literal. Iterate until stable.
    let mut float_set: HashSet<String> = program_float.clone();
    loop {
        let before = float_set.len();
        for s in &top_lets {
            if let Statement::Let { name, names, expr: Some(e), .. } = s {
                if expr_is_float(e, &float_set) {
                    float_set.insert(name.clone());
                    for n in names {
                        float_set.insert(n.clone());
                    }
                }
            }
        }
        if float_set.len() == before {
            break;
        }
    }
    let float_idents = top_lets
        .iter()
        .filter(|s| matches!(s, Statement::Let { name, .. } if float_set.contains(name)))
        .count();
    let mut cross_ops: u32 = 0;
    for s in &top_lets {
        if let Statement::Let { expr: Some(e), .. } = s {
            cross_ops += count_cross_float_ops(e, &float_set);
        }
    }
    let per_field = if float_idents == 0 {
        0.0
    } else {
        cross_ops as f64 / float_idents as f64
    };
    ComputeDensity {
        float_idents,
        cross_ops,
        per_field,
    }
}

/// Fixpoint: classify bindings whose initializer references a float binding
/// (or contains a float literal) as float.
fn float_fixpoint(bindings: &[(String, Option<Type>, &Expr)]) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    // Seed from explicit float type annotations and float literals.
    for (name, ty, e) in bindings {
        if ty.as_ref().map_or(false, is_float_type) {
            out.insert(name.clone());
        } else if expr_has_float_literal(e) {
            out.insert(name.clone());
        }
    }
    // Fixpoint: propagate through referenced identifiers.
    loop {
        let before = out.len();
        for (name, _ty, e) in bindings {
            if expr_is_float(e, &out) {
                out.insert(name.clone());
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// Is this type annotation a float protocol type?
///
/// TEMP: 2026-07-31 — name-based until Phase 3 (§8.4 D4) wires the casting
/// graph into analysis. The primitive float set is closed in Briev's
/// bootstrap; a user float type carries an `op Add(Float)` binding and is
/// caught by the literal/operation propagation instead.
fn is_float_type(ty: &Type) -> bool {
    match ty {
        Type::Custom(n) => matches!(
            n.as_str(),
            "Float" | "Float32" | "Float64" | "Double" | "Half" | "BFloat" | "Bfloat16" | "FP16" | "FP32" | "FP64"
        ),
        Type::Constrained(inner, _) => is_float_type(inner),
        _ => false,
    }
}

/// Does the expression contain a bare float literal?
fn expr_has_float_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Float(_) => true,
        Expr::BinaryOp(_, l, r) => expr_has_float_literal(l) || expr_has_float_literal(r),
        Expr::UnaryOp(_, e) => expr_has_float_literal(e),
        Expr::Call(_, args, _) => args.iter().any(expr_has_float_literal),
        Expr::Cast(e, _) => expr_has_float_literal(e),
        Expr::Tuple(ts) => ts.iter().any(expr_has_float_literal),
        Expr::List(xs) => xs.iter().any(expr_has_float_literal),
        Expr::If(c, t, e) => {
            expr_has_float_literal(c)
                || expr_has_float_literal(t)
                || e.as_ref().map_or(false, |x| expr_has_float_literal(x))
        }
        _ => false,
    }
}

/// Is this expression float-typed? Structural: contains a float literal, or
/// references a known-float identifier, or has a float operand (float calls
/// like Sqrt# receive float args — the result is float).
fn expr_is_float(expr: &Expr, float_set: &HashSet<String>) -> bool {
    match expr {
        Expr::Float(_) => true,
        Expr::Identifier(n) => float_set.contains(n),
        Expr::BinaryOp(_, l, r) => expr_is_float(l, float_set) || expr_is_float(r, float_set),
        Expr::UnaryOp(_, e) => expr_is_float(e, float_set),
        Expr::Call(_, args, _) => args.iter().any(|a| expr_is_float(a, float_set)),
        Expr::Cast(e, _) => expr_is_float(e, float_set),
        Expr::Field(o, _) => expr_is_float(o, float_set),
        Expr::Index(a, i) => expr_is_float(a, float_set) || expr_is_float(i, float_set),
        Expr::Tuple(ts) => ts.iter().any(|x| expr_is_float(x, float_set)),
        Expr::List(xs) => xs.iter().any(|x| expr_is_float(x, float_set)),
        Expr::If(c, t, e) => {
            expr_is_float(c, float_set)
                || expr_is_float(t, float_set)
                || e.as_ref().map_or(false, |x| expr_is_float(x, float_set))
        }
        _ => false,
    }
}

/// Count cross-field BinaryOps where each operand side references ≥1
/// identifier in the float set.
///
/// 2026-07-31: This is the FIXED version of the backend's
/// `count_cross_float_ops_in_expr` (emit_toplevel.rs:1557) — the old
/// version ignored its `_all_idents` parameter and counted ANY identifier.
/// Gating on the float set means int-only arithmetic (`i + 1`) no longer
/// inflates the density.
fn count_cross_float_ops(expr: &Expr, float_set: &HashSet<String>) -> u32 {
    match expr {
        Expr::BinaryOp(_, lhs, rhs) => {
            let has_lhs = expr_refs_float(lhs, float_set);
            let has_rhs = expr_refs_float(rhs, float_set);
            let count = if has_lhs && has_rhs { 1u32 } else { 0u32 };
            count + count_cross_float_ops(lhs, float_set) + count_cross_float_ops(rhs, float_set)
        }
        _ => 0,
    }
}

/// Does the expression reference ≥1 identifier in the float set?
fn expr_refs_float(expr: &Expr, float_set: &HashSet<String>) -> bool {
    match expr {
        Expr::Identifier(n) => float_set.contains(n),
        Expr::BinaryOp(_, l, r) => expr_refs_float(l, float_set) || expr_refs_float(r, float_set),
        Expr::UnaryOp(_, e) => expr_refs_float(e, float_set),
        Expr::Call(_, args, _) => args.iter().any(|a| expr_refs_float(a, float_set)),
        Expr::Field(o, _) => expr_refs_float(o, float_set),
        Expr::Index(a, i) => expr_refs_float(a, float_set) || expr_refs_float(i, float_set),
        Expr::Cast(e, _) => expr_refs_float(e, float_set),
        Expr::Deref(e) => expr_refs_float(e, float_set),
        Expr::Tuple(ts) => ts.iter().any(|x| expr_refs_float(x, float_set)),
        Expr::List(xs) => xs.iter().any(|x| expr_refs_float(x, float_set)),
        Expr::If(c, t, e) => {
            expr_refs_float(c, float_set)
                || expr_refs_float(t, float_set)
                || e.as_ref().map_or(false, |x| expr_refs_float(x, float_set))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, Contract, Transaction};

    fn float_expr(v: f64) -> Expr {
        Expr::Float(v)
    }

    fn id(name: &str) -> Expr {
        Expr::Identifier(name.to_string())
    }

    fn add(l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(BinaryOpKind::Add, Box::new(l), Box::new(r))
    }

    fn mul(l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(BinaryOpKind::Mul, Box::new(l), Box::new(r))
    }

    fn let_stmt(name: &str, ty: Option<Type>, e: Expr) -> Statement {
        Statement::Let {
            name: name.to_string(),
            names: Vec::new(),
            ty,
            expr: Some(e),
            modifiers: Vec::new(),
        }
    }

    fn txn(name: &str, body: Vec<Statement>) -> TopLevel {
        TopLevel::Transaction(Transaction {
            name: name.to_string(),
            is_reactive: true,
            is_async: false,
            type_params: Vec::new(),
            parameters: Vec::new(),
            output_type: None,
            outputs: Vec::new(),
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: Vec::new(),
            span: None,
            doc: None,
        })
    }

    /// Kalman-style dense txn: 12 float lets, each combining float fields.
    /// Must produce per_field > 4.0 (the downgrade fires).
    #[test]
    fn dense_kalman_style_downgrades() {
        // Program-scope float bindings: state fields and consts referenced by
        // the txn lets (the real kalman declares all of them; the metric's
        // float set is built from program-scope float declarations).
        let field = |name: &str| TopLevel::Statement(Box::new(Statement::Let {
            name: name.to_string(),
            names: Vec::new(),
            ty: Some(Type::Custom("Float".into())),
            expr: Some(float_expr(0.0)),
            modifiers: Vec::new(),
        }));
        let items = vec![
            field("x0"), field("x1"), field("x2"),
            field("p00"), field("p10"), field("p20"),
            TopLevel::Constant(crate::ast::Constant {
                name: "a00".into(), ty: Type::Custom("Float".into()), expr: float_expr(1.0),
                section: None,
            }),
            TopLevel::Constant(crate::ast::Constant {
                name: "a01".into(), ty: Type::Custom("Float".into()), expr: float_expr(0.01),
                section: None,
            }),
            TopLevel::Constant(crate::ast::Constant {
                name: "a02".into(), ty: Type::Custom("Float".into()), expr: float_expr(0.0),
                section: None,
            }),
            txn("propagate", vec![
                let_stmt("nx0", Some(Type::Custom("Float".into())),
                    add(add(mul(id("a00"), id("x0")), mul(id("a01"), id("x1"))), mul(id("a02"), id("x2")))),
                let_stmt("ap00", Some(Type::Custom("Float".into())),
                    add(add(mul(id("a00"), id("p00")), mul(id("a01"), id("p10"))), mul(id("a02"), id("p20")))),
            ]),
        ];
        let mut d = compute_densities(&items);
        let dens = d.remove("propagate").unwrap();
        assert_eq!(dens.float_idents, 2);
        assert_eq!(dens.cross_ops, 10);
        assert!(dens.per_field > 4.0, "dense txn must downgrade, got {}", dens.per_field);
    }

    /// Nbody-style sparse txn: many simple lets (dx = bx0 - bx1).
    /// Must produce per_field ≤ 4.0 (no downgrade).
    #[test]
    fn sparse_nbody_style_keeps_11() {
        let field = |name: &str| TopLevel::Statement(Box::new(Statement::Let {
            name: name.to_string(),
            names: Vec::new(),
            ty: Some(Type::Custom("Float32".into())),
            expr: Some(float_expr(0.0)),
            modifiers: Vec::new(),
        }));
        let items = vec![
            field("bx0"),
            field("bx1"),
            field("dy01"),
            txn("simulate", vec![
                let_stmt("dx01", Some(Type::Custom("Float32".into())), mul(id("bx0"), id("bx1"))),
                let_stmt("dsq01", Some(Type::Custom("Float32".into())),
                    add(mul(id("dx01"), id("dx01")), mul(id("dy01"), id("dy01")))),
            ]),
        ];        let mut d = compute_densities(&items);
        let dens = d.remove("simulate").unwrap();
        assert_eq!(dens.float_idents, 2);
        assert_eq!(dens.cross_ops, 4);
        assert!(dens.per_field <= 4.0, "sparse txn must keep #11, got {}", dens.per_field);
    }

    /// Int-only txn (mandelbrot pattern): no float lets → no downgrade.
    #[test]
    fn int_only_txn_has_zero_density() {
        let body = vec![
            let_stmt("ns1", Some(Type::Custom("Int".into())),
                add(mul(id("seed"), id("IA")), id("IC"))),
            let_stmt("nt1", Some(Type::Custom("Int".into())),
                add(mul(id("zr"), id("ncr")), id("SCALE"))),
        ];
        let items = vec![txn("mb", body)];
        let mut d = compute_densities(&items);
        let dens = d.remove("mb").unwrap();
        assert_eq!(dens.float_idents, 0);
        assert_eq!(dens.cross_ops, 0);
        assert_eq!(dens.per_field, 0.0);
    }

    /// Nested guard lets are NOT counted (kalman `when` block) — matches the
    /// backend's top-level-only scan.
    #[test]
    fn guarded_lets_are_excluded() {
        let guarded = Statement::Guarded(
            Expr::Bool(true),
            vec![let_stmt("energy", Some(Type::Custom("Float".into())),
                add(mul(id("x0"), id("x0")), mul(id("p00"), id("p00"))))],
        );
        let items = vec![txn("t", vec![guarded])];
        let mut d = compute_densities(&items);
        let dens = d.remove("t").unwrap();
        assert_eq!(dens.float_idents, 0);
        assert_eq!(dens.cross_ops, 0);
    }

    /// The `_all_idents` gap: an int-only BinaryOp operand must NOT count.
    #[test]
    fn int_ops_do_not_inflate_cross_ops() {
        // Mixed: a float let and an int let referencing a float ident in an
        // int-only context would not be a well-typed program, so test the
        // pure int side: `i = i + 1` style int lets produce 0 float ops.
        let body = vec![
            let_stmt("acc", Some(Type::Custom("Int".into())), add(id("i"), id("j"))),
        ];
        let items = vec![txn("t", body)];
        let mut d = compute_densities(&items);
        let dens = d.remove("t").unwrap();
        assert_eq!(dens.float_idents, 0);
        assert_eq!(dens.cross_ops, 0);
        assert_eq!(dens.per_field, 0.0);
    }
}
