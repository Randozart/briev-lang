// ── SPIR-V Normalizer — AST Annotation Pass ──────────────────────────
// 2026-07-15: Resolves types via TypeUniverse, flags kernels, validates
// against SPIR-V supported ops.
//
// 2026-07-20: Simplified for hashword protocol. ALU metadata removed.
// Op validation uses hardcoded standard ops instead of TOML config.
// 2026-08-10: user TypeDefs registered via the shared
// backend::register_types::register_typedefs before validation.

use crate::ast::*;
use crate::backend::normalizer;
use crate::backend::register_types::register_typedefs;
use crate::type_universe::TypeUniverse;
use std::collections::HashSet;

/// 2026-07-15: Normalize AST for SPIR-V backend emission.
/// 2026-07-20: No TOML config — hashword protocol replaces op dispatch.
pub fn normalize(items: &mut Vec<TopLevel>, universe: &mut TypeUniverse, int_bits: u64) -> Result<(), String> {
    // 2026-08-10: shared type registration — uniform universe population.
    register_typedefs(items, universe, int_bits)?;

    // 2026-07-15: Flag kernel transactions ([idx < N] pattern)
    for item in items.iter_mut() {
        if let TopLevel::Transaction(txn) = item {
            if is_kernel_txn(txn) {
                txn.metadata.insert("is_kernel".into(), PropertyValue::Bool(true));
            }
        }
    }

    // 2026-07-15: Validate operators against standard op set
    let supported = build_supported_ops();
    let errors = normalizer::validate_intrinsics(items, &supported);
    if !errors.is_empty() {
        return Err(format!("SPIR-V normalizer:\n  {}", errors.join("\n  ")));
    }

    // 2026-08-31 (plan abv-gpu-by-default): the universe-property STRIP that
    // lived here (keep-set {is_kernel, disamb, op.*}) is REMOVED — it deleted
    // the `bits`/protocol-category keys the accel eligibility proof
    // (protocol_category) and the casting graph's resolve_spirv_shape read,
    // so every real-pipeline .abv failed flatness with "not a flat scalar
    // type" while fresh-universe unit tests passed. Universe properties are
    // shared frontend state, not backend-local metadata.
    Ok(())
}

fn is_kernel_txn(txn: &crate::ast::top::Transaction) -> bool {
    // 2026-07-15: Kernel if contract has an index-sized precondition
    let pre = &txn.contract.pre_condition;
    let s = format!("{}", pre);
    s.contains("idx") || s.contains("Index")
}

/// Build the set of supported intrinsic names.
fn build_supported_ops() -> HashSet<String> {
    let mut set = HashSet::new();
    for name in &["Add#", "Sub#", "Mul#", "Div#", "Eq#", "Lt#", "Gt#",
                   "BitAnd#", "BitOr#", "BitXor#", "Shl#", "Shr#",
                   "Malloc#", "Free#", "Print#", "Exp#", "Sqrt#", "Fabs#"] {
        set.insert(name.to_string());
    }
    // 2026-09-01 (plan 2026-09-01-cooperative-row-kernels): subgroup
    // reduction — lowered to OpGroupNonUniformFAdd (Subgroup scope).
    set.insert("SubgroupFAdd#".to_string());
    set.insert("SubgroupFMax#".to_string());
    set.insert("SubgroupFMin#".to_string());
    set.insert("ShuffleDown#".to_string());
    set.insert("ShuffleXor#".to_string());
    set.insert("Fma#".to_string());
    set.insert("Max#".to_string());
    set.insert("Min#".to_string());
    set.insert("SubgroupBallot#".to_string());
    set.insert("SubgroupBroadcast#".to_string());
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
                reference: None,
                tolerance: None,
                rating: None,
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
    fn test_spirv_register_typedefs() {
        let mut u = TypeUniverse::new();
        let items = vec![make_type_def("Point", vec![("x", Type::int()), ("y", Type::int())])];
        normalize(&mut items.clone(), &mut u, 32).unwrap();
        let rt = u.get("Point").expect("typedef registered");
        assert_eq!(rt.bytes, 16);
    }
}
