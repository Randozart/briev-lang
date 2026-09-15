// ── Type Universe — Central Type Registry ──────────────────────────────
// 2026-07-12: Phase 2.0 — Type definition registry.
// All types are resolved to Bits(N) with metadata overlays.
// The TypeUniverse is built during the type-checking pass.

pub(crate) mod operators;
mod packed;
mod resolve;
mod validate;

pub use operators::*;
pub use packed::*;
pub use resolve::*;
pub use validate::*;

use crate::ast::Type;
use std::collections::HashMap;

/// The canonical `Cast.<Prop>` → protocol-category table, in priority order
/// (Float → UInt → Int → String → Bool → Char → Blob). 2026-09-02 (plan
/// fundamental-parent-membership): this mapping appeared inline three times
/// (operators::protocol_category, casting::graph::type_to_protocol, and the
/// typechecker's new declared_category_of) — one table, all sites iterate it.
/// `Data`/`Bit` are deliberately absent: Data is the universal fallback and
/// Bit the leaf bit type, both handled by their own rules at each site.
pub const CAST_CATEGORY_PROPS: &[(&str, &str)] = &[
    ("Cast.Float", "Float"),
    ("Cast.UInt", "UInt"),
    ("Cast.Int", "Int"),
    ("Cast.String", "String"),
    ("Cast.Bool", "Bool"),
    ("Cast.Char", "Char"),
    ("Cast.Blob", "Blob"),
];

/// The fundamental type names that ARE protocol categories (a bare parent
/// `type Float16 : Float` names one). 2026-09-02 (plan
/// fundamental-parent-membership): shared by the AST-level protocol
/// derivation (casting::derive_type_protocols); `Data`/`Bit` included —
/// they are base categories under the fundamentals model.
pub const FUNDAMENTAL_TYPES: &[&str] = &[
    "Float", "UInt", "Int", "String", "Bool", "Char", "Blob", "Data", "Bit",
];

/// Resolved metadata for a single type in the universe.
/// CTD and ALU are set by the primordial, llvm_type by the normalizer.
/// Properties, fields, and ops are all stored here — the flat property bag
/// holds ops (keyed as "op.Add" etc.) and general annotations, while the
/// `fields` vec stores ordered struct-like field declarations.
///
/// 2026-07-18: Added `fields` — struct field declarations populated from
/// TypeDef.body.slots by the normalizer (register_typedefs). For primitive
/// types seeded via seed_primordial_types(), fields is empty unless the type
/// has a struct-like shape (e.g. String has [("data", Int), ("len", Int)]).
/// Codegen uses this to drive LLVM struct lowering and state slot width,
/// replacing hardcoded "String" → "ptr" matches with shape+encoding checks.
#[derive(Debug, Clone)]
pub struct ResolvedType {
    pub name: String,
    pub base: String,
    /// Exact byte width (for fixed-width types like Int32, Float).
    /// For flexible types (Int), use max_bits instead.
    pub bytes: u64,
    /// Minimum bit width (0 = unknown/flexible). The compiler may narrow
    /// the type to any width between min_bits and max_bits.
    /// 2026-07-24: Added for value-range narrowing.
    pub min_bits: u64,
    /// Maximum bit width (upper bound). The type MUST fit in this width.
    /// For fixed types like Int32, min_bits == max_bits == 32.
    /// For flexible types like Int, min_bits=0, max_bits=64.
    pub max_bits: u64,
    pub alignment: u64,
    pub properties: HashMap<String, crate::ast::PropertyValue>,
    /// 2026-09-14 (Matrix type plan): the type's parameter NAMES in order
    /// (e.g. `Matrix<T, R, C>` → ["T", "R", "C"]). Lets the reader resolve a
    /// `spec Rows: R` value (an identifier) to the instantiation's arg index.
    /// Empty for non-generic types.
    pub type_params: Vec<String>,
    /// 2026-07-18: Struct field declarations — (name, type) pairs from
    /// TypeDef.body.slots or primordial seed table. Empty for scalars.
    /// Drives LLVM struct type lowering and is_string_like() checks.
    pub fields: Vec<(String, crate::ast::Type)>,
}

/// Central type definition registry.
/// Built during the type-checking pass from all `TopLevel::TypeDef` items.
/// 2026-08-09 (Phase 12, SPEC §19.6): the `melds` registry is removed — foreign
/// shapes adapt through GLUE/Data Briev descriptors, explicit protocol cast
/// edges, ownership contracts, and effects. No meld declarations exist.
#[derive(Debug)]
pub struct TypeUniverse {
    pub types: HashMap<String, ResolvedType>,
    /// 2026-07-31: Phase 3 (§8.5-E6) — non-fatal diagnostics surfaced by the
    /// normalizer when a type's size/width/alignment falls back to a default
    /// (e.g. a type with no primordial and no `!> bits` metadata). The LLVM
    /// backend copies these into its warning report so the fallback is never
    /// silent. The default VALUES are preserved (behavior unchanged); this
    /// channel just makes them observable.
    pub warnings: Vec<String>,
    /// 2026-08-17 (plan 2026-08-17-error-intrinsic-piggybank-hashmap-completion.md):
    /// usage-gated compile errors recorded by `Error#` in a MEMBER body. The
    /// typechecker's member-body context and its call-site context both borrow
    /// the same `&TypeUniverse`, so a member's pending error recorded here is
    /// visible to the call-site promotion (resolve_method_call /
    /// infer_generative_op_call / arrow extract). Keyed by member name.
    /// Interior mutability: `TypecheckContext` holds `&TypeUniverse`.
    pub pending_member_errors: std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
}

/// Return the default ALU for a given Common Type Definition.
// 2026-07-17: ALU describes what hardware computes with values of this type.
// PascalCase = known to all backends; lowercase-quoted = backend-specific.
impl Clone for TypeUniverse {
    fn clone(&self) -> Self {
        TypeUniverse {
            types: self.types.clone(),
            warnings: self.warnings.clone(),
            pending_member_errors: std::sync::Mutex::new(
                self.pending_member_errors.lock().unwrap().clone(),
            ),
        }
    }
}

impl TypeUniverse {
    pub fn new() -> Self {
        let mut universe = TypeUniverse {
            types: HashMap::new(),
            warnings: Vec::new(),
            pending_member_errors: std::sync::Mutex::new(std::collections::HashMap::new()),
        };
        universe.seed_primordial_types();
        universe
    }

    /// 2026-07-16: Seed the universe with primordial type entries so that
    /// `Int`, `Float`, etc. are available without stdlib import. User
    /// `type X { ... }` declarations override these via register().
    ///
    /// 2026-07-20: Simplified for hashword protocol architecture.
    /// No `ctd`, `alu`, or `encoding` properties. Types get `llvm_type`
    /// set directly here so the normalizer (which only derives "i{N*8}"
    /// from bytes) doesn't re-derive well-known types as raw integers.
    /// Hashword op signatures are the new dispatch mechanism.
    fn seed_primordial_types(&mut self) {
        // 2026-07-30: Data is the universal parent — NOT a primordial.
        // It cannot be overloaded or redeclared. Primordials are
        // overrideable; Data is the compiler's sole hardcoded constant.
        // 2026-08-15 (fundamentals): Bit was the axiomatic anchor; Data
        // replaces it as the universal parent (raw storage root). Bit<N>
        // is the bit type, still composed of Cast.Bit (treat-as-bits).
        self.types.insert("Data".to_string(), ResolvedType {
            type_params: vec![],

            name: "Data".to_string(),
            base: "Data".to_string(),
            bytes: 0,
            min_bits: 0,
            max_bits: 0,
            alignment: 0,
            properties: {
                let mut p = std::collections::HashMap::new();
                p.insert("Cast.Data".into(), crate::ast::PropertyValue::Bool(true));
                p
            },
            fields: vec![],
        });
        self.types.insert("Bit".to_string(), ResolvedType {
            type_params: vec![],

            name: "Bit".to_string(),
            base: "Data".to_string(),
            bytes: 0,
            min_bits: 0,
            max_bits: 0,
            alignment: 0,
            properties: {
                let mut p = std::collections::HashMap::new();
                p.insert("Cast.Bit".into(), crate::ast::PropertyValue::Bool(true));
                p
            },
            fields: vec![],
        });

        // Table: (name, bytes, min_bits, max_bits, alignment, &[(&str, &str)])
        // bytes is the exact width for fixed types; min_bits/max_bits is
        // the range for flexible types (Int has max_bits=64, min_bits=0).
        // 2026-07-30: These are overrideable by stdlib — adding a type
        // declaration of the same name in bootstrap.bv replaces the
        // primordial entry without error.
        // 2026-07-30: No llvm_type column — LLVM type is resolved by the
        // casting graph from (protocol, metadata). See resolve_llvm_type().
        const PRIMORDIALS: &[(&str, u64, u64, u64, u64, &[(&str, &str)])] = &[
            // 2026-07-29: Flexible protocol types — all fields resolved by normalizer from int_bits.
            // No baked-in width, alignment, or bytes. Every value is 0 = "not yet resolved."
            ("Int",    0, 0, 0,  0, &[("Cast.Int", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("UInt",   0, 0, 0,  0, &[("Cast.UInt", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            // Fixed-width integer types — exact bit width is absolute
            ("Int8",   1, 8, 8,  1, &[("Cast.Int", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("UInt8",  1, 8, 8,  1, &[("Cast.UInt", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("Int16",  2, 16, 16, 2, &[("Cast.Int", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("UInt16", 2, 16, 16, 2, &[("Cast.UInt", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("Int32",  4, 32, 32, 4, &[("Cast.Int", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("UInt32", 4, 32, 32, 4, &[("Cast.UInt", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("Int64",  8, 64, 64, 8, &[("Cast.Int", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("UInt64", 8, 64, 64, 8, &[("Cast.UInt", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("Int128", 16, 128, 128, 16, &[("Cast.Int", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("UInt128",16, 128, 128, 16, &[("Cast.UInt", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            // Floating-point types — bit-width is accuracy, not maximum storage.
            // Each float type carries an explicit bits property for the normalizer.
            ("Half",   2, 16, 16, 2, &[("Cast.Float", "true"), ("Cast.Bit", "true"), ("bits", "16")]),
            ("BFloat", 2, 16, 16, 2, &[("Cast.Float", "true"), ("Cast.Bit", "true"), ("bits", "16")]),
            ("Float",  4, 32, 32, 4, &[("Cast.Float", "true"), ("Cast.Bit", "true"), ("bits", "32")]),
            ("Float32",4, 32, 32, 4, &[("Cast.Float", "true"), ("Cast.Bit", "true"), ("bits", "32")]),
            ("Float64",8, 64, 64, 8, &[("Cast.Float", "true"), ("Cast.Bit", "true"), ("bits", "64")]),
            ("Double", 8, 64, 64, 8, &[("Cast.Float", "true"), ("Cast.Bit", "true"), ("bits", "64")]),
            ("X86_FP80",10, 80, 80, 4, &[("Cast.Float", "true"), ("Cast.Bit", "true"), ("bits", "80")]),
            ("FP128",  16, 128, 128, 16, &[("Cast.Float", "true"), ("Cast.Bit", "true"), ("bits", "128")]),
            // Other
            ("Bool",   1, 8, 8,  1, &[("Cast.Bool", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            // 2026-07-31: Phase 3 (§8.4) — Char gains Cast.Char so the casting
            // graph resolves Char → category "Char" → Fixed("i32") instead of
            // the generic "Bit" fallback (which produced i64). The graph already
            // had a Char lane (Fixed("i32")).
            ("Char",   4, 32, 32, 4, &[("Cast.Char", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("Blob",   8, 64, 64, 8, &[("Cast.Blob", "true"), ("Cast.Data", "true"), ("Cast.Bit", "true")]),
            ("Void",   0, 0,  0,  0, &[]),
        ];
        for &(name, bytes, min_bits, max_bits, alignment, extras) in PRIMORDIALS {
            let mut properties = std::collections::HashMap::new();
            properties.insert("alignment".into(), crate::ast::PropertyValue::Int(alignment as i64));
            for &(k, v) in extras {
                // 2026-07-30: Numeric extras (bits, maxbits) are stored as Int so
                if let Ok(n) = v.parse::<i64>() {
                    properties.insert(k.to_string(), crate::ast::PropertyValue::Int(n));
                } else {
                    properties.insert(k.to_string(), crate::ast::PropertyValue::String(v.to_string()));
                }
            }
            self.types.insert(name.to_string(), ResolvedType {
                type_params: vec![],

                name: name.to_string(),
                // 2026-08-15 (fundamentals): every type refines through Data
                // (the universal parent / raw storage root).
                base: "Data".to_string(),
                bytes,
                min_bits,
                max_bits,
                alignment,
                properties,
                fields: vec![],
            });
        }
        // 2026-07-18: String primordial — 2-field struct (data: Int, len: Int)
        // The casting graph resolves String's LLVM type as Fixed("{ i64, i64 }")
        // from String protocol membership. No llvm_type property needed.
        // 2026-07-31: Phase 3 (§8.4) — Cast.String seeded so the casting
        // graph resolves String → category "String" → Fixed("{ i64, i64 }").
        // Previously the property was absent, so a bare primordial universe
        // resolved String → "Bit" → i64 (wrong).
        // 2026-08-01: Bits model (B0) — a String value is a `ptr` to
        // [len][bytes]. String is now a FLEXIBLE-width primordial exactly like
        // Int/UInt: (bytes=0, min_bits=0, max_bits=0) = "not yet resolved". Its
        // width derives from the target machine word (`int_bits`, i.e. the
        // data-layout pointer width) at codegen time — a String is one machine
        // word on every target, so on x86-64 it is 64 bits, on wasm32 it is 32.
        // The old `{ i64, i64 }` fat-pointer fields (data/len) were the last
        // source of a `%String = type { i64, i64 }` named decl in emitted IR,
        // violating B0 acceptance. The LLVM type still resolves via the casting
        // graph (String → ptr); this entry provides no width of its own, and
        // `type_size` (types.rs) falls back to 8 (pointer word) for it.
        {
            let mut p = std::collections::HashMap::new();
            p.insert("alignment".into(), crate::ast::PropertyValue::Int(8));
            p.insert("Cast.String".into(), crate::ast::PropertyValue::String("true".into()));
            self.types.insert("String".to_string(), ResolvedType {
                type_params: vec![],

                name: "String".to_string(),
                base: "Data".to_string(),
                bytes: 0,
                min_bits: 0,
                max_bits: 0,
                alignment: 8,
                properties: p,
                fields: vec![],
            });
        }
    }

    /// 2026-07-16: P2 — Look up "String.c" from base "String" and extension "c".
    pub fn get_extension(&self, base: &str, ext: &str) -> Option<&ResolvedType> {
        self.types.get(&format!("{}.{}", base, ext))
    }

    pub fn get(&self, name: &str) -> Option<&ResolvedType> {
        self.types.get(name)
    }

    pub fn register(&mut self, resolved: ResolvedType) {
        self.types.insert(resolved.name.clone(), resolved);
    }

    pub fn contains(&self, name: &str) -> bool {
        self.types.contains_key(name)
    }

    /// 2026-09-14 (Matrix type plan): the SHAPE of a shape-bearing Applied
    /// type — one whose declaration has `spec Rows`/`spec Cols`/`spec Depth`
    /// (e.g. `Matrix<T, R, C>`). Returns `(rows, cols, depth)`. Each value is
    /// either a fixed `Int` (`spec Rows: 128`) or an identifier referencing a
    /// type parameter (`spec Rows: R`), resolved via `ResolvedType.type_params`
    /// to the instantiation's `Number` arg. Returns `None` for non-shape
    /// types. Generic (Rule 15: reads the universe, never a type-name match).
    pub fn matrix_shape(&self, ty: &Type) -> Option<(u64, u64, u64)> {
        let Type::Applied(name, args) = ty else {
            return None;
        };
        let rt = self.types.get(name)?;
        let read_dim = |key: &str| -> Option<u64> {
            match rt.properties.get(key)? {
                crate::ast::PropertyValue::Int(n) => Some(*n as u64),
                crate::ast::PropertyValue::Identifier(p) => {
                    let idx = rt.type_params.iter().position(|p2| p2 == p)?;
                    match args.get(idx)? {
                        Type::Number(n) => Some(*n as u64),
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        let rows = read_dim("rows")?;
        let cols = read_dim("cols")?;
        let depth = read_dim("depth").unwrap_or(1);
        Some((rows, cols, depth))
    }

    /// Look up the `Formatting` property for a type.
    pub fn get_formatting(&self, ty: &Type) -> crate::ast::Formatting {
        let name = match ty {
            Type::Custom(name) => name,
            _ => return crate::ast::Formatting::None,
        };
        self.types
            .get(name)
            .and_then(|rt| rt.properties.get("formatting"))
            .and_then(|pv| {
                if let crate::ast::PropertyValue::Identifier(s) = pv {
                    crate::ast::Formatting::from_name(s)
                } else {
                    None
                }
            })
            .unwrap_or(crate::ast::Formatting::None)
    }

    // 2026-08-15 (coll plan §3.5): is_string_like + SVO helpers
    // (is_vector_like, svo_capacity) REMOVED — never used in production.
}

#[cfg(test)]
 mod tests {
    use super::*;
    use crate::ast::Expr;

    #[test]
    fn test_get_extension_exists() {
        let mut u = TypeUniverse::new();
        u.types.insert("String.c".into(), ResolvedType {
            type_params: vec![],

            name: "String.c".into(), base: "String".into(), bytes: 8, alignment: 8,
            min_bits: 64, max_bits: 64,
            properties: HashMap::new(), fields: vec![],
        });
        assert!(u.get_extension("String", "c").is_some());
    }

    #[test]
    fn test_get_extension_not_found() {
        let u = TypeUniverse::new();
        assert!(u.get_extension("String", "c").is_none());
    }

    #[test]
    fn test_matrix_shape_param_ref() {
        // type Matrix<T, R, C> { spec Rows: R; spec Cols: C; } +
        // Matrix<Float16, 128, 64> → (128, 64, 1)
        let mut u = TypeUniverse::new();
        u.types.insert("Matrix".into(), ResolvedType {
            type_params: vec!["T".into(), "R".into(), "C".into()],
            name: "Matrix".into(), base: "Data".into(), bytes: 8, alignment: 8,
            min_bits: 64, max_bits: 64,
            properties: [
                ("rows".into(), crate::ast::PropertyValue::Identifier("R".into())),
                ("cols".into(), crate::ast::PropertyValue::Identifier("C".into())),
            ].into_iter().collect(),
            fields: vec![],
        });
        let ty = Type::Applied(
            "Matrix".into(),
            vec![Type::Custom("Float16".into()), Type::Number(128), Type::Number(64)],
        );
        assert_eq!(u.matrix_shape(&ty), Some((128, 64, 1)));
        // non-shape type → None
        let scalar = Type::Custom("Float16".into());
        assert_eq!(u.matrix_shape(&scalar), None);
    }

    #[test]
    fn test_matrix_shape_literal() {
        // type Mat4x4 : Float { spec Rows: 4; spec Cols: 4; } → (4, 4, 1)
        let mut u = TypeUniverse::new();
        u.types.insert("Mat4x4".into(), ResolvedType {
            type_params: vec![],
            name: "Mat4x4".into(), base: "Float".into(), bytes: 4, alignment: 4,
            min_bits: 32, max_bits: 32,
            properties: [
                ("rows".into(), crate::ast::PropertyValue::Int(4)),
                ("cols".into(), crate::ast::PropertyValue::Int(4)),
            ].into_iter().collect(),
            fields: vec![],
        });
        let ty = Type::Applied("Mat4x4".into(), vec![]);
        assert_eq!(u.matrix_shape(&ty), Some((4, 4, 1)));
    }
}

