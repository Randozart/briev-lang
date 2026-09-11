// ── Operator Resolution ────────────────────────────────────────────────
// 2026-07-12: Phase 2.3 — Resolve runes (+ - * / == [] etc.) to op bindings.
// This is the most critical function in the compiler (Risk #1 from the plan).
// Flat code: each function is max 2 levels, extracted helpers where needed.
//
// 2026-08-03 (operator-resolution fix): operator bindings are resolved by
// PROTOCOL CATEGORY, never by type name. The resolution order is
//   declared → parent's bindings → protocol bindings
// and ONLY the protocol bindings are hardcoded — keyed by the bare protocol
// category ("Int", "Float", "String", "Bool", "Char"), which is what the
// table keys always were. Matching a custom type's NAME against those keys
// (the old `type_name_str` lookup) made `MyNum : Int` + `MyNum` fail even
// though MyNum is a Int member and should inherit Int's Add → AddI64#.
// The protocol category is derived from the universe (Cast. properties /
// base chain), mirroring casting::graph::type_to_protocol — no name matching
// (rules 14/18). Custom types not registered in a given universe resolve
// their category through the typechecker's own type_protocols/type_parents.

use crate::ast::{OpBinding, Type};
use crate::type_universe::TypeUniverse;

/// The rune-to-op-name mapping. These are the same across all types.
pub(crate) fn rune_to_op_name(rune: &str) -> Option<&'static str> {
    Some(match rune {
        "+" => "Add",
        "-" => "Sub",
        "*" => "Mul",
        "/" => "Div",
        "%" => "Rem",
        "==" => "Eq",
        "!=" => "Neq",
        "<" => "Lt",
        ">" => "Gt",
        "<=" => "Le",
        ">=" => "Ge",
        "&&" => "And",
        "||" => "Or",
        "&" => "BitAnd",
        "|" => "BitOr",
        "^" => "BitXor",
        "<<" => "Shl",
        ">>" => "Shr",
        // 2026-07-14: String concatenation operator was missing
        "++" => "Concat",
        "[]" => "ExtractFrom",
        "[]=" => "InsertAt",
        "()" => "Call",
        _ => return None,
    })
}

/// Resolve a rune to an OpBinding for a given type.
/// Returns None if the type has no binding for the given operator.
///
/// 2026-08-03: resolves the type's PROTOCOL CATEGORY from the universe and
/// looks the operator up in the hardcoded protocol-binding table. Custom
/// types not registered in `universe` (e.g. the typechecker's fresh universe)
/// return None here — their category is resolved by the typechecker via
/// `type_protocols`/`type_parents`, which then calls `protocol_binding`.
pub fn get_operator_intrinsic(universe: &TypeUniverse, rune: &str, ty: &Type) -> Option<OpBinding> {
    let op_name = rune_to_op_name(rune)?;
    let category = protocol_category(universe, ty)?;
    protocol_binding(&category, op_name)
}

/// Resolve a rune to an OpBinding by protocol category (the ONLY hardcoded
/// operator knowledge). `category` is the bare protocol category ("Int",
/// "Float", "Bool", "Char", "String") — never a type name. 2026-08-03: split
/// out of the old name-keyed `builtin_operator_binding` so callers that know
/// the category (the typechecker, via its type_protocols record) can consult
/// it directly.
pub fn protocol_binding(category: &str, op_name: &str) -> Option<OpBinding> {
    match (category, op_name) {
        ("Int", "Add") => Some(OpBinding::Intrinsic("AddI64#".into())),
        ("Int", "Sub") => Some(OpBinding::Intrinsic("SubI64#".into())),
        ("Int", "Mul") => Some(OpBinding::Intrinsic("MulI64#".into())),
        ("Int", "Div") => Some(OpBinding::Intrinsic("DivI64#".into())),
        ("Int", "Rem") => Some(OpBinding::Intrinsic("RemI64#".into())),
        ("Int", "BitAnd") => Some(OpBinding::Intrinsic("BitAndI64#".into())),
        ("Int", "BitOr") => Some(OpBinding::Intrinsic("BitOrI64#".into())),
        ("Int", "BitXor") => Some(OpBinding::Intrinsic("BitXorI64#".into())),
        ("Int", "Shl") => Some(OpBinding::Intrinsic("ShlI64#".into())),
        ("Int", "Shr") => Some(OpBinding::Intrinsic("ShrI64#".into())),
        ("Int", "Eq") => Some(OpBinding::Intrinsic("EqI64#".into())),
        ("Int", "Lt") => Some(OpBinding::Intrinsic("LtI64#".into())),
        ("Float", "Add") => Some(OpBinding::Intrinsic("FAddF64#".into())),
        ("Float", "Sub") => Some(OpBinding::Intrinsic("FSubF64#".into())),
        ("Float", "Mul") => Some(OpBinding::Intrinsic("FMulF64#".into())),
        ("Float", "Div") => Some(OpBinding::Intrinsic("FDivF64#".into())),
        ("Float", "Eq") => Some(OpBinding::Intrinsic("FEqF64#".into())),
        ("Float", "Lt") => Some(OpBinding::Intrinsic("FLtF64#".into())),
        ("Bool", "Eq") => Some(OpBinding::Intrinsic("EqI1#".into())),
        ("Bool", "And") => Some(OpBinding::Intrinsic("AndI1#".into())),
        ("Bool", "Or") => Some(OpBinding::Intrinsic("OrI1#".into())),
        ("Char", "Eq") => Some(OpBinding::Intrinsic("EqI32#".into())),
        ("String", "Concat") => Some(OpBinding::Intrinsic("StringConcat#".into())),
        // 2026-08-03: `+` is string concat for String operands — the `++`/
        // Concat operation, resolved at the binding table so BOTH the builtin
        // String path (get_operator_intrinsic) and the typechecker's
        // variant-aware path get it from one source.
        ("String", "Add") => Some(OpBinding::Intrinsic("StringConcat#".into())),
        ("String", "Eq") => Some(OpBinding::Intrinsic("StringEq#".into())),
        // 2026-08-01 (B1): String bitwise defaults — & | ^ ~ operate on the
        // content bytes and return a new String of the same length (see
        // briev_str_band/bor/bxor/bnot). Binding here lets the typechecker
        // accept `a & b` on Strings; the backend/interpreter dispatch to the
        // content ops via String protocol membership.
        ("String", "BitAnd") => Some(OpBinding::Intrinsic("StringBitAnd#".into())),
        ("String", "BitOr") => Some(OpBinding::Intrinsic("StringBitOr#".into())),
        ("String", "BitXor") => Some(OpBinding::Intrinsic("StringBitXor#".into())),
        _ => None,
    }
}

/// Resolve a type's PROTOCOL CATEGORY (bare name: "Int", "Float", "String",
/// ...) from the universe. Mirrors casting::graph::type_to_protocol's
/// universe path — Cast.<Category> properties first, then the `base` chain.
/// Never matches type names (rules 14/18). Returns None for types with no
/// registered universe entry (custom types in a fresh universe); the
/// typechecker resolves those via its own type_protocols/type_parents.
/// 2026-08-11 (Phase 2a2): `pub` — the brievc bin (separate crate) resolves
/// b-bind marshalling categories from it.
pub fn protocol_category(universe: &TypeUniverse, ty: &Type) -> Option<String> {
    match ty {
        Type::Bits(_) | Type::Void => return Some("Bit".to_string()),
        _ => {}
    }
    let key = ty.universe_key()?;
    let rt = universe.get(key)?;
    // Cast. properties (primordial seeding) — the shared priority table
    // (mirrors the casting graph). 2026-09-02: centralized as
    // CAST_CATEGORY_PROPS (was inline here AND in graph.rs).
    let props = &rt.properties;
    for (prop, cat) in crate::type_universe::CAST_CATEGORY_PROPS {
        if props.contains_key(*prop) {
            return Some(cat.to_string());
        }
    }
    // base-chain fallback (the normalizer no longer injects Cast. for
    // subtypes) — `type Latin1String: String` ⇒ base "String".
    // 2026-08-15 (fundamentals): Data is the universal parent — a type whose
    // base is Data resolves to Data; Bit is the leaf bit type.
    let base = rt.base.trim_start_matches('#');
    match base {
        "Float" | "UInt" | "Int" | "String" | "Bool" | "Char" | "Blob"
        | "Data" | "Bit" => {
            Some(base.to_string())
        }
        _ => None,
    }
}

/// Get the intrinsic name for an operator on a type.
/// Returns a pretty-printed description for error messages if not found.
pub fn resolve_operator_or_error(
    universe: &TypeUniverse,
    rune: &str,
    ty: &Type,
) -> Result<OpBinding, String> {
    get_operator_intrinsic(universe, rune, ty)
        .ok_or_else(|| format!("no op '{}' for type {}", rune, display_type_short(ty)))
}

/// Short display of a type for error messages.
fn display_type_short(ty: &Type) -> String {
    match ty {
        Type::Custom(name) => name.clone(),
        Type::Applied(name, _) => format!("{}<...>", name),
        Type::Bits(n) => format!("Bits({})", n),
        Type::Void => "Void".into(),
        Type::Ptr(inner) => format!("Ptr<{}>", display_type_short(inner)),
        Type::Tuple(types) => {
            let inner: Vec<String> = types.iter().map(display_type_short).collect();
            format!("({})", inner.join(", "))
        }
        _ => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Type;

    fn empty_universe() -> TypeUniverse {
        TypeUniverse::new()
    }

    #[test]
    fn test_rune_to_op_name() {
        assert_eq!(rune_to_op_name("+"), Some("Add"));
        assert_eq!(rune_to_op_name("=="), Some("Eq"));
        assert_eq!(rune_to_op_name("[]"), Some("ExtractFrom"));
        assert_eq!(rune_to_op_name("???"), None);
    }

    #[test]
    fn test_protocol_binding_int_add() {
        assert_eq!(
            protocol_binding("Int", "Add"),
            Some(OpBinding::Intrinsic("AddI64#".into()))
        );
    }

    #[test]
    fn test_protocol_binding_missing_category() {
        assert_eq!(protocol_binding("NoSuchCategory", "Add"), None);
        assert_eq!(protocol_binding("Int", "NoSuchOp"), None);
    }

    #[test]
    fn test_builtin_int_add() {
        let binding = get_operator_intrinsic(&empty_universe(), "+", &Type::int());
        assert_eq!(binding, Some(OpBinding::Intrinsic("AddI64#".into())));
    }

    #[test]
    fn test_builtin_float_mul() {
        let binding = get_operator_intrinsic(&empty_universe(), "*", &Type::float());
        assert_eq!(binding, Some(OpBinding::Intrinsic("FMulF64#".into())));
    }

    #[test]
    fn test_builtin_bool_eq() {
        let binding = get_operator_intrinsic(&empty_universe(), "==", &Type::bool_());
        assert_eq!(binding, Some(OpBinding::Intrinsic("EqI1#".into())));
    }

    #[test]
    fn test_builtin_string_concat() {
        let binding = get_operator_intrinsic(&empty_universe(), "++", &Type::string());
        assert_eq!(binding, Some(OpBinding::Intrinsic("StringConcat#".into())));
    }

    #[test]
    fn test_missing_operator() {
        let binding = get_operator_intrinsic(&empty_universe(), "*", &Type::bool_());
        assert_eq!(binding, None);
    }

    #[test]
    fn test_missing_type() {
        // A custom type with no universe entry resolves to no category.
        let binding = get_operator_intrinsic(&empty_universe(), "+", &Type::Custom("Nonexistent".into()));
        assert_eq!(binding, None);
    }

    #[test]
    fn test_int8_resolves_via_protocol() {
        // Int8 is a Int protocol member — + must resolve through the
        // category, not the type name. 2026-08-03: the old name-keyed table
        // returned None here (the bug this fix removes).
        let binding = get_operator_intrinsic(&empty_universe(), "+", &Type::Custom("Int8".into()));
        assert_eq!(binding, Some(OpBinding::Intrinsic("AddI64#".into())));
    }

    #[test]
    fn test_fundamental_category_resolves() {
        // 2026-09-11 (Phase A4): the bare fundamental IS the protocol —
        // the + op on `Int` resolves through the universe category.
        let binding = get_operator_intrinsic(&empty_universe(), "+", &Type::Custom("Int".into()));
        assert_eq!(binding, Some(OpBinding::Intrinsic("AddI64#".into())));
    }

    #[test]
    fn test_universe_lookup_empty() {
        let uni = empty_universe();
        // Int is a seeded primordial with Cast.Int — resolves via category.
        let result = get_operator_intrinsic(&uni, "+", &Type::int());
        assert_eq!(result, Some(OpBinding::Intrinsic("AddI64#".into())));
    }
}
