// GLUE Bridge Unit Tests
//
// 2026-07-22: Tests protocol chain computation, bridge codegen helpers,
// and protocol path resolution using the TOML-based GLUE registry.

use std::collections::HashMap;
use std::path::Path;

use briev_compiler::analysis::frgn_dispatch::{
    compute_protocol_path, resolve_single_frgn, ResolvedFrgn, TransformKind,
};
use briev_compiler::ast::top::{ForeignBinding, ForeignTarget, FromSpec};
use briev_compiler::ast::Type;
use briev_compiler::glue::config::{find_language_by_extension, load_glue_config, GlueTarget};

fn sample_glue_targets() -> HashMap<String, GlueTarget> {
    HashMap::from([
        (
            "python".to_string(),
            GlueTarget {
                language: "python".to_string(),
                types_module: std::path::PathBuf::from("glue/python/types.bv"),
                extension: "py".to_string(),
                bridge_kind: "native_module".to_string(),
                calling_convention: "c_abi".to_string(),
                module_init: false,
                protocols: HashMap::new(),
                templates: HashMap::new(),
            conversions: briev_compiler::glue::config::Conversions::default(),
            state: briev_compiler::glue::config::StateAbi::default(),
            param_decl: "{name}: {type}".to_string(),
            fn_param_decl: "{name}: {type}".to_string(),
            native_include_cmd: None,
            native_suffix: None,
            native_suffix_cmd: None,
            native_link_cmd: None,
            native_cc: None,
            native_prefix: None,
            },
        ),
        (
            "rust".to_string(),
            GlueTarget {
                language: "rust".to_string(),
                types_module: std::path::PathBuf::from("glue/rust/types.bv"),
                extension: "rs".to_string(),
                bridge_kind: "extern_c_crate".to_string(),
                calling_convention: "lto".to_string(),
                module_init: false,
                protocols: HashMap::new(),
                templates: HashMap::new(),
            conversions: briev_compiler::glue::config::Conversions::default(),
            state: briev_compiler::glue::config::StateAbi::default(),
            param_decl: "{name}: {type}".to_string(),
            fn_param_decl: "{name}: {type}".to_string(),
            native_include_cmd: None,
            native_suffix: None,
            native_suffix_cmd: None,
            native_link_cmd: None,
            native_cc: None,
            native_prefix: None,
            },
        ),
    ])
}

fn make_frgn(name: &str, ext: &str, as_name: Option<&str>) -> ForeignBinding {
    ForeignBinding::new(
        name.to_string(),
        as_name.map(|s| s.to_string()),
        FromSpec::Literal(std::path::PathBuf::from(format!("lib.{}", ext))),
        ForeignTarget::Native,
    )
}

// ── Protocol Path Tests ────────────────────────────────────────────────

#[test]
fn test_compute_protocol_path_identity() {
    let int_type = Type::int();
    let path = compute_protocol_path(&int_type, &int_type, None).unwrap();
    assert_eq!(path.len(), 1, "identity path should have 1 step");
    assert!(matches!(path[0].kind, TransformKind::Identity));
}

#[test]
fn test_compute_protocol_path_bitcast() {
    let a = Type::Custom("A".to_string());
    let b = Type::Custom("B".to_string());
    let path = compute_protocol_path(&a, &b, None).unwrap();
    assert_eq!(path.len(), 1, "bitcast path should have 1 step");
    assert!(matches!(path[0].kind, TransformKind::Bitcast));
}

// ── Extension-to-Language Resolution ────────────────────────────────────

#[test]
fn test_find_language_by_extension_python() {
    let targets = sample_glue_targets();
    let found = find_language_by_extension(&targets, "py");
    assert!(found.is_some(), ".py should map to a language target");
    assert_eq!(found.unwrap().language, "python");
}

#[test]
fn test_find_language_by_extension_rust() {
    let targets = sample_glue_targets();
    let found = find_language_by_extension(&targets, "rs");
    assert!(found.is_some(), ".rs should map to a language target");
    assert_eq!(found.unwrap().language, "rust");
}

#[test]
fn test_find_language_by_extension_unknown() {
    let targets = sample_glue_targets();
    let found = find_language_by_extension(&targets, "kt");
    assert!(found.is_none(), ".kt should not be in any language target");
}

// ── Resolve Single Frgn ─────────────────────────────────────────────────

#[test]
fn test_resolve_single_frgn_inline_c() {
    let fb = make_frgn("my_func", "c", None);
    let targets = sample_glue_targets();
    let result =
        resolve_single_frgn(&fb, "c", &targets, briev_compiler::target::BackendKind::Llvm, None)
            .unwrap();
    match result {
        ResolvedFrgn::Inline { symbol, compile_source, .. } => {
            assert_eq!(symbol, "my_func");
            assert!(compile_source);
        }
        other => panic!("Expected Inline, got {:?}", other),
    }
}

#[test]
fn test_resolve_single_frgn_bridge_python() {
    let fb = make_frgn("py_func", "py", None);
    let targets = sample_glue_targets();
    let result =
        resolve_single_frgn(&fb, "py", &targets, briev_compiler::target::BackendKind::Llvm, None)
            .unwrap();
    match result {
        ResolvedFrgn::Bridge { language, .. } => {
            assert_eq!(language, "python");
        }
        other => panic!("Expected Bridge, got {:?}", other),
    }
}

#[test]
fn test_resolve_single_frgn_unsupported() {
    let fb = make_frgn("kotlin_func", "kt", None);
    let targets = sample_glue_targets();
    let result =
        resolve_single_frgn(&fb, "kt", &targets, briev_compiler::target::BackendKind::Llvm, None)
            .unwrap();
    match result {
        ResolvedFrgn::Unsupported(msg) => {
            assert!(msg.contains("kt"), "msg should mention extension: {}", msg);
        }
        other => panic!("Expected Unsupported, got {:?}", other),
    }
}

#[test]
fn test_resolve_single_frgn_empty_extension() {
    let fb = make_frgn("no_ext", "", None);
    let targets = sample_glue_targets();
    let result =
        resolve_single_frgn(&fb, "", &targets, briev_compiler::target::BackendKind::Llvm, None).unwrap();
    match result {
        ResolvedFrgn::Unsupported(msg) => {
            assert!(msg.contains("no file extension"), "msg: {}", msg);
        }
        other => panic!("Expected Unsupported, got {:?}", other),
    }
}

#[test]
fn test_resolve_single_frgn_with_as() {
    // frgn <foreign_symbol> ... as <briev_name>: foreign_name is the linker
    // symbol; the `as` clause renames the Briv-side callsite.
    let fb = make_frgn("foreign_sym", "c", Some("briev_name"));
    let targets = sample_glue_targets();
    let result =
        resolve_single_frgn(&fb, "c", &targets, briev_compiler::target::BackendKind::Llvm, None)
            .unwrap();
    match result {
        ResolvedFrgn::Inline { symbol, .. } => {
            assert_eq!(symbol, "foreign_sym");
        }
        other => panic!("Expected Inline, got {:?}", other),
    }
}

#[test]
fn test_resolve_single_frgn_native_so() {
    let fb = make_frgn("native_fn", "so", None);
    let targets = sample_glue_targets();
    let result =
        resolve_single_frgn(&fb, "so", &targets, briev_compiler::target::BackendKind::Llvm, None)
            .unwrap();
    match result {
        ResolvedFrgn::Inline { symbol, compile_source, .. } => {
            assert_eq!(symbol, "native_fn");
            assert!(!compile_source);
        }
        other => panic!("Expected Inline, got {:?}", other),
    }
}

// ── TOML Config Loading ─────────────────────────────────────────────────

#[test]
fn test_load_glue_config_shiped() {
    let config = load_glue_config(None).expect("should load built-in config/glue.dbvl");
    assert!(config.contains_key("python"), "should have python entry");
    assert!(config.contains_key("rust"), "should have rust entry");
    let python = config.get("python").unwrap();
    assert_eq!(python.extension, "so");
    assert_eq!(python.calling_convention, "c_abi");
    assert_eq!(
        python.protocols.get("Int").unwrap().c_abi.as_deref(),
        Some("ctypes.c_int64")
    );
}

#[test]
fn test_load_glue_config_custom_path() {
    // 2026-08-04 (glue-host): glue configs moved to per-language folders
    // (lib/glue/<lang>/glue.dbvl); the old config/glue.dbvl monolith is gone.
    // A custom-path load points at one language folder's dbvl.
    let custom = Path::new("lib/glue/python/glue.dbv");
    let config = load_glue_config(Some(custom)).expect("should load from custom path");
    assert!(config.contains_key("python"));
}
