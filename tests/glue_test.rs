// GLUE Integration Tests
//
// 2026-08-03 (plan 2026-08-03-glue-folders-node-bridge): tests the per-language
// GLUE registry (lib/glue/<lang>/glue.dbvl, referenced as config). These run
// as `cargo test --test glue_test` (separate binary from lib tests). Only
// uses `pub` items from the library crate.

use briev_compiler::glue::config::find_language_by_extension;
use briev_compiler::glue::config::load_glue_config;
use briev_compiler::glue::export::extract_bridge_info;
use briev_compiler::glue::link::generate_bridge_bv;
use briev_compiler::glue::link::{ForeignFunction, LinkResult};
use briev_compiler::lexer::Token;
use briev_compiler::parser::Parser;
use std::path::Path;

// ============ TOML Registry ============

#[test]
fn test_glue_dbvl_parses() {
    let config = load_glue_config(None).expect("config/glue.dbvl should load");
    assert!(
        config.contains_key("python"),
        "config/glue.dbvl should have python entry"
    );
    assert!(
        config.contains_key("rust"),
        "config/glue.dbvl should have rust entry"
    );
}

#[test]
fn test_glue_dbvl_custom_path() {
    // A single per-language config file loads on its own.
    let config = load_glue_config(Some(Path::new("lib/glue/python/glue.dbv")))
        .expect("custom path should load");
    assert!(config.contains_key("python"));
}

// ============ Language Lookup ============

#[test]
fn test_find_language_by_extension_rust() {
    let config = load_glue_config(None).unwrap();
    let found = find_language_by_extension(&config, "rs");
    assert!(found.is_some(), "rust adapter should be found");
    let adapter = found.unwrap();
    assert_eq!(adapter.language, "rust");
    assert_eq!(adapter.extension, "rs");
    assert_eq!(adapter.calling_convention, "lto");
}

#[test]
fn test_find_language_by_extension_python() {
    let config = load_glue_config(None).unwrap();
    let found = find_language_by_extension(&config, "so");
    assert!(found.is_some(), "python adapter should be found");
    let adapter = found.unwrap();
    assert_eq!(adapter.language, "python");
    assert_eq!(adapter.extension, "so");
    assert_eq!(adapter.calling_convention, "c_abi");
    assert_eq!(
        adapter.protocols.get("#Int").unwrap().c_abi.as_deref(),
        Some("ctypes.c_int64")
    );
}

#[test]
fn test_find_language_by_extension_node() {
    let config = load_glue_config(None).unwrap();
    // Both [node] and [web] register extension "mjs", so the lookup may
    // return either — assert the found target is one of them (deterministic;
    // the duplicate-extension ambiguity is resolved in the config migration).
    let found = find_language_by_extension(&config, "mjs");
    assert!(found.is_some(), "an mjs target should be found");
    let adapter = found.unwrap();
    assert!(adapter.language == "node" || adapter.language == "web");
    assert_eq!(adapter.extension, "mjs");
    assert!(config.values().any(|t| t.language == "node" && t.extension == "mjs"));
}

#[test]
fn test_find_language_by_extension_nonexistent() {
    let config = load_glue_config(None).unwrap();
    let found = find_language_by_extension(&config, "cobol");
    assert!(found.is_none(), "cobol adapter should not exist");
}

// ============ Bridge Info Extraction ============

fn parse_bv(source: &str) -> Vec<briev_compiler::ast::TopLevel> {
    use logos::Logos;
    let tokens: Vec<(Token, std::ops::Range<usize>)> = Token::lexer(source)
        .map(|r| (r.unwrap(), 0..0))
        .collect();
    let mut parser = Parser::new(tokens, source);
    parser.parse_program().expect("should parse")
}

#[test]
fn test_extract_exports_from_source() {
    let source = r#"
        export defn add(a: Int, b: Int) -> Int { term a + b; };
        export defn multiply(a: Int, b: Int) -> Int { term a * b; };
    "#;
    let items = parse_bv(source);
    let info = extract_bridge_info(&items, "test-bridge", None);
    assert_eq!(info.name, "test-bridge");
    assert_eq!(info.exports.len(), 2, "should find 2 export functions");
    assert_eq!(info.frgns.len(), 0, "no frgn declarations");
    let names: Vec<&str> = info.exports.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"add"), "should have 'add' export");
    assert!(names.contains(&"multiply"), "should have 'multiply' export");
    // Check specific export fields
    let add = info.exports.iter().find(|e| e.name == "add").unwrap();
    assert_eq!(add.return_type, "Int");
    assert_eq!(add.params.len(), 2);
    assert_eq!(add.params[0].1, "Int");
    assert_eq!(add.params[1].1, "Int");
}

#[test]
fn test_extract_bridge_info_from_test_bridge() {
    let source = std::fs::read_to_string("examples/test-bridge.bv")
        .expect("test-bridge.bv should exist");
    let items = parse_bv(&source);
    let info = extract_bridge_info(&items, "test-bridge", None);
    assert_eq!(info.name, "test-bridge");
    assert!(!info.exports.is_empty(), "should find at least one export");
}

#[test]
fn test_extract_bridge_info_empty_program() {
    let items = vec![];
    let info = extract_bridge_info(&items, "empty", None);
    assert_eq!(info.name, "empty");
    assert_eq!(info.exports.len(), 0);
    assert_eq!(info.frgns.len(), 0);
}

// ============ Link Pipeline ============

#[test]
fn test_link_generate_bridge_bv() {
    let result = LinkResult {
        library_path: "test.a".to_string(),
        functions: vec![
            ForeignFunction {
                name: "sqrt".to_string(),
                symbol: "sqrt".to_string(),
                is_intrinsic: true,
                intrinsic_name: Some("sqrt".to_string()),
            },
            ForeignFunction {
                name: "custom_op".to_string(),
                symbol: "custom_op".to_string(),
                is_intrinsic: false,
                intrinsic_name: None,
            },
        ],
    };
    let output = generate_bridge_bv(&result);
    assert!(output.contains("Auto-generated bridge for test.a"));
    assert!(output.contains("intrinsic: sqrt#()"));
    assert!(output.contains("frgn custom_op(...) -> Int ;"));
}

// ============ TOML Config Locading ============

#[test]
fn test_load_glue_config_fields() {
    let config = load_glue_config(None).unwrap();
    let python = config.get("python").unwrap();
    assert_eq!(python.extension, "so");
    assert_eq!(python.bridge_kind, "native_module");
    assert_eq!(python.calling_convention, "c_abi");
    assert_eq!(
        python.protocols.get("#Float").unwrap().c_abi.as_deref(),
        Some("ctypes.c_double")
    );
}
