// ── VM Normalizer — Minimal Universe Registration ────────────────────
// 2026-08-10: The VM is untyped (stack bytecode — sizes only, no native
// type derivation), but its universe must be populated exactly like every
// other backend so later passes see a uniform universe. The hand-rolled
// partial path would rot the normalizer invariant — VM gets the shared
// register_typedefs call and nothing more.

use crate::backend::register_types::register_typedefs;
use crate::type_universe::TypeUniverse;
use crate::ast::TopLevel;

/// 2026-08-10: Minimal VM normalization — register user TypeDefs so the
/// universe is uniformly populated, then hand off to bytecode emission.
pub fn normalize(items: &mut Vec<TopLevel>, universe: &mut TypeUniverse, int_bits: u64) -> Result<(), String> {
    register_typedefs(items, universe, int_bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::top::{TypeDef, TypeDefBody, TypeDefSlot};
    use crate::ast::Type;
    use std::collections::HashMap;

    #[test]
    fn test_vm_register_typedefs() {
        let mut u = TypeUniverse::new();
        let td = TopLevel::TypeDef(Box::new(TypeDef {
            name: "Point".to_string(),
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
                slots: vec![
                    TypeDefSlot { name: "x".to_string(), ty: Type::int(), bit_range: None },
                    TypeDefSlot { name: "y".to_string(), ty: Type::int(), bit_range: None },
                ],
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
        }));
        let items = vec![td];
        normalize(&mut items.clone(), &mut u, 64).unwrap();
        let rt = u.get("Point").expect("typedef registered");
        assert_eq!(rt.bytes, 16);
    }
}