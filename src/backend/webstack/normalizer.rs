// ── Webstack Normalizer — First-Class Universe Registration Pass ──────
// 2026-07-14: Attaches js_type for JS/TS/WASM codegen.
// 2026-08-10: Rewritten as a first-class pass. Registers user typedefs via
// the shared backend::register_types::register_typedefs (wasm32/32-bit so
// flexible types fall back to 32-bit pointers), then derives each type's
// js_type from its PROTOCOL CATEGORY (Cast. properties — rules 14/18:
// never matches type names). Strips metadata the webstack shim and generator
// don't consume (keeps Cast. + js_type + width metadata).

use std::collections::HashSet;
use crate::ast::*;
use crate::backend::normalizer;
use crate::backend::register_types::register_typedefs;
use crate::type_universe::TypeUniverse;
use crate::type_universe::protocol_category;

/// 2026-07-14: Normalize the AST for the Webstack (WASM + JS) backend.
/// 1. Register all user typedefs (shared pass) — wasm32 ⇒ int_bits = 32.
/// 2. Derive js_type from protocol category (Cast.).
/// 3. Validate intrinsics against the WASM supported set.
/// 4. Retain only the metadata the webstack backend consumes.
pub fn normalize(items: &mut Vec<TopLevel>, universe: &mut TypeUniverse, _int_bits: u64) -> Result<(), String> {
    // 2026-08-10: wasm32 target width is fixed at 32 for flexible types
    // (the LLVM wasm backend resolves 32-bit pointers regardless); the
    // shared pass records a warning when a type needs the fallback.
    register_typedefs(items, universe, 32)?;

    // 2026-08-10: derive js_type from protocol category, never type names.
    // A custom struct (no Cast. protocol) collapses to "object"; collection
    // metadata (e.g. `type List<T>: #Collection`) also maps to "object".
    // Protocol lookup reads the universe, so collect the mapping first and
    // apply it after the mutable iteration ends (mirrors the LLVM normalizer's
    // borrow discipline).
    let mut js_types: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for name in universe.types.keys() {
        let ty = Type::Custom(name.clone());
        let cat = protocol_category(universe, &ty);
        let js_type = match cat.as_deref() {
            Some("Int" | "UInt" | "Float") => "number",
            Some("Bool") => "boolean",
            Some("String" | "Char" | "Blob") => "string",
            // 2026-07-26: TS legacy wire format — a two-slot String-like
            // (data/len) is the bare `String` shim; everything else is object.
            _ => "object",
        };
        js_types.insert(name.clone(), js_type.to_string());
    }
    for rt in universe.types.values_mut() {
        if let Some(js) = js_types.get(&rt.name) {
            rt.properties.insert("js_type".into(), PropertyValue::String(js.clone()));
        }
    }

    // 2026-07-26: Reject intrinsics not supported by WASM/WebAssembly backend.
    // See docs/architecture/features/webstack-intrinsics.md for the full policy.
    let supported = build_supported_ops();
    let errors = normalizer::validate_intrinsics(items, &supported);
    if !errors.is_empty() {
        let detail = errors.join("\n  ");
        return Err(format!(
            "Intrinsic is not supported by the webstack/WebAssembly backend:\n  {}\n\
             See docs/architecture/features/webstack-intrinsics.md",
            detail
        ));
    }

    // 2026-08-10: retain what the backend consumes — protocol membership,
    // js_type, and width/alignment metadata (mirrors the LLVM normalizer's
    // keep-list; `llvm_type`/`disamb` are legacy, never set by the universe).
    let keep: HashSet<String> = [
        "js_type", "bits", "maxbits", "minbits", "alignment",
    ].iter().map(|s| s.to_string()).collect();
    for rt in universe.types.values_mut() {
        rt.properties.retain(|k, _| k.starts_with("Cast.") || keep.contains(k));
    }

    Ok(())
}

/// Webstack supported intrinsics — Tiers 1-3 from webstack intrinsics policy.
/// 2026-07-26: Tier 1 = WASM native, Tier 2 = WASM runtime, Tier 3 = browser API.
/// Any intrinsic not in this set produces a compile error:
///   "Intrinsic '<name>' is not supported by the webstack/WebAssembly backend."
/// See docs/architecture/features/webstack-intrinsics.md
fn build_supported_ops() -> HashSet<String> {
    let mut set = HashSet::new();
    // Tier 1: WASM native (arithmetic, comparison, bitwise, float math)
    for op in &[
        "Add#", "Sub#", "Mul#", "Div#", "Rem#", "Neg#", "Abs#",
        "Eq#", "Neq#", "Lt#", "Gt#", "Le#", "Ge#",
        "BitAnd#", "BitOr#", "BitXor#", "Shl#", "Shr#", "BitNot#",
        "Not#",
        "Fabs#", "Ceil#", "Floor#", "Sqrt#", "Sin#", "Cos#", "Pow#",
    ] { set.insert(op.to_string()); }
    // Tier 2: WASM runtime (memory, atomics, pointer, string ops)
    for op in &[
        "Ptr#", "Deref#", "Index#", "Cast#", "AddressOf#",
        "Load#", "Store#", "Malloc#", "Alloc#", "Free#", "Copy#", "Fill#",
        "Memcpy#", "Memset#",
        "Len#", "Length#", "Concat#", "Get#", "Insert#",
        "ToInt#", "ToFloat#", "ToString#",
        "AtomicLoad#", "AtomicStore#", "AtomicCas#", "AtomicXchg#",
        "AtomicAdd#", "Fence#",
    ] { set.insert(op.to_string()); }
    // Tier 3: Browser API (console, time, env queries — JS shim provides)
    for op in &[
        "PrintInt#", "PrintFloat#", "PrintChar#", "Print#",
        "Time#", "CpuCount#", "Hostname#", "PageSize#", "Errno#", "Sleep#",
    ] { set.insert(op.to_string()); }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_type_def(name: &str, protocol: Option<&str>, slots: Vec<(&str, Type)>) -> TopLevel {
        use crate::ast::top::{TypeDef, TypeDefBody, TypeDefSlot};
        TopLevel::TypeDef(Box::new(TypeDef {
            name: name.to_string(),
            type_params: vec![],
            parent: None,
            protocol: protocol.map(|p| p.to_string()),
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
                metadata: std::collections::HashMap::new(),
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

    fn js_type_of(u: &TypeUniverse, name: &str) -> String {
        let rt = u.get(name).expect("registered");
        match rt.properties.get("js_type") {
            Some(PropertyValue::String(s)) => s.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn test_js_type_from_protocol_category() {
        let mut u = TypeUniverse::new();
        let items = vec![
            make_type_def("MSFT", Some("String"), vec![]),
            make_type_def("Point", Some("Bit"), vec![("x", Type::int())]),
        ];
        normalize(&mut items.clone(), &mut u, 32).unwrap();
        assert_eq!(js_type_of(&u, "MSFT"), "string");
        assert_eq!(js_type_of(&u, "Point"), "object");
    }

    #[test]
    fn test_custom_type_registered() {
        let mut u = TypeUniverse::new();
        let items = vec![make_type_def("Widget", Some("Bit"), vec![("id", Type::int())])];
        normalize(&mut items.clone(), &mut u, 32).unwrap();
        assert!(u.get("Widget").is_some(), "user typedef must be registered");
    }
}