// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-11 (H5 port): image-storage strategy over typed HIR.
//!
//! Mirrors `analysis::image_storage`: the TEXEL is the primitive and
//! image-ness is a storage strategy the compiler chooses. Detection reads
//! state arrays and module consts from `HirProgram`, and recognizes the
//! index-math dimension source over HIR kernel statements. The public
//! `ImageStoragePlan` result is AST-free, so no result-boundary conversion is
//! needed.
//!
//! Eligibility (ALL required — silently absent, never partial):
//!   1. The write buffer's element type carries a `format` spec property.
//!   2. The kernel body computes (index % K, index / K) for ONE common
//!      module-const K — the dimension source.
//!   3. The element count divides evenly (height = count / K ≥ 1).
//!   4. The config gate `spirv_image_storage` is on.
//!
//! Complexity exception (house limits): `detect` scans accel entries ×
//! their write buffers, an O(kernels × buffers) nested walk that mirrors the
//! AST oracle; both counts are program-declared surfaces, not input size.

use crate::analysis::accel::AccelEntry;
use crate::analysis::image_storage::ImageStoragePlan;
use crate::ast::{BinaryOpKind, Dimension, PropertyValue, Type};
use crate::hir::expr::{HirExpr, HirExprKind, HirLiteral};
use crate::hir::program::HirProgram;
use crate::hir::stmt::{HirStmt, HirStmtKind};
use crate::type_universe::TypeUniverse;
use std::collections::HashMap;

/// Detect image-storage plans for every eligible HIR accel kernel.
pub fn detect(
    program: &HirProgram,
    accel: &HashMap<String, AccelEntry>,
    universe: Option<&TypeUniverse>,
    enabled: bool,
) -> HashMap<String, Vec<ImageStoragePlan>> {
    let mut plans: HashMap<String, Vec<ImageStoragePlan>> = HashMap::new();
    let (enabled, universe) = match (enabled, universe) {
        (true, Some(u)) => (true, u),
        _ => return plans,
    };
    let ctx = PlanCtx {
        arrays: state_arrays(program),
        consts: module_consts(program),
        universe,
    };
    let mut names: Vec<&String> = accel.keys().collect();
    names.sort();
    for txn in names {
        let entry = &accel[txn];
        if !entry.shape.eligible {
            continue;
        }
        let Some(callable) = program.callables.iter().find(|c| c.name == *txn) else {
            continue;
        };
        let kernel = super::accel::kernel_statements(program, callable, &entry.shape.index_var);
        let txn_plans = plans_for(
            &entry.shape.write_buffers,
            &kernel,
            &entry.shape.index_var,
            &ctx,
        );
        if !txn_plans.is_empty() {
            plans.insert(txn.clone(), txn_plans);
        }
    }
    plans
}

struct PlanCtx<'a> {
    arrays: HashMap<String, (Type, i64)>,
    consts: HashMap<String, i64>,
    universe: &'a TypeUniverse,
}

fn plans_for(
    buffers: &[String],
    kernel: &[HirStmt],
    index_var: &str,
    ctx: &PlanCtx<'_>,
) -> Vec<ImageStoragePlan> {
    buffers
        .iter()
        .filter_map(|array| plan_for(array, kernel, index_var, ctx))
        .collect()
}

/// The plan for one write buffer, or None when any eligibility clause fails.
fn plan_for(
    array: &str,
    kernel: &[HirStmt],
    index_var: &str,
    ctx: &PlanCtx<'_>,
) -> Option<ImageStoragePlan> {
    let (elem, count) = ctx.arrays.get(array)?;
    let count = *count;
    let format = element_format(elem, ctx.universe)?;
    let width = common_index_divisor(kernel, index_var, &ctx.consts)?;
    if width < 1 || count < width || count % width != 0 {
        return None;
    }
    Some(ImageStoragePlan {
        array: array.to_string(),
        width,
        height: count / width,
        format,
    })
}

/// The element type's `format` spec value, if any.
fn element_format(elem: &Type, universe: &TypeUniverse) -> Option<String> {
    let key = elem.universe_key()?;
    let rt = universe.get(key)?;
    match rt.properties.get("format")? {
        PropertyValue::Identifier(name) => Some(name.clone()),
        _ => None,
    }
}

/// The common divisor K in (index % K, index / K) — the dimension source.
/// Requires exactly one candidate shared by BOTH forms.
fn common_index_divisor(
    stmts: &[HirStmt],
    index_var: &str,
    consts: &HashMap<String, i64>,
) -> Option<i64> {
    let mut mods: Vec<i64> = Vec::new();
    let mut divs: Vec<i64> = Vec::new();
    for stmt in stmts {
        collect_index_divisors_stmt(stmt, index_var, consts, &mut mods, &mut divs);
    }
    let shared: Vec<i64> = mods.iter().filter(|k| divs.contains(k)).copied().collect();
    let first = *shared.first()?;
    (shared.iter().all(|k| *k == first)).then_some(first)
}

fn collect_index_divisors_stmt(
    stmt: &HirStmt,
    index_var: &str,
    consts: &HashMap<String, i64>,
    mods: &mut Vec<i64>,
    divs: &mut Vec<i64>,
) {
    match &stmt.kind {
        HirStmtKind::Let { expr: Some(e), .. } | HirStmtKind::Assign(_, e) => {
            collect_index_divisors_expr(e, index_var, consts, mods, divs);
        }
        HirStmtKind::Foreach { list, body, .. } => {
            collect_index_divisors_expr(list, index_var, consts, mods, divs);
            for s in body {
                collect_index_divisors_stmt(s, index_var, consts, mods, divs);
            }
        }
        HirStmtKind::Block(body) | HirStmtKind::Guarded(_, body) => {
            for s in body {
                collect_index_divisors_stmt(s, index_var, consts, mods, divs);
            }
        }
        _ => {}
    }
}

fn collect_index_divisors_expr(
    e: &HirExpr,
    index_var: &str,
    consts: &HashMap<String, i64>,
    mods: &mut Vec<i64>,
    divs: &mut Vec<i64>,
) {
    match &e.kind {
        HirExprKind::Binary {
            op: kind @ (BinaryOpKind::Mod | BinaryOpKind::Div),
            left,
            right,
        } => {
            if matches!(&left.kind, HirExprKind::Identifier(v) if v == index_var) {
                if let Some(k) = eval_const(right, consts) {
                    match kind {
                        BinaryOpKind::Mod => mods.push(k),
                        _ => divs.push(k),
                    }
                }
            }
            collect_index_divisors_expr(left, index_var, consts, mods, divs);
            collect_index_divisors_expr(right, index_var, consts, mods, divs);
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_index_divisors_expr(left, index_var, consts, mods, divs);
            collect_index_divisors_expr(right, index_var, consts, mods, divs);
        }
        HirExprKind::Unary { inner, .. } => {
            collect_index_divisors_expr(inner, index_var, consts, mods, divs);
        }
        HirExprKind::Index(base, idx) => {
            collect_index_divisors_expr(base, index_var, consts, mods, divs);
            collect_index_divisors_expr(idx, index_var, consts, mods, divs);
        }
        HirExprKind::Call { args, .. } => {
            for a in args {
                collect_index_divisors_expr(a, index_var, consts, mods, divs);
            }
        }
        _ => {}
    }
}

fn eval_const(e: &HirExpr, consts: &HashMap<String, i64>) -> Option<i64> {
    match &e.kind {
        HirExprKind::Literal(HirLiteral::Int(n)) => Some(*n),
        HirExprKind::Identifier(name) => consts.get(name).copied(),
        _ => None,
    }
}

/// Module constants (const NAME: T = literal;) — the dimension source.
fn module_consts(program: &HirProgram) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    for (name, expr) in &program.const_exprs {
        if let HirExprKind::Literal(HirLiteral::Int(n)) = expr.kind {
            out.insert(name.clone(), n);
        }
    }
    out
}

/// State array globals → (element type, element count). Both the `state`
/// declaration and a top-level `let x: T[N];` lower to globals; the type's
/// anonymous first dimension is the count.
fn state_arrays(program: &HirProgram) -> HashMap<String, (Type, i64)> {
    let mut out = HashMap::new();
    for global in &program.globals {
        let Some(ty) = &global.ty else {
            continue;
        };
        if let Type::Vector(elem, dims) = ty {
            if let Some(Dimension::Anonymous(n)) = dims.first() {
                out.insert(global.name.clone(), ((**elem).clone(), *n as i64));
            }
        }
    }
    out
}

/// Test-only HIR dump: detect, then canonicalise by transaction name.
#[cfg(test)]
pub(crate) fn plans_for_test(
    program: &HirProgram,
    accel: &HashMap<String, AccelEntry>,
    universe: Option<&TypeUniverse>,
    enabled: bool,
) -> std::collections::BTreeMap<String, String> {
    crate::analysis::image_storage::canonical_plans(&detect(program, accel, universe, enabled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::accel::AccelMode;
    use crate::ast::TopLevel;
    use crate::hir::program::HirProgram;
    use std::collections::BTreeMap;

    fn parse_lower(src: &str) -> (Vec<TopLevel>, HirProgram) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let program = crate::hir::lower_program(&items);
        (items, program)
    }

    fn r32_universe() -> TypeUniverse {
        let mut universe = TypeUniverse::new();
        let mut props = HashMap::new();
        props.insert(
            "format".to_string(),
            PropertyValue::Identifier("R32Float".into()),
        );
        universe.register(crate::type_universe::ResolvedType {
            type_params: vec![],
            name: "R32".into(),
            base: "Float".into(),
            bytes: 4,
            min_bits: 32,
            max_bits: 32,
            alignment: 4,
            properties: props,
            fields: vec![],
        });
        universe
    }

    fn parity(src: &str, enabled: bool) -> BTreeMap<String, String> {
        let (items, program) = parse_lower(src);
        let universe = r32_universe();
        let ast_accel =
            crate::analysis::accel::analyze(&items, Some(&universe), Some(AccelMode::TryAll));
        let hir_accel =
            crate::hir::accel::analyze(&program, Some(&universe), Some(AccelMode::TryAll));
        let ast =
            crate::analysis::image_storage::detect(&items, &ast_accel, Some(&universe), enabled);
        let ast = crate::analysis::image_storage::canonical_plans(&ast);
        let hir = plans_for_test(&program, &hir_accel, Some(&universe), enabled);
        assert_eq!(hir, ast, "HIR/AST image-storage drift:\n{src}");
        ast
    }

    const FILL_SRC: &str = r#"
const W: Int = 64;
type R32: Float { spec Format: R32Float; };
let img: R32[4096];
let i: Int = 0;
accel node fill [i < 4096][i == 4096] {
    let pix: Int = i % W;
    let row: Int = i / W;
    img[i] = pix + row;
    i = i + 1;
    term;
};
"#;

    #[test]
    fn detects_image_plan_from_index_math() {
        let plans = parity(FILL_SRC, true);
        assert_eq!(plans["fill"], "img:64x64:R32Float");
    }

    #[test]
    fn disabled_gate_yields_no_plans() {
        assert!(parity(FILL_SRC, false).is_empty());
    }

    #[test]
    fn formatless_element_yields_no_plan() {
        let src = r#"
const W: Int = 64;
let img: Float[4096];
let i: Int = 0;
accel node fill [i < 4096][i == 4096] {
    let pix: Int = i % W;
    let row: Int = i / W;
    img[i] = pix + row;
    i = i + 1;
    term;
};
"#;
        assert!(parity(src, true).is_empty());
    }

    #[test]
    fn missing_divisor_yields_no_plan() {
        let src = r#"
type R32: Float { spec Format: R32Float; };
let img: R32[4096];
let i: Int = 0;
accel node fill [i < 4096][i == 4096] {
    img[i] = 1.0;
    i = i + 1;
    term;
};
"#;
        assert!(parity(src, true).is_empty());
    }

    #[test]
    fn non_dividing_count_yields_no_plan() {
        let src = r#"
const W: Int = 64;
type R32: Float { spec Format: R32Float; };
let img: R32[4000];
let i: Int = 0;
accel node fill [i < 4000][i == 4000] {
    let pix: Int = i % W;
    let row: Int = i / W;
    img[i] = pix + row;
    i = i + 1;
    term;
};
"#;
        assert!(parity(src, true).is_empty());
    }

    #[test]
    fn multiple_kernels_are_keyed_by_name() {
        let src = r#"
const W: Int = 64;
type R32: Float { spec Format: R32Float; };
let a: R32[4096];
let b: R32[1024];
let i: Int = 0;
accel node fill_a [i < 4096][i == 4096] {
    let pix: Int = i % W;
    let row: Int = i / W;
    a[i] = pix + row;
    i = i + 1;
    term;
};
accel node fill_b [i < 1024][i == 1024] {
    let pix: Int = i % W;
    let row: Int = i / W;
    b[i] = pix + row;
    i = i + 1;
    term;
};
"#;
        let plans = parity(src, true);
        assert_eq!(plans.len(), 2);
        assert_eq!(plans["fill_a"], "a:64x64:R32Float");
        assert_eq!(plans["fill_b"], "b:64x16:R32Float");
    }

    #[test]
    fn empty_program_has_no_plans() {
        assert!(parity("", true).is_empty());
    }
}
