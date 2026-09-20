// ── Image Storage Strategy ──────────────────────────────────────────────
//
// 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): the TEXEL is
// the primitive — an ordinary element type carrying `spec Format`. Image-
// ness is a STORAGE STRATEGY the compiler chooses (the vec4-projection
// family), never a type-level concept. This pass decides, once in the
// frontend, which accel kernels' write buffers realize as device storage
// images; the SPIR-V backend consumes the plan (binding partition +
// OpTypeImage + OpImageWrite) and the runtime materializes VkImage. No
// plan → the buffer stays a plain SSBO: every existing program unchanged.
//
// Eligibility (ALL required — silently absent, never partial):
//   1. The write buffer's element type carries a `format` spec property.
//   2. The kernel body computes (index % K, index / K) for ONE common
//      module-const K — the dimension source. Without derivable dims a
//      device image write has no coordinates, so there is no plan.
//   3. The element count divides evenly (height = count / K ≥ 1).
//   4. The config gate `spirv_image_storage` is on (opt-in until
//      measured; the coopmat precedent — promote on ledger evidence).

use crate::analysis::accel::AccelEntry;
use crate::ast::{BinaryOpKind, Dimension, Expr, PropertyValue, Statement, TopLevel, Type};
use crate::type_universe::TypeUniverse;
use std::collections::HashMap;

/// One kernel write buffer realized as a device storage image.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageStoragePlan {
    /// The state field name.
    pub array: String,
    /// Image width — the common divisor K from the kernel's index math.
    pub width: i64,
    /// Image height — element count / width.
    pub height: i64,
    /// Texel format (the element type's `spec Format` value).
    pub format: String,
}

/// Detect image-storage plans for every eligible accel kernel.
pub fn detect(
    items: &[TopLevel],
    accel: &HashMap<String, AccelEntry>,
    universe: Option<&TypeUniverse>,
    enabled: bool,
) -> HashMap<String, Vec<ImageStoragePlan>> {
    let mut plans: HashMap<String, Vec<ImageStoragePlan>> = HashMap::new();
    let (enabled, universe) = match (enabled, universe) {
        (true, Some(u)) => (true, u),
        _ => return plans,
    };
    let consts = module_consts(items);
    let arrays = state_arrays(items);
    for (txn, entry) in accel {
        if !entry.shape.eligible {
            continue;
        }
        let mut txn_plans = Vec::new();
        for array in &entry.shape.write_buffers {
            if let Some(plan) = plan_for(array, entry, &arrays, &consts, universe) {
                txn_plans.push(plan);
            }
        }
        if !txn_plans.is_empty() {
            plans.insert(txn.clone(), txn_plans);
        }
    }
    plans
}

/// The plan for one write buffer, or None when any eligibility clause fails.
fn plan_for(
    array: &str,
    entry: &AccelEntry,
    arrays: &HashMap<String, (Type, i64)>,
    consts: &HashMap<String, i64>,
    universe: &TypeUniverse,
) -> Option<ImageStoragePlan> {
    let (elem, count) = arrays.get(array)?;
    let count = *count;
    let format = element_format(elem, universe)?;
    let width = common_index_divisor(&entry.shape.kernel_stmts, &entry.shape.index_var, consts)?;
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
    stmts: &[Statement],
    index_var: &str,
    consts: &HashMap<String, i64>,
) -> Option<i64> {
    let mut mods: Vec<i64> = Vec::new();
    let mut divs: Vec<i64> = Vec::new();
    for stmt in stmts {
        collect_index_divisors_stmt(stmt, index_var, consts, &mut mods, &mut divs);
    }
    let shared: Vec<i64> = mods
        .iter()
        .filter(|k| divs.contains(k))
        .copied()
        .collect();
    let first = *shared.first()?;
    (shared.iter().all(|k| *k == first)).then_some(first)
}

fn collect_index_divisors_stmt(
    stmt: &Statement,
    index_var: &str,
    consts: &HashMap<String, i64>,
    mods: &mut Vec<i64>,
    divs: &mut Vec<i64>,
) {
    match stmt {
        Statement::Let { expr: Some(e), .. } | Statement::Assign(_, e) => {
            collect_index_divisors_expr(e, index_var, consts, mods, divs);
        }
        Statement::Foreach { list, body, .. } => {
            collect_index_divisors_expr(list, index_var, consts, mods, divs);
            for s in body {
                collect_index_divisors_stmt(s, index_var, consts, mods, divs);
            }
        }
        Statement::Block(body) => {
            for s in body {
                collect_index_divisors_stmt(s, index_var, consts, mods, divs);
            }
        }
        Statement::Guarded(_, body) => {
            for s in body {
                collect_index_divisors_stmt(s, index_var, consts, mods, divs);
            }
        }
        _ => {}
    }
}

fn collect_index_divisors_expr(
    e: &Expr,
    index_var: &str,
    consts: &HashMap<String, i64>,
    mods: &mut Vec<i64>,
    divs: &mut Vec<i64>,
) {
    match e {
        Expr::BinaryOp(kind @ (BinaryOpKind::Mod | BinaryOpKind::Div), lhs, rhs) => {
            if matches!(lhs.as_ref(), Expr::Identifier(v) if v == index_var) {
                if let Some(k) = eval_const(rhs, consts) {
                    match kind {
                        BinaryOpKind::Mod => mods.push(k),
                        _ => divs.push(k),
                    }
                }
            }
            collect_index_divisors_expr(lhs, index_var, consts, mods, divs);
            collect_index_divisors_expr(rhs, index_var, consts, mods, divs);
        }
        Expr::BinaryOp(_, l, r) => {
            collect_index_divisors_expr(l, index_var, consts, mods, divs);
            collect_index_divisors_expr(r, index_var, consts, mods, divs);
        }
        Expr::UnaryOp(_, inner) => {
            collect_index_divisors_expr(inner, index_var, consts, mods, divs);
        }
        Expr::Index(base, idx) => {
            collect_index_divisors_expr(base, index_var, consts, mods, divs);
            collect_index_divisors_expr(idx, index_var, consts, mods, divs);
        }
        Expr::Call(_, args, _) => {
            for a in args {
                collect_index_divisors_expr(a, index_var, consts, mods, divs);
            }
        }
        _ => {}
    }
}

fn eval_const(e: &Expr, consts: &HashMap<String, i64>) -> Option<i64> {
    match e {
        Expr::Decimal(n) => Some(*n),
        Expr::Identifier(name) => consts.get(name).copied(),
        _ => None,
    }
}

/// Module constants (const NAME: T = literal;) — the dimension source.
fn module_consts(items: &[TopLevel]) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    for item in items {
        if let TopLevel::Constant(c) = item {
            if let Expr::Decimal(n) = &c.expr {
                out.insert(c.name.clone(), *n);
            }
        }
    }
    out
}

/// Array state fields → (element type, element count). Handles BOTH
/// top-level forms — the parser emits `let x: T[N];` as Statement(Let),
/// hand-built fixtures as StateDecl (the same dual form
/// collect_state_fields handles; missing an arm silently drops the
/// field from every plan).
fn state_arrays(items: &[TopLevel]) -> HashMap<String, (Type, i64)> {
    let mut out = HashMap::new();
    for item in items {
        let (name, ty): (String, Type) = match item {
            TopLevel::StateDecl(sd) => (sd.name.clone(), sd.ty.clone()),
            TopLevel::Statement(stmt) => match stmt.as_ref() {
                Statement::Let {
                    name,
                    ty: Some(ty),
                    ..
                } => (name.clone(), ty.clone()),
                _ => continue,
            },
            _ => continue,
        };
        if let Type::Vector(elem, dims) = ty {
            if let Some(Dimension::Anonymous(n)) = dims.first() {
                out.insert(name, ((*elem).clone(), *n as i64));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::top::StateDecl;
    use crate::ast::PropertyValue;

    fn r32_decl(count: i64) -> TopLevel {
        TopLevel::StateDecl(StateDecl {
            name: "img".into(),
            ty: Type::Vector(
                Box::new(Type::Custom("R32".into())),
                vec![Dimension::Anonymous(count as usize)],
            ),
            span: None,
        })
    }

    fn fixture() -> (Vec<TopLevel>, HashMap<String, AccelEntry>, TypeUniverse) {
        let items = vec![
            TopLevel::Constant(crate::ast::top::Constant {
                name: "W".into(),
                ty: Type::int(),
                expr: Expr::Decimal(64),
                section: None,
            }),
            TopLevel::StateDecl(StateDecl {
                name: "i".into(),
                ty: Type::int(),
                span: None,
            }),
            r32_decl(4096),
        ];
        // kernel_stmts: let pix: Int = i % W; let row: Int = i / W; i = i + 1;
        let shape = crate::analysis::accel::KernelShape {
            index_var: "i".into(),
            count_expr: Some(Expr::Decimal(4096)),
            kernel_stmts: vec![
                Statement::Let {
                    name: "pix".into(),
                    names: vec![],
                    ty: Some(Type::int()),
                    expr: Some(Expr::BinaryOp(
                        BinaryOpKind::Mod,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Identifier("W".into())),
                    )),
                    modifiers: vec![],
                },
                Statement::Let {
                    name: "row".into(),
                    names: vec![],
                    ty: Some(Type::int()),
                    expr: Some(Expr::BinaryOp(
                        BinaryOpKind::Div,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Identifier("W".into())),
                    )),
                    modifiers: vec![],
                },
                Statement::Assign(
                    Expr::Identifier("i".into()),
                    Expr::BinaryOp(
                        BinaryOpKind::Add,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(1)),
                    ),
                ),
            ],
            host_stmts: vec![],
            read_buffers: vec![],
            write_buffers: vec!["img".into()],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: None,
            deferred_normalize: None,
        };
        let entry = AccelEntry {
            mode: crate::analysis::accel::AccelMode::TryKeyword,
            forced: false,
            shape,
            decision: crate::analysis::accel::AccelDecision::Gpu,
        };
        let mut accel = HashMap::new();
        accel.insert("fill".into(), entry);

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
        (items, accel, universe)
    }

    #[test]
    fn detects_image_plan_from_index_math() {
        let (items, accel, universe) = fixture();
        let plans = detect(&items, &accel, Some(&universe), true);
        let plan = &plans["fill"][0];
        assert_eq!(plan.array, "img");
        assert_eq!(plan.width, 64);
        assert_eq!(plan.height, 64);
        assert_eq!(plan.format, "R32Float");
    }

    #[test]
    fn disabled_gate_yields_no_plans() {
        let (items, accel, universe) = fixture();
        assert!(detect(&items, &accel, Some(&universe), false).is_empty());
    }

    #[test]
    fn formatless_element_yields_no_plan() {
        let (mut items, accel, universe) = fixture();
        // Replace the R32 array with a plain Float array — no format property.
        items[2] = TopLevel::StateDecl(StateDecl {
            name: "img".into(),
            ty: Type::Vector(
                Box::new(Type::float()),
                vec![Dimension::Anonymous(4096)],
            ),
            span: None,
        });
        assert!(detect(&items, &accel, Some(&universe), true).is_empty());
    }

}
