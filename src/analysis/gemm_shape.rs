//! Structural GEMM detection (KernelPlan Phase 1b).
//!
//! 2026-09-30 (plan `2026-09-30-kernel-plan-and-per-target-lowering.md`):
//! the canonical flattened-2D matmul shape is a *frontend fact* — the
//! tiled synthesis in `backend/spirv/gemm.rs` and the PTX tensor tier are
//! two *lowerings* of it. This module owns the structural match so both
//! targets (and the `KernelPlan` builder) consume ONE decision
//! (`analysis-once`, `backend-contracts.md` §2).
//!
//! Moved verbatim from `backend/spirv/gemm.rs::GemmPlan::match_stmts` and
//! its helpers; the SPIR-V `GemmPlan` now delegates here. Field names stay
//! opaque (no Briev-type knowledge, Rule 19).

use crate::analysis::accel::KernelShape;
use crate::ast::{Expr, Statement, TopLevel};
use std::collections::HashMap;

/// Tile edge the tiled tiers require the shape to be divisible by.
const TILE: i64 = 64;

/// The recognized naive-GEMM shape, with every literal a lowering needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GemmShape {
    pub m: i64,
    pub n: i64,
    pub k: i64,
    /// State field names (a: row-major M×K, b: row-major K×N, y: M×N).
    pub a_field: String,
    pub b_field: String,
    pub y_field: String,
}

/// 2026-10-03 (declared dot rung): the GENERAL declaration check — the
/// kernel's first `let` carries `declared_composite` naming `name` (the
/// marker the composite expansion writes). `declared_matmul` is the
/// matmul instance; the cooperative-reduction channel gates on "dot".
pub fn declared_composite(shape: &KernelShape, name: &str) -> bool {
    shape
        .kernel_stmts
        .iter()
        .find_map(|s| match s {
            Statement::Let { modifiers, .. } => Some(
                modifiers.iter().any(|m| {
                    m.name == "declared_composite"
                        && matches!(&m.value, Some(Expr::Identifier(n)) if n == name)
                }),
            ),
            _ => None,
        })
        .unwrap_or(false)
}

/// 2026-10-03 (declared matmul retirement — plan
/// `2026-10-03-declared-matmul-gemmplan-retirement.md`): the DECLARATION
/// gate. `matmul!(...)` (lib/std/numeric.bv) marks its expansion's first
/// `let` with `declared_composite`; only a declared body takes the tensor
/// GEMM channel. A body with the matmul FACTS but no declaration lowers
/// through the general path — detection for advice, declaration for
/// specialization (Rules 23/24). The scan keys the FIRST LET (host
/// statements may precede the expansion — order never mattered here).
pub fn declared_matmul(shape: &KernelShape) -> bool {
    declared_composite(shape, "matmul")
}

/// Detect the canonical flattened-2D matmul in a kernel shape. `None` when
/// the body does not match the canonical form or the shape is not
/// tile-divisible (the caller falls back to the naive tier).
///
/// 2026-10-03 (declared matmul retirement — plan
/// `2026-10-03-declared-matmul-gemmplan-retirement.md`): detection is a
/// FACT derivation for ADVICE and the KernelPlan record; the
/// SPECIALIZATION route is gated by [`declared_matmul`] — the compiler
/// never recognizes a hand-written triple loop as a matmul (Rules 23/24),
/// the author declares `matmul!(...)` (lib/std/numeric.bv).
pub fn detect_gemm_shape(shape: &KernelShape, items: &[TopLevel]) -> Option<GemmShape> {
    let consts = module_const_map(items);
    let iv = shape.index_var.clone();

    let bound_v = lit(&fold_consts(shape.count_expr.as_ref()?, &consts), &consts)?;
    let lets = collect_let_table(&shape.kernel_stmts, &consts);
    let (m_name, n_name, n) = match_decomposition(&lets, &consts, &iv)?;
    let m = bound_v.checked_div(n)?;

    let (k_item, k, fbody) = match_foreach(&shape.kernel_stmts, &consts)?;
    let decomp = Decomp {
        m_name: &m_name,
        n_name: &n_name,
        k_item: &k_item,
        k,
        n,
    };
    let (acc, a_field, b_field) = match_reduction(&fbody, &decomp, &consts)?;
    let y_field = match_y_store(&shape.kernel_stmts, &iv, &acc)?;

    if m <= 0 || n <= 0 || k <= 0 {
        return None;
    }
    if m % TILE != 0 || n % TILE != 0 || k % TILE != 0 {
        return None;
    }
    Some(GemmShape {
        m,
        n,
        k,
        a_field,
        b_field,
        y_field,
    })
}

/// Const-fold identifier references into Decimal literals (moved from
/// `backend/spirv/lower.rs`; pure AST, frontend-owned).
pub fn fold_consts(e: &Expr, consts: &HashMap<String, i64>) -> Expr {
    match e {
        Expr::Identifier(n) => match consts.get(n) {
            Some(v) => Expr::Decimal(*v),
            None => e.clone(),
        },
        Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
            *k,
            Box::new(fold_consts(l, consts)),
            Box::new(fold_consts(r, consts)),
        ),
        other => other.clone(),
    }
}

/// Resolve a literal expression: Decimal, a const identifier, or a pure
/// arithmetic combination of those (`M * N` bounds, `0..K` ends).
pub fn lit(e: &Expr, consts: &HashMap<String, i64>) -> Option<i64> {
    match e {
        Expr::Decimal(d) => Some(*d),
        Expr::Identifier(name) => consts.get(name).copied(),
        Expr::BinaryOp(kind, l, r) => {
            let (a, b) = (lit(l, consts)?, lit(r, consts)?);
            match kind {
                crate::ast::BinaryOpKind::Add => Some(a.checked_add(b)?),
                crate::ast::BinaryOpKind::Sub => Some(a.checked_sub(b)?),
                crate::ast::BinaryOpKind::Mul => Some(a.checked_mul(b)?),
                crate::ast::BinaryOpKind::Div if b != 0 => Some(a.checked_div(b)?),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `mul(ident, literal)` — either operand order.
fn linear_term_of(e: &Expr, name: &str) -> Option<i64> {
    match e {
        Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, l, r) => {
            if let Expr::Identifier(n) = l.as_ref() {
                if n == name {
                    if let Some(v) = lit(r, &HashMap::new()) {
                        return Some(v);
                    }
                }
            }
            if let Expr::Identifier(n) = r.as_ref() {
                if n == name {
                    if let Some(v) = lit(l, &HashMap::new()) {
                        return Some(v);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// `a_idx` must be `m*K + k` or `k + m*K` (row-major, k coefficient 1).
fn match_a_index(e: &Expr, m: &str, k: &str) -> Option<i64> {
    match e {
        Expr::BinaryOp(crate::ast::BinaryOpKind::Add, l, r) => {
            if let Some(v) = linear_term_of(l, m) {
                if let Expr::Identifier(k1) = r.as_ref() {
                    if k1 == k {
                        return Some(v);
                    }
                }
            }
            if let Some(v) = linear_term_of(r, m) {
                if let Expr::Identifier(k1) = l.as_ref() {
                    if k1 == k {
                        return Some(v);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// `b_idx` must be `k*N + n` or `n + k*N`.
fn match_b_index(e: &Expr, k: &str, n: &str) -> Option<i64> {
    match e {
        Expr::BinaryOp(crate::ast::BinaryOpKind::Add, l, r) => {
            if let Some(v) = linear_term_of(l, k) {
                if let Expr::Identifier(n1) = r.as_ref() {
                    if n1 == n {
                        return Some(v);
                    }
                }
            }
            if let Some(v) = linear_term_of(r, k) {
                if let Expr::Identifier(n1) = l.as_ref() {
                    if n1 == n {
                        return Some(v);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Module-level `const` literals.
fn module_const_map(items: &[TopLevel]) -> HashMap<String, i64> {
    let mut consts = HashMap::new();
    for item in items {
        if let TopLevel::Constant(c) = item {
            if let Expr::Decimal(d) = &c.expr {
                consts.insert(c.name.clone(), *d);
            }
        }
    }
    consts
}

/// The node's let table (name → const-folded initializer).
fn collect_let_table(stmts: &[Statement], consts: &HashMap<String, i64>) -> HashMap<String, Expr> {
    let mut lets = HashMap::new();
    for stmt in stmts {
        if let Statement::Let {
            name,
            expr: Some(e),
            ..
        } = stmt
        {
            lets.insert(name.clone(), fold_consts(e, consts));
        }
    }
    lets
}

/// The flattened-2D decomposition: m = i / DN, n = i % DN (same divisor).
fn match_decomposition(
    lets: &HashMap<String, Expr>,
    consts: &HashMap<String, i64>,
    iv: &str,
) -> Option<(String, String, i64)> {
    let mut m_name: Option<String> = None;
    let mut n_name: Option<String> = None;
    let mut div: Option<i64> = None;
    for (name, e) in lets {
        if let Expr::BinaryOp(kind, l, r) = e {
            if !matches!(l.as_ref(), Expr::Identifier(x) if x == iv) {
                continue;
            }
            let d = lit(r, consts)?;
            match kind {
                crate::ast::BinaryOpKind::Div => {
                    m_name = Some(name.clone());
                    div = Some(d);
                }
                crate::ast::BinaryOpKind::Mod => {
                    n_name = Some(name.clone());
                }
                _ => {}
            }
        }
    }
    let n = div?;
    Some((m_name?, n_name?, n))
}

/// The reduction foreach: item name, literal trip count K, body clone.
fn match_foreach(
    stmts: &[Statement],
    consts: &HashMap<String, i64>,
) -> Option<(String, i64, Vec<Statement>)> {
    for stmt in stmts {
        if let Statement::Foreach { item, list, body } = stmt {
            if let Expr::Range { end, .. } = list.as_ref() {
                let end = fold_consts(end, consts);
                if let Some(k) = lit(&end, consts) {
                    return Some((item.clone(), k, body.clone()));
                }
            }
        }
    }
    None
}

/// The decomposition names + literal strides the reduction match needs.
struct Decomp<'a> {
    m_name: &'a str,
    n_name: &'a str,
    k_item: &'a str,
    k: i64,
    n: i64,
}

/// The reduction statement: acc = acc + A[m*K + k] * B[k*N + n].
fn match_reduction(
    fbody: &[Statement],
    d: &Decomp,
    consts: &HashMap<String, i64>,
) -> Option<(String, String, String)> {
    if fbody.len() != 1 {
        return None;
    }
    let Statement::Assign(lhs, rhs) = &fbody[0] else {
        return None;
    };
    let Expr::Identifier(acc) = lhs else {
        return None;
    };
    let Expr::BinaryOp(crate::ast::BinaryOpKind::Add, a, b) = rhs else {
        return None;
    };
    if !matches!(a.as_ref(), Expr::Identifier(x) if x == acc) {
        return None;
    }
    let Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, l, r) = b.as_ref() else {
        return None;
    };
    let a_idx_f = fold_consts(match_index_of(l.as_ref())?, consts);
    let b_idx_f = fold_consts(match_index_of(r.as_ref())?, consts);
    let a_field = match_field_of(l.as_ref())?;
    let b_field = match_field_of(r.as_ref())?;
    let a_row_stride = match_a_index(&a_idx_f, d.m_name, d.k_item)?;
    let b_col_stride = match_b_index(&b_idx_f, d.k_item, d.n_name)?;
    if a_row_stride != d.k || b_col_stride != d.n {
        return None;
    }
    Some((acc.clone(), a_field, b_field))
}

/// The store: y[i] = acc (LHS index is the BARE counter).
fn match_y_store(stmts: &[Statement], iv: &str, acc: &str) -> Option<String> {
    for stmt in stmts {
        let Statement::Assign(lhs, Expr::Identifier(v)) = stmt else {
            continue;
        };
        if v != acc {
            continue;
        }
        let Expr::Index(of, idx) = lhs else {
            continue;
        };
        if !matches!(idx.as_ref(), Expr::Identifier(x) if x == iv) {
            continue;
        }
        if let Expr::Identifier(f) = of.as_ref() {
            return Some(f.clone());
        }
    }
    None
}

fn match_index_of(e: &Expr) -> Option<&Expr> {
    match e {
        Expr::Index(_, idx) => Some(idx),
        _ => None,
    }
}

fn match_field_of(e: &Expr) -> Option<String> {
    match e {
        Expr::Index(of, _) => match of.as_ref() {
            Expr::Identifier(f) => Some(f.clone()),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_consts_substitutes_and_recurses() {
        let mut c = HashMap::new();
        c.insert("M".to_string(), 64);
        let e = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Mul,
            Box::new(Expr::Identifier("M".into())),
            Box::new(Expr::Decimal(2)),
        );
        assert_eq!(
            fold_consts(&e, &c),
            Expr::BinaryOp(
                crate::ast::BinaryOpKind::Mul,
                Box::new(Expr::Decimal(64)),
                Box::new(Expr::Decimal(2))
            )
        );
        // Unknown identifiers are left untouched.
        assert_eq!(
            fold_consts(&Expr::Identifier("z".into()), &c),
            Expr::Identifier("z".into())
        );
    }

    #[test]
    fn lit_resolves_arithmetic() {
        let mut c = HashMap::new();
        c.insert("M".to_string(), 64);
        c.insert("N".to_string(), 64);
        // M * N = 4096
        let e = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Mul,
            Box::new(Expr::Identifier("M".into())),
            Box::new(Expr::Identifier("N".into())),
        );
        assert_eq!(lit(&e, &c), Some(4096));
    }
}

/// 2026-10-03 (const-expression folding — the derive-the-counts gap, plan
/// `2026-10-03-declared-matmul-gemmplan-retirement.md`): the program's
/// compile-time constant VALUES in declaration order, each init folded
/// against the consts before it — `const MN: Int = M * N;` folds to the
/// product, chains compose. Non-foldable inits contribute nothing
/// (fail-open: the readers keep their literal-only errors for those).
/// The kernel const readers (`materialize_consts`, the SSBO dim
/// resolution) consume this so a DERIVED const is as good as a literal.
pub fn folded_const_map(items: &[TopLevel]) -> HashMap<String, i64> {
    let mut map: HashMap<String, i64> = HashMap::new();
    for item in items {
        if let TopLevel::Constant(c) = item {
            if let Some(v) = lit(&fold_consts(&c.expr, &map), &map) {
                map.insert(c.name.clone(), v);
            }
        }
    }
    map
}

#[cfg(test)]
mod folded_const_tests {
    use super::*;

    #[test]
    fn folded_const_map_folds_expressions_and_chains() {
        let items = parse_items(
            "const M: Int = 4096;\n\
             const N: Int = 4096;\n\
             const K: Int = 1024;\n\
             const MN: Int = M * N;\n\
             const MK: Int = M * K;\n\
             const DEEP: Int = MN + MK + 2;\n\
             const BAD: Int = M + missing;",
        );
        let map = folded_const_map(&items);
        assert_eq!(map["M"], 4096);
        assert_eq!(map["MN"], 4096 * 4096);
        assert_eq!(map["MK"], 4096 * 1024);
        assert_eq!(map["DEEP"], 4096 * 4096 + 4096 * 1024 + 2);
        // fail-open: an init referencing an unknown name contributes nothing
        assert!(!map.contains_key("BAD"));
    }

    fn parse_items(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("lex");
        crate::parser::Parser::new(tokens, src)
            .parse_program()
            .expect("parse")
    }
}
