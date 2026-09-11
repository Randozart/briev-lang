// ── CIRCT Normalizer — AST Annotation Pass ────────────────────────────
// 2026-07-14: Strips primitive metadata (CIRCT uses bytes only),
// attaches bit_width, rejects runtime-dependent intrinsics.
// Follows the same pattern as llvm/normalizer.rs.
// 2026-08-10: user TypeDefs registered via the shared
// backend::register_types::register_typedefs before bit_width derivation.

use std::collections::HashSet;
use crate::ast::*;
use crate::backend::normalizer;
use crate::backend::register_types::register_typedefs;
use crate::type_universe::TypeUniverse;

/// 2026-07-14: Normalize the AST for CIRCT hardware backend emission.
/// CIRCT doesn't use primitive — it uses bytes to determine bit width.
/// Rejects intrinsics that require runtime OS support (Print#, Malloc#, etc.).
pub fn normalize(items: &mut Vec<TopLevel>, universe: &mut TypeUniverse, int_bits: u64) -> Result<(), String> {
    // 2026-08-10: shared type registration — CIRCT needs the same universe
    // population every backend gets (semantic goal, not just LLVM's).
    register_typedefs(items, universe, int_bits)?;

    // Attach bit_width to every type
    for rt in universe.types.values_mut() {
        let bits = rt.bytes * 8;
        rt.properties.insert("bit_width".into(), PropertyValue::Int(bits as i64));
    }

    // Reject runtime-dependent intrinsics
    let supported = build_supported_ops();
    let errors = normalizer::validate_intrinsics(items, &supported);
    if !errors.is_empty() {
        return Err(format!("CIRCT normalizer:\n  {}", errors.join("\n  ")));
    }

    // Strip everything except what CIRCT needs.
    // 2026-08-23 (Plan 3.1 bugfix): KEEP the protocol-category properties
    // (Cast.Int / Cast.UInt / Cast.Float / …) — mlir_type derives MLIR
    // signedness (siN/uN/fN) from them. The old keep-set stripped them, so
    // every pipeline-built module rendered plain i64 regardless of the
    // declared type. To undo: restore the three-key keep list.
    let keep_exact: HashSet<String> = ["bit_width", "hardware", "alignment"]
        .iter().map(|s| s.to_string()).collect();
    for rt in universe.types.values_mut() {
        rt.properties.retain(|k, _| keep_exact.contains(k) || k.starts_with("Cast."));
    }

    Ok(())
}

/// CIRCT supported intrinsics — hardware subset only.
fn build_supported_ops() -> HashSet<String> {
    let mut set = HashSet::new();
    for op in &[
        "Add#", "Sub#", "Mul#", "Div#", "Rem#",
        "Eq#", "Neq#", "Lt#", "Gt#", "Le#", "Ge#",
        "Neg#", "Abs#",
        "GetGlobalId#", "GetGlobalSize#", "GetLocalId#",
    ] {
        set.insert(op.to_string());
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::top::{TypeDef, TypeDefBody, TypeDefSlot};
    use std::collections::HashMap;

    fn make_type_def(name: &str, slots: Vec<(&str, Type)>) -> TopLevel {
        TopLevel::TypeDef(Box::new(TypeDef {
            name: name.to_string(),
            type_params: vec![],
            parent: None,
            protocol: Some("Bit".to_string()),
            traits: vec![],
            bit_range: None,
            coll: false,
            ports_in: Vec::new(),
            ports_out: Vec::new(),
            seq: false,
            body: TypeDefBody {
                pins: Vec::new(),
                slots: slots.into_iter().map(|(n, ty)| TypeDefSlot {
                    name: n.to_string(), ty, bit_range: None,
                }).collect(),
                metadata: HashMap::new(),
                projections: vec![],
                bindings: vec![],
                operators: vec![],
                op_bindings: vec![],
                constraints: vec![],
                members: vec![],
                span: None,
            },
            span: None,
        }))
    }

    #[test]
    fn test_circt_register_typedefs() {
        let mut u = TypeUniverse::new();
        let items = vec![make_type_def("Point", vec![("x", Type::int()), ("y", Type::int())])];
        normalize(&mut items.clone(), &mut u, 32).unwrap();
        let rt = u.get("Point").expect("typedef registered");
        assert_eq!(rt.bytes, 16);
        assert_eq!(u.get("Point").unwrap().properties.get("bit_width"), Some(&PropertyValue::Int(128)));
    }
}
