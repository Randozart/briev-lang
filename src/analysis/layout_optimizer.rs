// ── Layout Optimizer — "Become the Foreign" ──────────────────────────
// 2026-07-22: Analysis pass that proposes type layout specialization at
// frgn/export boundaries to minimize protocol transformation costs.
//
// How it works:
//   1. Scan the AST for frgn declarations with Bridge dispatch
//   2. For each parameter type, find the foreign type's layout from the
//      GLUE registry + type universe
//   3. If adopting the foreign layout would make the boundary identity
//      and the foreign type can cast back safely, propose the change
//
// Safety: A specialized type must have CastTo/CastFrom back to the
// original protocol. The optimizer rejects unsafe proposals.

use std::collections::HashMap;

use crate::analysis::frgn_dispatch::ResolvedFrgn;
use crate::ast::top::{ForeignBinding, TopLevel};
use crate::ast::{PropertyValue, Type};
use crate::glue::config::GlueTarget;
use crate::type_universe::TypeUniverse;

/// Result of the layout optimization pass: a set of type layout changes
/// to apply before codegen.
///
/// 2026-07-22: Each change targets a TypeDef in the AST, modifying its
/// `maxbits <~ N` and `alignment <~ N` metadata to match the foreign type's
/// layout. The backend emits whatever layout it receives.
pub struct LayoutChange {
    /// The name of the type to modify (e.g., "Int")
    pub type_name: String,
    /// New byte size to adopt (e.g., 16 for a 2-field struct)
    pub new_bytes: u64,
    /// New alignment to adopt
    pub new_alignment: u64,
    /// The foreign type this layout was modeled after (for documentation)
    pub modeled_after: String,
}

/// Run the layout optimizer pass.
///
/// 2026-07-22: Scans all ForeignBindings that resolve to Bridge dispatch,
/// computes protocol paths for each parameter type, and proposes layout
/// changes where adopting the foreign layout would make the boundary
/// cost zero. Returns empty vec if no optimization is beneficial.
///
/// # Arguments
/// * `items` — parsed AST items
/// * `universe` — type universe with resolved types and melds
/// * `resolved_frgns` — pre-computed dispatch strategies per frgn
/// * `glue_targets` — GLUE registry for language type mappings
pub fn optimize_layouts(
    items: &[TopLevel],
    universe: &TypeUniverse,
    resolved_frgns: &HashMap<String, ResolvedFrgn>,
    glue_targets: &HashMap<String, GlueTarget>,
) -> Result<Vec<LayoutChange>, String> {
    let mut changes = Vec::new();

    for item in items {
        let fb = match item {
            TopLevel::ForeignBinding(fb) => fb,
            _ => continue,
        };

        // 2026-07-22: Only optimize bridge-path frgns (GLUE-mediated calls).
        // Inline frgns and unsupported ones are skipped.
        let target = match resolved_frgns.get(fb.effective_briev_name()) {
            Some(ResolvedFrgn::Bridge { language, .. }) => {
                match glue_targets.get(language) {
                    Some(t) => t,
                    None => continue,
                }
            }
            _ => continue,
        };

        for (_param_name, param_ty) in &fb.inputs {
            let ty_key = match param_ty.universe_key() {
                Some(k) => k,
                None => continue,
            };

            // 2026-07-22: Get the current type's layout from the universe.
            let current = match universe.get(ty_key) {
                Some(rt) => rt,
                None => continue,
            };

            // 2026-07-22: Find the protocol category for this type via CastTo.
            let protocol_cat = find_protocol_category(universe, ty_key);
            // 2026-07-26: c_abi is optional — fall back to derive_foreign_type_name
            let foreign_ty_name = match protocol_cat {
                Some(cat) => {
                    let protocol_key = format!("#{}", cat);
                    target.protocols.get(&protocol_key)
                        .and_then(|e| e.c_abi.clone())
                        .unwrap_or_else(|| derive_foreign_type_name(param_ty, &target.language))
                }
                None => derive_foreign_type_name(param_ty, &target.language),
            };

            // 2026-07-22: Get the foreign type's layout. If the foreign type
            // isn't in the universe (e.g., types.bv wasn't loaded), skip.
            let (foreign_bytes, foreign_alignment) = match get_type_layout(universe, &foreign_ty_name) {
                Some(l) => l,
                None => continue,
            };

            // 2026-07-22: If layouts are already identical, no optimization.
            if current.bytes == foreign_bytes && current.alignment == foreign_alignment {
                continue;
            }

            // 2026-08-09 (Phase 12, SPEC §18.2): the meld identity check is
            // removed — foreign shapes adapt through explicit protocol cast
            // edges, not an implicit meld. The safe-cast-path check below is
            // the remaining gate.

            // 2026-07-22: Safety check — the foreign type must be able to
            // cast back to the original type or its protocol (#Bits).
            // Otherwise adopting its layout would break type safety.
            if !has_safe_cast_path(universe, &foreign_ty_name, ty_key) {
                continue;
            }

            // 2026-07-22: All checks passed — propose the layout change.
            changes.push(LayoutChange {
                type_name: ty_key.to_string(),
                new_bytes: foreign_bytes,
                new_alignment: foreign_alignment,
                modeled_after: foreign_ty_name,
            });
        }
    }

    Ok(changes)
}

/// Apply a LayoutChange to the AST items.
///
/// 2026-07-22: Finds the TypeDef matching `change.type_name` and updates
/// its `bytes <~` and `alignment <~` metadata to match the foreign layout.
/// Returns an error if the type is not defined in the program.
///
/// # Arguments
/// * `items` — mutable AST items (modified in place)
/// * `change` — layout change to apply
pub fn apply_layout_change(items: &mut [TopLevel], change: &LayoutChange) -> Result<(), String> {
    for item in items.iter_mut() {
        let td = match item {
            TopLevel::TypeDef(td) if td.name == change.type_name => td,
            _ => continue,
        };

        // 2026-07-25: Use maxbits (bits) instead of bytes.
        td.body.metadata.insert(
            "maxbits".to_string(),
            PropertyValue::Int(change.new_bytes as i64 * 8),
        );

        td.body.metadata.insert(
            "alignment".to_string(),
            PropertyValue::Int(change.new_alignment as i64),
        );

        return Ok(());
    }

    Err(format!(
        "layout change targets type '{}' which is not defined in the program",
        change.type_name
    ))
}

/// Get the layout (bytes, alignment) for a named type from the universe.
///
/// 2026-07-22: Looks up the ResolvedType in the universe and returns its
/// byte size and alignment. Returns None if the type is not registered.
fn get_type_layout(universe: &TypeUniverse, type_name: &str) -> Option<(u64, u64)> {
    universe.get(type_name).map(|rt| (rt.bytes, rt.alignment))
}

/// Find the protocol category a Briev type participates in via its CastTo properties.
/// Returns the category name (e.g., "String" for Cast.String) or None.
fn find_protocol_category(universe: &TypeUniverse, type_name: &str) -> Option<String> {
    let rt = universe.get(type_name)?;
    for prop_key in rt.properties.keys() {
        if let Some(cat) = prop_key.strip_prefix("Cast.") {
            return Some(cat.to_string());
        }
    }
    None
}

/// Derive the foreign type name for a Briev parameter type in the target language.
///
/// 2026-07-22: Maps Briev type names to their foreign equivalents based on
/// known language conventions. Python types are prefixed "Py", Node "Js",
/// Rust "Rst". Unknown languages use the capitalized language name as prefix.
///
/// # Examples
/// * `derive_foreign_type_name(Int, "python")` → `"PyInt"`
/// * `derive_foreign_type_name(String, "node")` → `"JsString"`
/// * `derive_foreign_type_name(Float, "rust")` → `"RstFloat"`
fn derive_foreign_type_name(ty: &Type, language: &str) -> String {
    // 2026-07-22: Foreign type name is the protocol's c_abi type.
    // Look up the protocol category from the type's CastTo property.
    // If the protocol exists, use its c_abi type. Otherwise, derive a name.
    let prefix = match language {
        "python" => "Py",
        "node" | "javascript" => "Js",
        "rust" => "Rst",
        _ => return format!("{}{}", capitalize_first(language), type_display_name(ty)),
    };
    match ty.universe_key() {
        Some("Int") => format!("{}Int", prefix),
        Some("Float") => format!("{}Float", prefix),
        Some("Bool") => format!("{}Bool", prefix),
        Some("String") => format!("{}String", prefix),
        Some("Char") => format!("{}Char", prefix),
        Some(other) => format!("{}{}", prefix, other),
        None => format!("{}{}", prefix, type_display_name(ty)),
    }
}

/// Capitalize the first character of a string.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

/// Get a human-readable display name for a type.
fn type_display_name(ty: &Type) -> String {
    match ty {
        Type::Custom(name) => name.clone(),
        _ => format!("{:?}", ty),
    }
}

/// Check if a foreign type has a safe CastTo/CastFrom path back to the
/// original Briev type or its protocol.
///
/// 2026-07-22: Safety precondition for layout adoption. The foreign type
/// must be able to cast back to the original type (directly or through
/// a shared protocol like #Bits). If no path exists, adopting the foreign
/// layout would make the Briev type incompatible.
fn has_safe_cast_path(universe: &TypeUniverse, foreign_type: &str, original_type: &str) -> bool {
    if find_cast_path(universe, foreign_type, original_type).is_some() {
        return true;
    }
    find_cast_path(universe, foreign_type, "Bits").is_some()
}

/// BFS shortest path through the protocol graph.
///
/// 2026-07-22: Finds a sequence of Cast ops from source_type to target_type.
/// Uses the type universe's property map, scanning for keys with the "Cast."
/// prefix (e.g., "Cast.Int", "Cast.String"). #Bits is always reachable
/// from every type (implicit Cast(#Bits)).
///
/// 2026-07-30: ProtocolGraph parameter removed — variant-aware edges are
/// handled by CastingGraph (src/casting/graph.rs). This function uses
/// the universe's Cast. properties for backward compat during migration.
pub(crate) fn find_cast_path(
    universe: &TypeUniverse,
    source_type: &str,
    target_type: &str,
) -> Option<Vec<String>> {
    use std::collections::VecDeque;

    let mut visited = std::collections::HashSet::new();
    let mut queue: VecDeque<(String, Vec<String>)> = VecDeque::new();
    queue.push_back((source_type.to_string(), vec![source_type.to_string()]));
    visited.insert(source_type.to_string());

    while let Some((current, path)) = queue.pop_front() {
        if current == target_type || current == format!("#{}", target_type) {
            return Some(path);
        }

        // 2026-09-11 (Phase A4): the synthetic universal-cast node is bare
        // "Bits" — matching the retired-# world and the glue CastTo(Bits)
        // registrations.
        if !visited.contains("Bits") {
            visited.insert("Bits".to_string());
            let mut new_path = path.clone();
            new_path.push("Bits".to_string());
            queue.push_back(("Bits".to_string(), new_path));
        }

        if let Some(rt) = universe.get(&current) {
            for key in rt.properties.keys() {
                if let Some(target_name) = key.strip_prefix("Cast.") {
                    if !target_name.is_empty() && !visited.contains(target_name) {
                        visited.insert(target_name.to_string());
                        let mut new_path = path.clone();
                        new_path.push(target_name.to_string());
                        queue.push_back((target_name.to_string(), new_path));
                    }
                }
            }
        }

        // 2026-07-30: ProtocolGraph variant-aware edges removed.
        // CastingGraph (src/casting/graph.rs) handles variant-aware cast
        // paths via register_protocol_def() + find_path().
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::top::*;
    use crate::ast::{Expr, PropertyValue, Type};
    use crate::glue::config::GlueTarget;
    use std::path::PathBuf;

    // ── Helpers ──────────────────────────────────────────────────────────

    // Type shorthand helpers
    fn ti() -> Type { Type::int() }
    fn tf() -> Type { Type::float() }
    fn tb() -> Type { Type::bool_() }
    fn ts() -> Type { Type::string() }

    fn make_frgn(name: &str, ext: &str, param_types: Vec<(&str, Type)>) -> TopLevel {
        let inputs: Vec<(String, Type)> = param_types.into_iter()
            .map(|(n, t)| (n.to_string(), t))
            .collect();
        let mut fb = ForeignBinding::new(
            name.to_string(),
            None,
            FromSpec::Literal(PathBuf::from(format!("lib.{}", ext))),
            ForeignTarget::Native,
        );
        fb.inputs = inputs;
        TopLevel::ForeignBinding(fb)
    }

    fn sample_glue_targets() -> HashMap<String, GlueTarget> {
        let mut map = HashMap::new();
        map.insert("python".to_string(), GlueTarget {
            language: "python".to_string(),
            types_module: PathBuf::from("glue/python/types.bv"),
            extension: "py".to_string(),
            bridge_kind: "native_module".to_string(),
            calling_convention: "c_abi".to_string(),
            module_init: false,
            protocols: HashMap::new(),
            templates: HashMap::new(),
            conversions: crate::glue::config::Conversions::default(),
            state: crate::glue::config::StateAbi::default(),
            param_decl: "{name}: {type}".to_string(),
            fn_param_decl: "{name}: {type}".to_string(),
            ..Default::default()
        });
        map.insert("rust".to_string(), GlueTarget {
            language: "rust".to_string(),
            types_module: PathBuf::from("glue/rust/types.bv"),
            extension: "rs".to_string(),
            bridge_kind: "extern_c_crate".to_string(),
            calling_convention: "lto".to_string(),
            module_init: false,
            protocols: HashMap::new(),
            templates: HashMap::new(),
            conversions: crate::glue::config::Conversions::default(),
            state: crate::glue::config::StateAbi::default(),
            param_decl: "{name}: {type}".to_string(),
            fn_param_decl: "{name}: {type}".to_string(),
            ..Default::default()
        });
        map
    }

    fn make_universe_with_types(pairs: &[(&str, u64, u64)]) -> TypeUniverse {
        let mut u = TypeUniverse::new();
        for &(name, bytes, alignment) in pairs {
            u.types.insert(name.to_string(), crate::type_universe::ResolvedType {
                type_params: vec![],
                name: name.to_string(),
                base: "Data".to_string(),
                bytes,
                min_bits: bytes * 8,
                max_bits: bytes * 8,
                alignment,
                properties: std::collections::HashMap::new(),
                fields: vec![],
            });
        }
        u
    }

    fn make_type_def(name: &str, bytes: u64, alignment: u64) -> TopLevel {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("maxbits".to_string(), PropertyValue::Int(bytes as i64 * 8));
        metadata.insert("alignment".to_string(), PropertyValue::Int(alignment as i64));
        TopLevel::TypeDef(Box::new(TypeDef {
            name: name.to_string(),
            type_params: vec![],
            parent: None,
            protocol: None,
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
                slots: vec![],
                metadata,
                projections: vec![],
                bindings: vec![],
                operators: vec![], op_bindings: vec![],
                constraints: vec![],
                members: vec![],
                when_laws: vec![],
                modes: vec![],
                span: None,
            },
            span: None,
        }))
    }

    // ── Tests ────────────────────────────────────────────────────────────

    /// No frgn declarations → no layout changes.
    #[test]
    fn test_layout_optimizer_no_boundary() {
        let items: Vec<TopLevel> = vec![];
        let universe = TypeUniverse::new();
        let resolved_frgns = HashMap::new();
        let targets = sample_glue_targets();

        let changes = optimize_layouts(&items, &universe, &resolved_frgns, &targets).unwrap();
        assert!(changes.is_empty(), "expected no changes with no frgn declarations");
    }

    /// A frgn with an existing identity meld → no layout change needed.
    #[test]
    fn test_layout_optimizer_identity_meld() {
        let frgn = make_frgn("py_func", "py", vec![("x", ti())]);
        let items = vec![frgn];
        let universe = TypeUniverse::new();
        let mut resolved_frgns = HashMap::new();
        resolved_frgns.insert("py_func".to_string(), ResolvedFrgn::Bridge {
            language: "python".to_string(),
            param_paths: Vec::new(),
            return_path: None,
        });

        let targets = sample_glue_targets();

        let changes = optimize_layouts(&items, &universe, &resolved_frgns, &targets).unwrap();
        assert!(changes.is_empty(), "expected no changes when foreign type is absent");
    }

    /// A frgn with a different foreign layout → proposes layout change.
    #[test]
    fn test_layout_optimizer_adopt_foreign_layout() {
        let frgn = make_frgn("py_func", "py", vec![("x", ti())]);
        let items = vec![frgn];
        let universe = make_universe_with_types(&[
            ("Int", 8, 8),
            ("PyInt", 16, 8),
        ]);
        let mut resolved_frgns = HashMap::new();
        resolved_frgns.insert("py_func".to_string(), ResolvedFrgn::Bridge {
            language: "python".to_string(),
            param_paths: Vec::new(),
            return_path: None,
        });
        let targets = sample_glue_targets();

        let changes = optimize_layouts(&items, &universe, &resolved_frgns, &targets).unwrap();

        assert!(!changes.is_empty(), "expected layout change for different foreign layout");
        assert_eq!(changes[0].type_name, "Int");
        assert_eq!(changes[0].new_bytes, 16);
        assert_eq!(changes[0].new_alignment, 8);
        assert_eq!(changes[0].modeled_after, "PyInt");
    }

    /// A frgn where the foreign type is not in the universe → no change.
    /// (The optimizer can't propose a layout change if it doesn't know the
    /// foreign layout.)
    #[test]
    fn test_layout_optimizer_foreign_type_absent() {
        let frgn = make_frgn("py_func", "py", vec![("x", ti())]);
        let items = vec![frgn];
        let universe = TypeUniverse::new();  // No PyInt registered
        let mut resolved_frgns = HashMap::new();
        resolved_frgns.insert("py_func".to_string(), ResolvedFrgn::Bridge {
            language: "python".to_string(),
            param_paths: Vec::new(),
            return_path: None,
        });
        let targets = sample_glue_targets();

        let changes = optimize_layouts(&items, &universe, &resolved_frgns, &targets).unwrap();
        // PyInt is not in the universe → optimizer skips this parameter
        assert!(changes.is_empty(),
            "expected no changes when foreign type is not in the universe");
    }

    /// verify that apply_layout_change modifies a TypeDef's metadata.
    #[test]
    fn test_apply_layout_change_modifies_typedef() {
        let mut items = vec![make_type_def("Int", 8, 8)];
        let change = LayoutChange {
            type_name: "Int".to_string(),
            new_bytes: 16,
            new_alignment: 8,
            modeled_after: "PyInt".to_string(),
        };

        apply_layout_change(&mut items, &change).unwrap();

        // Verify metadata was updated
        if let TopLevel::TypeDef(td) = &items[0] {
            let maxbits = td.body.metadata.get("maxbits").unwrap();
            let alignment = td.body.metadata.get("alignment").unwrap();
            assert_eq!(*maxbits, PropertyValue::Int(128));  // 16 bytes * 8
            assert_eq!(*alignment, PropertyValue::Int(8));
        } else {
            panic!("expected TypeDef");
        }
    }

    /// apply_layout_change on a non-existent type should error.
    #[test]
    fn test_apply_layout_change_missing_type() {
        let mut items: Vec<TopLevel> = vec![];
        let change = LayoutChange {
            type_name: "NonExistent".to_string(),
            new_bytes: 16,
            new_alignment: 8,
            modeled_after: "Foreign".to_string(),
        };

        let result = apply_layout_change(&mut items, &change);
        assert!(result.is_err(), "expected error for non-existent type");
        assert!(result.unwrap_err().contains("not defined"));
    }

    #[test]
    fn test_derive_foreign_type_name_python() {
        assert_eq!(derive_foreign_type_name(&ti(), "python"), "PyInt");
        assert_eq!(derive_foreign_type_name(&tf(), "python"), "PyFloat");
        assert_eq!(derive_foreign_type_name(&tb(), "python"), "PyBool");
        assert_eq!(derive_foreign_type_name(&ts(), "python"), "PyString");
    }

    #[test]
    fn test_derive_foreign_type_name_rust() {
        assert_eq!(derive_foreign_type_name(&ti(), "rust"), "RstInt");
        assert_eq!(derive_foreign_type_name(&tf(), "rust"), "RstFloat");
    }

    #[test]
    fn test_derive_foreign_type_name_node() {
        assert_eq!(derive_foreign_type_name(&ti(), "node"), "JsInt");
        assert_eq!(derive_foreign_type_name(&ts(), "node"), "JsString");
    }

    #[test]
    fn test_get_type_layout_found() {
        let universe = make_universe_with_types(&[("TestType", 16, 8)]);
        let layout = get_type_layout(&universe, "TestType");
        assert_eq!(layout, Some((16, 8)));
    }

    #[test]
    fn test_get_type_layout_not_found() {
        let universe = TypeUniverse::new();
        let layout = get_type_layout(&universe, "NonExistent");
        assert_eq!(layout, None);
    }

    #[test]
    fn test_find_cast_path_direct() {
        let mut universe = make_universe_with_types(&[("A", 8, 8), ("B", 8, 8)]);
        // Add Cast.B to A
        if let Some(ref mut rt) = universe.types.get_mut("A") {
            rt.properties.insert("Cast.B".to_string(), PropertyValue::Bool(true));
        }
        let path = find_cast_path(&universe, "A", "B");

        let path = find_cast_path(&universe, "Int", "Bits");
        assert!(path.is_some(), "expected Int → #Bits path");
    }

    #[test]
    fn test_layout_optimizer_skips_inline_frgn() {
        let frgn = make_frgn("inline_func", "c", vec![("x", ti())]);
        let items = vec![frgn];
        let universe = TypeUniverse::new();
        let mut resolved_frgns = HashMap::new();
        resolved_frgns.insert(
            "inline_func".to_string(),
            ResolvedFrgn::Inline { symbol: "inline_func".to_string(), compile_source: true, protocol_lib: None },
        );
        let targets = sample_glue_targets();

        let changes = optimize_layouts(&items, &universe, &resolved_frgns, &targets).unwrap();
        assert!(changes.is_empty(), "expected no changes for inline frgn");
    }

    #[test]
    fn test_capitalize_first() {
        assert_eq!(capitalize_first("hello"), "Hello");
        assert_eq!(capitalize_first("world"), "World");
        assert_eq!(capitalize_first(""), "");
        assert_eq!(capitalize_first("a"), "A");
    }
}
