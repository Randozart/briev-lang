// ── Frgn Dispatch Resolution ──────────────────────────────────────────
// 2026-07-22: Resolved during the main compilation pass (before codegen),
// not inside the backend. The backend receives a ResolvedFrgn and emits
// the appropriate IR without re-implementing dispatch logic.
//
// Why resolve pre-backend:
//   1. The dispatch decision depends on the protocol graph (type_universe),
//      the GLUE registry, and the backend's capabilities — all available
//      at compile time.
//   2. Backends should not reimplement extension matching or BFS.
//   3. A single error point for "no bridge available" is cleaner than
//      per-backend error messages.
//
// Why NOT resolve in the backend:
//   The backend already knows its own capabilities. The ResolvedFrgn is
//   the intersection of "what the type system says" and "what the backend
//   can do." The backend still validates that it can handle the result.

use std::collections::HashMap;

use crate::ast::top::{ForeignBinding, FromSpec};
use crate::glue::config::{find_language_by_extension, GlueTarget};
use crate::target::BackendKind;

/// The dispatch strategy for a single frgn declaration.
///
/// 2026-07-22: Determined during the main compilation pass before any
/// backend runs. The backend receives this and emits the appropriate IR.
#[derive(Debug, Clone)]
pub enum ResolvedFrgn {
    /// Backend inlines directly (compile/link the source, call the symbol)
    Inline {
        /// The foreign symbol name (from `as` or briev_name)
        symbol: String,
        /// If true, the backend should compile this source to .o first
        compile_source: bool,
        /// 2026-07-26: Protocol library name for from #System.
        /// `#System` is the sole protocol — any other hashword produces a
        /// compile error. None = resolved from a normal file path or
        /// compiler registry. Some(lib) = link with -l<lib>.
        protocol_lib: Option<String>,
    },
    /// Route through the GLUE bridge
    Bridge {
        /// Language identifier
        language: String,
        /// Protocol transform chain for each parameter
        param_paths: Vec<ProtocolStep>,
        /// Protocol transform chain for the return value
        return_path: Option<ProtocolStep>,
    },
    /// Not supported by this backend
    Unsupported(String),
}

/// A single step in a protocol transform chain.
///
/// 2026-07-22: Describes how to go from one type representation to another
/// at the FFI boundary. Multiple steps form a path from Briev type to
/// foreign type (or vice versa).
#[derive(Debug, Clone)]
pub struct ProtocolStep {
    /// The source type in the chain
    pub source: crate::ast::Type,
    /// The target type in the chain
    pub target: crate::ast::Type,
    /// The kind of transform needed
    pub kind: TransformKind,
}

/// Cost category for a protocol transform.
///
/// 2026-07-22: Used by compute_protocol_path() to find the cheapest path.
/// The categories form a partial order: Identity < Bitcast < MeldShuffle < ProtocolTransform.
#[derive(Debug, Clone)]
pub enum TransformKind {
    /// No transform needed — types are structurally identical
    Identity,
    /// Meld shuffle — bit permutation, field reordering
    MeldShuffle,
    /// Protocol transform — CastTo/CastFrom with actual encoding work
    ProtocolTransform(String),
    /// Raw bitcast — implicit Cast(#Bits)
    Bitcast,
}

/// Resolve the dispatch strategy for a single frgn declaration.
///
/// 2026-07-22: Given the frgn declaration, its file extension, the GLUE
/// registry, and the backend kind, determines whether the call can be
/// inlined, needs a GLUE bridge, or is unsupported.
pub fn resolve_single_frgn(
    fb: &ForeignBinding,
    ext: &str,
    glue_targets: &HashMap<String, GlueTarget>,
    backend: BackendKind,
    universe: Option<&crate::type_universe::TypeUniverse>,
) -> Result<ResolvedFrgn, String> {
    // 2026-07-26: Resolve protocol-based FFI (from #System, from #Web).
    // #System resolves to a system library for linking.
    // #Web routes through the GLUE web bridge (wasm_runtime).
    // This must come before the extension check because FromSpec::Protocol
    // has no file extension — it resolves to a system library or GLUE bridge.
    if let FromSpec::Protocol(proto) = &fb.from {
        // 2026-07-26: #Web protocol — route through GLUE web bridge.
        // The web target provides wasm_runtime import stubs (handle table,
        // DOM operations, canvas context). No library linking needed.
        if proto == "#Web" {
            let web_lang = find_language_by_extension(glue_targets, "mjs");
            if let Some(target) = web_lang {
                let param_paths: Vec<ProtocolStep> = fb.inputs.iter()
                    .map(|(_, briev_type)| {
                        let foreign_type = lookup_foreign_type(briev_type, &target.protocols, universe);
                        compute_protocol_path(briev_type, &foreign_type, universe)
                            .and_then(|steps| steps.into_iter().next().ok_or_else(|| "empty path".to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let return_path: Option<ProtocolStep> = fb.success_output.first()
                    .and_then(|(_, ty)| {
                        let foreign_type = lookup_foreign_type(ty, &target.protocols, universe);
                        compute_protocol_path(ty, &foreign_type, universe).ok()?.into_iter().next()
                    });
                return Ok(ResolvedFrgn::Bridge {
                    language: target.language.clone(),
                    param_paths,
                    return_path,
                });
            }
        }
        // 2026-07-26: #System protocol — resolve to a system library.
        let protocol_config = crate::target::ProtocolConfig::load();
        let default_triple = "x86_64-linux";
        let lib = protocol_config.resolve(default_triple, proto).map_err(|e| {
            format!(
                "frgn '{}': {}",
                fb.effective_briev_name(),
                e
            )
        })?;
        return Ok(ResolvedFrgn::Inline {
            symbol: fb.foreign_name.clone(),
            compile_source: false,
            protocol_lib: lib.map(|s| s.to_string()),
        });
    }

    // 2026-07-26: Resolve #Link<name> — direct linker directive.
    // No protocol config lookup needed; the library name IS the linker flag.
    if let FromSpec::Linked(lib) = &fb.from {
        return Ok(ResolvedFrgn::Inline {
            symbol: fb.foreign_name.clone(),
            compile_source: false,
            protocol_lib: Some(lib.clone()),
        });
    }

    // 2026-07-22: Empty extension means the foreign path has no known type —
    // treat as unsupported with a clear error message.
    if ext.is_empty() {
        return Ok(ResolvedFrgn::Unsupported(format!(
            "frgn '{}' has no file extension in its 'from' path. \
             Add a file extension so the compiler can determine the dispatch strategy",
            fb.effective_briev_name()
        )));
    }

    // 2026-07-22: First check if the extension maps to a GLUE language target.
    // If found, this is a bridge candidate.
    let language = find_language_by_extension(glue_targets, ext);

    // 2026-07-22: Check if the extension is inlineable for this backend.
    // Inlineable means the backend can compile the source directly and
    // link the object code. Currently only .c/.cpp for LLVM backend.
    if backend == BackendKind::Llvm && matches!(ext, "c" | "cpp" | "cxx" | "rs") {
        let symbol = fb.foreign_name.clone();
        return Ok(ResolvedFrgn::Inline {
            symbol,
            compile_source: true,
            protocol_lib: None,
        });
    }

    // 2026-07-22: If a GLUE language target exists, this is a bridge call.
    if let Some(target) = language {
        // 2026-07-22: Compute protocol transform for each parameter (one step each),
        // using the target's protocol mapping to derive the foreign type.
        let param_paths: Vec<ProtocolStep> = fb.inputs.iter()
            .map(|(_, briev_type)| {
                let foreign_type = lookup_foreign_type(briev_type, &target.protocols, universe);
                compute_protocol_path(briev_type, &foreign_type, universe)
                    .and_then(|steps| steps.into_iter().next().ok_or_else(|| "empty path".to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let return_path: Option<ProtocolStep> = fb.success_output.first()
            .and_then(|(_, ty)| {
                let foreign_type = lookup_foreign_type(ty, &target.protocols, universe);
                compute_protocol_path(ty, &foreign_type, universe).ok()?.into_iter().next()
            });
        return Ok(ResolvedFrgn::Bridge {
            language: target.language.clone(),
            param_paths,
            return_path,
        });
    }

    // 2026-07-22: Native (Metropolitan) targets always inline since they
    // are already compiled. The backend calls them directly.
    if ext == "native" || ext == "o" || ext == "so" || ext == "a" {
        let symbol = fb.foreign_name.clone();
        return Ok(ResolvedFrgn::Inline {
            symbol,
            compile_source: false,
            protocol_lib: None,
        });
    }

    Ok(ResolvedFrgn::Unsupported(format!(
        "frgn '{}' from '{}': extension '.{}' is not supported by the {} backend. \
         Add a GLUE registry entry in config/glue.dbv or use a supported extension (.c, .rs, .py, .js, .mjs)",
        fb.effective_briev_name(),
        ext,
        ext,
        match backend {
            BackendKind::Llvm => "LLVM",
            BackendKind::Circt => "CIRCT",
            BackendKind::Electronics => "Electronics",
            BackendKind::Webstack => "Webstack",
            BackendKind::Gpu => "GPU",
            BackendKind::Spirv => "SPIR-V",
            BackendKind::Vm => "VM",
            BackendKind::Ptx => "PTX",
            BackendKind::Bad => "Briev Assembly Dialect",
        }
    )))
}

/// Compute the protocol path between two types for a frgn boundary.
///
/// 2026-07-22: Uses the existing BFS in find_cast_path() + meld lookup
/// to determine how to transform a Briev type to/from a foreign type.
/// Returns the shortest path by cost.
///
/// Stub: Returns an identity path. Full implementation in Phase 3.
pub fn compute_protocol_path(
    briev_type: &crate::ast::Type,
    _foreign_type: &crate::ast::Type,
    universe: Option<&crate::type_universe::TypeUniverse>,
) -> Result<Vec<ProtocolStep>, String> {
    // 2026-07-22: If types are structurally identical, return identity.
    if briev_type == _foreign_type {
        return Ok(vec![ProtocolStep {
            source: briev_type.clone(),
            target: _foreign_type.clone(),
            kind: TransformKind::Identity,
        }]);
    }

    // 2026-07-22: Use BFS via find_cast_path if universe is available.
    if let Some(u) = universe {
        let briev_key = type_to_key(briev_type);
        let foreign_key = type_to_key(_foreign_type);
        if let Some(path) = crate::analysis::layout_optimizer::find_cast_path(u, &briev_key, &foreign_key) {
            let steps = path_to_protocol_steps(&path);
            if !steps.is_empty() {
                return Ok(steps);
            }
        }
    }

    // 2026-07-22: Fallback: return a bitcast path (Cast(#Bits)).
    Ok(vec![ProtocolStep {
        source: briev_type.clone(),
        target: _foreign_type.clone(),
        kind: TransformKind::Bitcast,
    }])
}

/// Look up the foreign protocol category for a Briev type, then map it
/// to a foreign type via the target's protocol mapping.
/// Falls back to the Briev type's universe key if no protocol exists.
fn lookup_foreign_type(
    briev_type: &crate::ast::Type,
    protocols: &std::collections::HashMap<String, crate::glue::config::ProtocolEntry>,
    universe: Option<&crate::type_universe::TypeUniverse>,
) -> crate::ast::Type {
    // Find the protocol category that this Briev type participates in
    if let Some(u) = universe {
        if let Some(key) = briev_type.universe_key() {
            if let Some(rt) = u.get(key) {
                // Look for a CastTo property that points to a protocol category
                for prop_key in rt.properties.keys() {
                    if let Some(cat) = prop_key.strip_prefix("Cast.") {
                        // 2026-09-11 (Phase A6/A4): protocol keys and the
                        // returned category are BARE — the hashword spellings
                        // are retired.
                        if protocols.contains_key(cat) {
                            return crate::ast::Type::Custom(cat.to_string());
                        }
                    }
                }
            }
        }
    }
    // Fallback: derive from the type's name
    match briev_type {
        crate::ast::Type::Custom(name) => {
            if protocols.contains_key(name) {
                crate::ast::Type::Custom(name.clone())
            } else {
                briev_type.clone()
            }
        }
        _ => briev_type.clone(),
    }
}

/// Convert a Type to a string key for use with find_cast_path BFS.
fn type_to_key(ty: &crate::ast::Type) -> String {
    match ty {
        crate::ast::Type::Custom(name) => name.clone(),
        crate::ast::Type::Applied(name, _) => name.clone(),
        crate::ast::Type::Void => "Void".to_string(),
        crate::ast::Type::Ptr(_) => "Ptr".to_string(),
        crate::ast::Type::Bits(w) => format!("Bits({})", w),
        crate::ast::Type::Tuple(_) => "Tuple".to_string(),
        crate::ast::Type::TypeVar(name) => name.clone(),
        _ => format!("{:?}", ty),
    }
}

/// Convert a path of type names from find_cast_path BFS into ProtocolStep entries.
fn path_to_protocol_steps(path: &[String]) -> Vec<ProtocolStep> {
    if path.len() < 2 {
        return vec![];
    }
    let mut steps = Vec::new();
    for pair in path.windows(2) {
        let kind = if pair[0] == pair[1] {
            TransformKind::Identity
        } else if pair[0] == "Bits" || pair[1] == "Bits" {
            TransformKind::Bitcast
        } else {
            TransformKind::ProtocolTransform(pair[1].clone())
        };
        steps.push(ProtocolStep {
            source: crate::ast::Type::Custom(pair[0].clone()),
            target: crate::ast::Type::Custom(pair[1].clone()),
            kind,
        });
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::top::{ForeignTarget, FromSpec};
    use std::path::PathBuf;

    fn make_frgn(name: &str, ext: &str) -> ForeignBinding {
        ForeignBinding::new(
            name.to_string(),
            None,
            FromSpec::Literal(PathBuf::from(format!("lib.{}", ext))),
            ForeignTarget::Native,
        )
    }

    fn make_frgn_with_as(name: &str, as_name: &str, ext: &str) -> ForeignBinding {
        ForeignBinding::new(
            name.to_string(),
            Some(as_name.to_string()),
            FromSpec::Literal(PathBuf::from(format!("lib.{}", ext))),
            ForeignTarget::Native,
        )
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

    #[test]
    fn test_resolve_single_frgn_inline_c() {
        let fb = make_frgn("my_func", "c");
        let targets = sample_glue_targets();
        let result = resolve_single_frgn(&fb, "c", &targets, BackendKind::Llvm, None).unwrap();
        match result {
            ResolvedFrgn::Inline { symbol, compile_source, .. } => {
                assert_eq!(symbol, "my_func");
                assert!(compile_source);
            }
            other => panic!("Expected Inline, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_single_frgn_inline_rs() {
        let fb = make_frgn("my_func", "rs");
        let targets = sample_glue_targets();
        let result = resolve_single_frgn(&fb, "rs", &targets, BackendKind::Llvm, None).unwrap();
        match result {
            ResolvedFrgn::Inline { symbol, .. } => {
                assert_eq!(symbol, "my_func");
            }
            other => panic!("Expected Inline, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_single_frgn_inline_with_as() {
        // foreign_name = "c_symbol", briev_name = Some("briev_alias")
        let fb = make_frgn_with_as("c_symbol", "briev_alias", "c");
        let targets = sample_glue_targets();
        let result = resolve_single_frgn(&fb, "c", &targets, BackendKind::Llvm, None).unwrap();
        match result {
            ResolvedFrgn::Inline { symbol, .. } => {
                // Backend links against the C symbol, not the Briev alias
                assert_eq!(symbol, "c_symbol");
            }
            other => panic!("Expected Inline, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_single_frgn_bridge_python() {
        let fb = make_frgn("py_func", "py");
        let targets = sample_glue_targets();
        let result = resolve_single_frgn(&fb, "py", &targets, BackendKind::Llvm, None).unwrap();
        match result {
            ResolvedFrgn::Bridge { language, .. } => {
                assert_eq!(language, "python");
            }
            other => panic!("Expected Bridge, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_single_frgn_unsupported_unknown_ext() {
        let fb = make_frgn("kotlin_func", "kt");
        let targets = sample_glue_targets();
        let result = resolve_single_frgn(&fb, "kt", &targets, BackendKind::Llvm, None).unwrap();
        match result {
            ResolvedFrgn::Unsupported(msg) => {
                assert!(msg.contains("kt"));
            }
            other => panic!("Expected Unsupported, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_single_frgn_unsupported_empty_ext() {
        let fb = make_frgn("no_ext", "");
        let targets = sample_glue_targets();
        let result = resolve_single_frgn(&fb, "", &targets, BackendKind::Llvm, None).unwrap();
        match result {
            ResolvedFrgn::Unsupported(msg) => {
                assert!(msg.contains("no file extension"));
            }
            other => panic!("Expected Unsupported, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_single_frgn_native_object() {
        let fb = make_frgn("native_fn", "so");
        let targets = sample_glue_targets();
        let result = resolve_single_frgn(&fb, "so", &targets, BackendKind::Llvm, None).unwrap();
        match result {
            ResolvedFrgn::Inline { symbol, compile_source, .. } => {
                assert_eq!(symbol, "native_fn");
                assert!(!compile_source);
            }
            other => panic!("Expected Inline, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_single_frgn_protocol_system() {
        // 2026-07-26: from #System should resolve to Inline with protocol_lib
        let fb = ForeignBinding::new(
            "printf".to_string(),
            None,
            FromSpec::Protocol("#System".to_string()),
            ForeignTarget::Native,
        );
        let targets = sample_glue_targets();
        let result = resolve_single_frgn(&fb, "", &targets, BackendKind::Llvm, None).unwrap();
        match result {
            ResolvedFrgn::Inline { symbol, compile_source, protocol_lib } => {
                assert_eq!(symbol, "printf");
                assert!(!compile_source, "protocol frgn should not need compilation");
                assert_eq!(protocol_lib, Some("c".to_string()),
                    "#System on x86_64-linux should resolve to 'c'");
            }
            other => panic!("Expected Inline, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_single_frgn_protocol_unknown() {
        // 2026-07-26: from #SomethingElse should produce an error
        let fb = ForeignBinding::new(
            "foo".to_string(),
            None,
            FromSpec::Protocol("#SomethingElse".to_string()),
            ForeignTarget::Native,
        );
        let targets = sample_glue_targets();
        let result = resolve_single_frgn(&fb, "", &targets, BackendKind::Llvm, None);
        let err = result.unwrap_err();
        assert!(err.contains("supported protocols"),
            "should mention supported protocols (got: '{}')", err);
    }

    #[test]
    fn test_resolve_single_frgn_link() {
        // 2026-07-26: from #Link<z> should resolve to Inline with protocol_lib = "z"
        let fb = ForeignBinding::new(
            "compress".to_string(),
            None,
            FromSpec::Linked("z".to_string()),
            ForeignTarget::Native,
        );
        let targets = sample_glue_targets();
        let result = resolve_single_frgn(&fb, "", &targets, BackendKind::Llvm, None).unwrap();
        match result {
            ResolvedFrgn::Inline { symbol, compile_source, protocol_lib } => {
                assert_eq!(symbol, "compress");
                assert!(!compile_source, "#Link frgn should not need compilation");
                assert_eq!(protocol_lib, Some("z".to_string()),
                    "#Link<z> should resolve to protocol_lib = Some('z')");
            }
            other => panic!("Expected Inline, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_protocol_path_identity() {
        let int_type = crate::ast::Type::Custom("Int".to_string());
        let result = compute_protocol_path(&int_type, &int_type, None).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].kind, TransformKind::Identity));
    }
}
