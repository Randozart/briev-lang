// ── SoA Reorder Projection — the second compiler-in-Briev handoff ──────
// 2026-08-04 (plan 2026-08-04-compiler-in-briev-dogfood-ffi, P5): serializes
// the input to `soa_reorder::reorder_fields` into the permutation-only form
// the Briev pass (lib/compiler/soa_reorder.bv) reads. Rust walks the AST and
// emits, per reorderable float field, its SIBLING-REFERENCE count (how many
// same-prefix fields its update expr / the txn body's assigns reference);
// Briev decides which prefix groups are SAFE (≥2 members, no sibling refs)
// and builds the item-index permutation. This is a DIFFERENT handoff shape
// than needs_state (field descriptors + a permutation buffer vs a bitmask) —
// proving the pattern generalizes.
//
// Projection format:
//   soa 1
//   total <N>
//   nfields <k>
//   prefixes <p> <prefix> <maxidx> ...     # distinct prefixes, Rust-sorted
//   field <name> <prefix> <index> <itemidx> <refcnt>
//   ...                                    # declaration order
//   nonfloat <m> <itemidx> ...
//
// Undo: if the Briev pass is ever removed, delete this module and the
// reorder_fields_briev dispatch in soa_reorder.rs.

use crate::ast::{Expr, Statement, TopLevel};
use std::collections::HashMap;

use super::soa_reorder::{find_txn_body, FloatField, try_extract_float_field};

/// Collect every identifier name referenced by an expression (recursing all
/// wrapping forms) — the projection's sibling-reference source.
pub(crate) fn collect_ref_names(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Identifier(name) => out.push(name.clone()),
        Expr::Call(_, args, _) => { for a in args { collect_ref_names(a, out); } }
        Expr::BinaryOp(_, l, r) => { collect_ref_names(l, out); collect_ref_names(r, out); }
        Expr::UnaryOp(_, e) => collect_ref_names(e, out),
        Expr::Field(recv, _) => collect_ref_names(recv, out),
        Expr::MethodCall(recv, _, args, _) => {
            collect_ref_names(recv, out);
            for a in args { collect_ref_names(a, out); }
        }
        Expr::Reflect(recv, _, _) => collect_ref_names(recv, out),
        Expr::Index(arr, idx) => { collect_ref_names(arr, out); collect_ref_names(idx, out); }
        Expr::Slice { array, start, end, .. } => {
            collect_ref_names(array, out);
            if let Some(s) = start { collect_ref_names(s, out); }
            if let Some(e) = end { collect_ref_names(e, out); }
        }
        Expr::List(items) => { for e in items { collect_ref_names(e, out); } }
        Expr::Cast(inner, _) => collect_ref_names(inner, out),
        Expr::AddrOf(inner) => collect_ref_names(inner, out),
        _ => {}
    }
}

/// Serialize the soa_reorder projection for a program.
pub fn serialize_soa_projection(items: &[TopLevel]) -> String {
    let (fields, non_float) = collect_fields_and_nonfloat(items);
    let name_prefix: HashMap<&str, &str> = fields.iter()
        .map(|f| (f.name.as_str(), f.prefix.as_str()))
        .collect();
    let txn_body = find_txn_body(items);
    let refcnt = sibling_refcnts(&fields, &name_prefix, &txn_body);

    // Distinct prefixes (Rust-sorted for deterministic output) + max index.
    let mut prefix_max: HashMap<&str, usize> = HashMap::new();
    for f in &fields {
        let entry = prefix_max.entry(f.prefix.as_str()).or_insert(f.index);
        *entry = (*entry).max(f.index);
    }
    let mut prefixes: Vec<&str> = prefix_max.keys().copied().collect();
    prefixes.sort();

    emit_soa_projection(&fields, &non_float, &prefix_max, &prefixes, &refcnt)
}

/// Split the items into reorderable float fields and everything else.
fn collect_fields_and_nonfloat(
    items: &[TopLevel],
) -> (Vec<FloatField>, Vec<usize>) {
    let mut fields = Vec::new();
    let mut non_float = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if let Some(f) = try_extract_float_field(item, i) {
            fields.push(f);
        } else {
            non_float.push(i);
        }
    }
    (fields, non_float)
}

/// Per-field sibling-reference count: refs from the field's update expr plus
/// refs from txn-body assigns TARGETING this field, counted only when the
/// referenced name is another float field with the same prefix.
fn sibling_refcnts(
    fields: &[FloatField],
    name_prefix: &HashMap<&str, &str>,
    txn_body: &[Statement],
) -> Vec<usize> {
    fields.iter().map(|f| {
        let mut refs: Vec<String> = Vec::new();
        if let Some(e) = &f.expr { collect_ref_names(e, &mut refs); }
        for stmt in txn_body {
            if let Statement::Assign(lhs, rhs) = stmt {
                if let Expr::Identifier(n) = lhs {
                    if n == &f.name { collect_ref_names(rhs, &mut refs); }
                }
            }
        }
        refs.iter().filter(|r| {
            r.as_str() != f.name.as_str()
                && name_prefix.get(r.as_str()).copied() == Some(f.prefix.as_str())
        }).count()
    }).collect()
}

/// Emit the projection sections.
fn emit_soa_projection(
    fields: &[FloatField],
    non_float: &[usize],
    prefix_max: &HashMap<&str, usize>,
    prefixes: &[&str],
    refcnt: &[usize],
) -> String {
    let mut out = String::new();
    out.push_str("soa 1\n");
    out.push_str(&format!("total {}\n", fields.len() + non_float.len()));
    out.push_str(&format!("nfields {}\n", fields.len()));
    out.push_str(&format!("prefixes {}", prefixes.len()));
    for p in prefixes {
        out.push_str(&format!(" {} {}", p, prefix_max[p]));
    }
    out.push('\n');
    for (i, f) in fields.iter().enumerate() {
        out.push_str(&format!(
            "field {} {} {} {} {}\n",
            f.name, f.prefix, f.index, f.item_index, refcnt[i]
        ));
    }
    out.push_str(&format!("nonfloat {}", non_float.len()));
    for idx in non_float {
        out.push_str(&format!(" {}", idx));
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_let(name: &str, prefix: &str, index: usize, itemidx: usize, expr: Expr) -> TopLevel {
        // Build a `let name: Float = expr` top-level statement.
        let stmt = Statement::Let {
            names: vec![],
            name: name.to_string(),
            ty: Some(crate::ast::Type::float()),
            expr: Some(expr),
            modifiers: vec![],
        };
        let _ = prefix;
        let _ = index;
        let _ = itemidx;
        TopLevel::Statement(Box::new(stmt))
    }

    #[test]
    fn projection_lists_fields_and_refcnts() {
        let bx0 = state_let("bx0", "bx", 0, 0, Expr::Decimal(1));
        let by0 = state_let("by0", "by", 0, 1, Expr::Decimal(2));
        let bx1 = state_let(
            "bx1", "bx", 1, 2,
            Expr::BinaryOp(crate::ast::BinaryOpKind::Add,
                Box::new(Expr::Identifier("bx0".to_string())),
                Box::new(Expr::Decimal(1))),
        );
        let items = vec![bx0, by0, bx1];
        let proj = serialize_soa_projection(&items);
        assert!(proj.contains("total 3"), "total: {}", proj);
        assert!(proj.contains("prefixes 2 bx 1 by 0"), "prefixes: {}", proj);
        // bx1 references bx0 (a sibling) → refcnt 1; bx0/by0 refcnt 0.
        assert!(proj.contains("field bx1 bx 1 2 1"), "bx1 refcnt: {}", proj);
        assert!(proj.contains("field bx0 bx 0 0 0"), "bx0 refcnt: {}", proj);
        assert!(proj.contains("nonfloat 0"), "nonfloat: {}", proj);
    }

    #[test]
    fn projection_marks_non_float_items() {
        let bx0 = state_let("bx0", "bx", 0, 0, Expr::Decimal(1));
        let count = TopLevel::Constant(crate::ast::Constant {
            name: "count".to_string(),
            ty: crate::ast::Type::int(),
            expr: Expr::Decimal(0),
            section: None,
        });
        let items = vec![bx0, count];
        let proj = serialize_soa_projection(&items);
        assert!(proj.contains("nonfloat 1 1"), "nonfloat: {}", proj);
    }
}
