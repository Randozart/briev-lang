// ── Type Resolution ────────────────────────────────────────────────────
// 2026-07-12: Phase 2.1 — Resolve type definitions to Bits(N).
// Follows the derivation chain until reaching Bits or a known base type.
//
// 2026-07-14: No fallback tables. If the type isn't in the universe,
// resolve_type returns None and the backend treats it as raw Bits(8).
// Type::int(), Type::float() etc. are pure name constructors with zero
// semantics — semantics come from source declarations in bootstrap.bv.

use crate::type_universe::{ResolvedType, TypeUniverse};

/// Resolve a Type to its ResolvedType metadata.
/// Returns None for types not in the universe — backend falls back to Bits(8).
pub fn resolve_type(universe: &TypeUniverse, ty: &crate::ast::Type) -> Option<ResolvedType> {
    match ty {
        crate::ast::Type::Custom(name) => universe.get(name).cloned(),
        crate::ast::Type::Bits(n) => {
            // 2026-08-13 (layout-keywords plan): Bits stores bits; storage bytes
            // round up (div_ceil) so sub-byte widths keep one storage byte.
            let bytes = n.div_ceil(8);
            Some(ResolvedType {
                name: format!("Bits({})", n),
                base: "Data".into(),
                bytes,
                min_bits: *n,
                max_bits: *n,
                alignment: bytes.min(8),
                properties: std::collections::HashMap::new(),
                fields: vec![],
                type_params: vec![],
            })
        },
        crate::ast::Type::Ptr(_) => Some(ResolvedType {
            name: "Ptr".into(),
            base: "Ptr".into(),
            bytes: 8,
            min_bits: 64,
            max_bits: 64,
            alignment: 8,
            properties: std::collections::HashMap::new(),
            fields: vec![],
            type_params: vec![],
        }),
        crate::ast::Type::Void => Some(ResolvedType {
            name: "Void".into(),
            base: "Data".into(),
            bytes: 0,
            min_bits: 0,
            max_bits: 0,
            alignment: 1,
            properties: std::collections::HashMap::new(),
            fields: vec![],
            type_params: vec![],
        }),
        _ => None,
    }
}
