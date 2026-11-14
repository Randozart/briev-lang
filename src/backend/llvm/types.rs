// ── LLVM Type Lowering ─────────────────────────────────────────────────
// 2026-07-12: Phase 2.6/4 — Lower Briev types to LLVM IR type strings.
// Consults the llvm metadata property, falls back to iN based on byte width.

use crate::ast::Type;
use crate::type_universe::resolve_type;

/// 2026-07-26: Lower a Briev Type to an LLVM IR type string.
/// Delegates to protocol_llvm_type for named types when a universe is
/// available. Falls back to "i64" for unknown types.
pub fn lower_type(ty: &Type, universe: Option<&crate::type_universe::TypeUniverse>) -> String {
    match ty {
        Type::Custom(name) => lower_custom_type(name, universe),
        // 2026-08-13 (Phase 6): `Bits<N>` parses as Applied("Bits", [Number(N)])
        // — the exact-width alias — lower it to i{N} so a `Bits<32>` field
        // reads/writes exactly 32 bits (a plain/union struct field, not just
        // packed). The generic-application default (i64) silently widened it.
        Type::Applied(name, _) if name == "Bits" => match crate::type_universe::bits_width(ty) {
            Some(n) => format!("i{}", n),
            None => lower_custom_type(name, universe),
        },
        Type::Applied(name, _) => lower_custom_type(name, universe),
        Type::Bits(n) => format!("i{}", n),
        Type::Void => "void".into(),
        Type::Ptr(_) => "ptr".into(),
        Type::Tuple(types) => {
            let inner: Vec<String> = types.iter().map(|t| lower_type(t, universe)).collect();
            format!("{{ {} }}", inner.join(", "))
        }
        Type::Function(params, ret) => {
            // 2026-08-03: a function VALUE (callback param / CallPtr#
            // operand) is a two-word code/env PAIR, and `ptr` under opaque
            // pointers points at that pair — NOT at the function itself.
            // Dispatch therefore loads word 0 for the code pointer and passes
            // the pair as the environment (see emit_call_ptr); calling the ptr
            // directly jumps to the pair's address. The bare `ret (params)`
            // form is only valid in a function DECLARATION, never as a
            // parameter/operand type.
            let _ = (params, ret);
            "ptr".into()
        }
        _ => "i64".into(),
    }
}

/// Lower a custom named type. Delegates to protocol_llvm_type when a
/// universe is available; no fallback name table.
fn lower_custom_type(name: &str, universe: Option<&crate::type_universe::TypeUniverse>) -> String {
    crate::backend::llvm::protocol_llvm_type(&Type::Custom(name.to_string()), universe)
}

/// 2026-07-26: Get the byte size of a type in the LLVM ABI.
/// Uses the ResolvedType.bytes from the universe when available.
/// Falls back to computing from the LLVM type string.
pub fn type_size(ty: &Type, universe: Option<&crate::type_universe::TypeUniverse>) -> u64 {
    // 2026-08-04 (compiler-in-Briev): a Ptr is ALWAYS one machine word. The
    // universe path below would return 0 for an unsubstituted generic
    // `Ptr<T>` (bytes=0, no Cast. property) and silently collapse struct
    // layouts containing such fields (List<String>.len collided with
    // inner.cap at offset 8). Pointer width is target-invariant here (8).
    if matches!(ty, Type::Ptr(_)) {
        return 8;
    }
    // When universe is available, read bytes from ResolvedType.
    if let Some(ref u) = universe {
        if let Some(rt) = ty.universe_key().and_then(|k| u.get(k)) {
            if rt.bytes > 0 {
                return rt.bytes;
            }
            // 2026-07-29: Flexible protocol type with no baked-in bytes.
            // Compute from protocol membership with conservative defaults.
            // After normalizer runs, rt.bytes is set from int_bits + protocol.
            if rt.properties.contains_key("Cast.Int") || rt.properties.contains_key("Cast.UInt") {
                return 8; // conservative default for Int/UInt
            }
            // 2026-08-01: Bits model (B0) — String is a flexible-width primordial
            // (bytes=0) whose LLVM type is a pointer; it is one machine word on
            // every target, so the conservative default is the pointer word (8).
            // Mirrors the Int/UInt flexible fallback above.
            if rt.properties.contains_key("Cast.String") {
                return 8;
            }
            if rt.properties.contains_key("Cast.Float") {
                if let Some(crate::ast::PropertyValue::Int(bits)) = rt.properties.get("bits") {
                    return (*bits as u64) / 8;
                }
                return 4; // default float size
            }
            if rt.properties.contains_key("Cast.Bool") || rt.properties.contains_key("Cast.Bit") {
                return 1;
            }
            return 0;
        }
    }
    match ty {
        // 2026-08-13 (layout-keywords plan): Bits stores BITS; storage bytes are
        // the rounded-up byte count (div_ceil), never the raw width. Sub-byte
        // widths (Bits<4>) round up to one storage byte.
        Type::Bits(n) => n.div_ceil(8),
        Type::Ptr(_) => 8,
        Type::Void => 0,
        // 2026-07-25: Fixed-size array: Int[1024] → 1024 * element_size.
        Type::Vector(inner, dims) => {
            let elem_size = type_size(inner, universe);
            let count: u64 = dims
                .iter()
                .map(|d| match d {
                    crate::ast::Dimension::Anonymous(n) => *n as u64,
                    crate::ast::Dimension::Named(_, n) => *n as u64,
                })
                .product();
            elem_size * count
        }
        _ => 8,
    }
}

/// Get the alignment of a type in bytes.
pub fn type_alignment(ty: &Type, universe: Option<&crate::type_universe::TypeUniverse>) -> u64 {
    type_size(ty, universe).min(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-08-13 (layout-keywords plan): Bits stores BITS. These lock the
    /// restored thesis: `Bits<N>` lowers to `iN` and storage rounds UP via
    /// div_ceil, so sub-byte widths (Bits<4>) keep one storage byte.
    #[test]
    fn sub_byte_bits_round_trip() {
        assert_eq!(lower_type(&Type::Bits(4), None), "i4");
        assert_eq!(type_size(&Type::Bits(4), None), 1);
        assert_eq!(lower_type(&Type::Bits(8), None), "i8");
        assert_eq!(type_size(&Type::Bits(8), None), 1);
        assert_eq!(lower_type(&Type::Bits(64), None), "i64");
        assert_eq!(type_size(&Type::Bits(64), None), 8);
    }

    /// 2026-08-13: flexible-width Bits(0) has no concrete size yet.
    #[test]
    fn flexible_bits_zero_preserved() {
        assert_eq!(lower_type(&Type::Bits(0), None), "i0");
        assert_eq!(type_size(&Type::Bits(0), None), 0);
    }

    /// 2026-08-13 (intrinsics.rs latent-bug regression guard): a byte pointer
    /// built as `Ptr<Bits(8)>` now lowers to a REAL i8 element (8 BITS); in the
    /// byte era `Type::bits(8)` was 8 BYTES → i64 element. The same expression
    /// is emitted by AddressOf#/CastPtr# (intrinsics.rs:724).
    #[test]
    fn byte_pointer_element_is_i8() {
        let p = Type::ptr(Type::bits(8));
        match &p {
            Type::Ptr(inner) => assert_eq!(lower_type(inner, None), "i8"),
            _ => panic!("expected Ptr"),
        }
        assert_eq!(type_size(&p, None), 8);
    }
}
