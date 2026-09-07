//! FFI Parser Integration Tests
//!
//! Tests the `frgn` declaration parser against the current GLUE
//! config-driven architecture (2026-09-07 rewrite).

use briev_compiler::lexer::tokenize;
use briev_compiler::parser::Parser;
use briev_compiler::ast::{TopLevel, ForeignBinding, FromSpec};

fn parse_frgn(src: &str) -> ForeignBinding {
    let tokens = tokenize(src).unwrap();
    let mut p = Parser::new(tokens, src);
    let tl = p.parse_top_level().unwrap();
    match tl {
        TopLevel::ForeignBinding(fb) => fb,
        other => panic!("expected ForeignBinding, got: {:?}", other),
    }
}

fn parse_items(src: &str) -> Vec<TopLevel> {
    let tokens = tokenize(src).unwrap();
    let mut p = Parser::new(tokens, src);
    p.parse_program().unwrap()
}

// ── Basic frgn declarations ──────────────────────────────────────

#[test]
fn test_frgn_literal_path() {
    let fb = parse_frgn(r#"frgn strlen(s: String) -> Int from "libc.so.6";"#);
    assert_eq!(fb.foreign_name, "strlen");
    assert_eq!(fb.inputs.len(), 1);
    assert_eq!(fb.inputs[0].0, "s");
    assert_eq!(fb.success_output.len(), 1);
    assert_eq!(fb.success_output[0].1, briev_compiler::ast::Type::int());
    match &fb.from {
        FromSpec::Literal(p) => assert_eq!(p.to_string_lossy(), "libc.so.6"),
        other => panic!("expected Literal, got: {:?}", other),
    }
}

#[test]
fn test_frgn_no_return() {
    let fb = parse_frgn(r#"frgn print(s: String) from "libio.so";"#);
    assert_eq!(fb.foreign_name, "print");
    assert!(fb.success_output.is_empty());
}

#[test]
fn test_frgn_compiler_registry() {
    let fb = parse_frgn(r#"frgn hash(data: Blob) -> Int from <xxhash.c>;"#);
    assert_eq!(fb.foreign_name, "hash");
    match &fb.from {
        FromSpec::CompilerRegistry(name) => assert_eq!(name, "xxhash.c"),
        other => panic!("expected CompilerRegistry, got: {:?}", other),
    }
}

#[test]
fn test_frgn_protocol_system() {
    let fb = parse_frgn(r#"frgn exit(code: Int) -> Void from #System;"#);
    assert_eq!(fb.foreign_name, "exit");
    match &fb.from {
        FromSpec::Protocol(p) => assert_eq!(p, "#System"),
        other => panic!("expected Protocol, got: {:?}", other),
    }
}

#[test]
fn test_frgn_link_directive() {
    let fb = parse_frgn(r#"frgn myFunc(x: Int) -> Int from #Link<mylib>;"#);
    assert_eq!(fb.foreign_name, "myFunc");
    match &fb.from {
        FromSpec::Linked(name) => assert_eq!(name, "mylib"),
        other => panic!("expected Linked, got: {:?}", other),
    }
}

// ── Colon binds external symbol ───────────────────────────────────

#[test]
fn test_frgn_colon_binds_external_symbol() {
    let fb = parse_frgn(
        r#"frgn local_add(a: Int, b: Int) -> Int: external_add from #System;"#,
    );
    assert_eq!(fb.foreign_name, "external_add", "the `:` symbol is the link name");
    assert_eq!(fb.briev_name.as_deref(), Some("local_add"), "the declaration name is the local Briev name");
    assert_eq!(fb.effective_briev_name(), "local_add");
}

// ── Variadic ──────────────────────────────────────────────────────

#[test]
fn test_frgn_variadic_named_param() {
    let fb = parse_frgn(
        r#"frgn log(format: String, variadic args: ForeignArgs) -> Void from #System;"#,
    );
    assert!(fb.is_variadic, "the `variadic` marker must be recorded");
    assert_eq!(fb.inputs.len(), 2);
    assert_eq!(fb.inputs[1].0, "args");
}

// ── Modifiers ─────────────────────────────────────────────────────

#[test]
fn test_frgn_optional_modifier() {
    // SPEC §19.3: `optional frgn` — `frgn?` was removed (2026-08-05).
    let fb = parse_frgn(r#"optional frgn dlopen(path: String) -> Ptr from #System;"#);
    assert!(fb.is_optional, "optional modifier");
}

// (2026-09-07): the `fire_forget` AST field has no syntax — `frgn!` was
// removed and nothing sets `is_fire_forget`; a test here would test an
// unexpressible field, not a behavior.

// ── Multiple items ────────────────────────────────────────────────

#[test]
fn test_frgn_with_defn() {
    let items = parse_items(
        r#"
        frgn read_file(path: String) -> Int from "libc.so";
        defn safe_read(path: String) -> Int { term read_file(path); };
        "#,
    );
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], TopLevel::ForeignBinding(_)));
    assert!(matches!(&items[1], TopLevel::Definition(_)));
}

// ── Rejected forms ────────────────────────────────────────────────

#[test]
fn test_frgn_mmio_address_rejected() {
    let result = std::panic::catch_unwind(|| {
        parse_frgn(r#"frgn reg(a: Int) -> Int from "c" @ 0x40000000;"#);
    });
    assert!(result.is_err(), "MMIO address form must be rejected");
}

// ── Error cases ───────────────────────────────────────────────────

#[test]
fn test_frgn_missing_from_rejected() {
    let tokens = tokenize(r#"frgn foo(x: Int) -> Int;"#).unwrap();
    let mut p = Parser::new(tokens, r#"frgn foo(x: Int) -> Int;"#);
    let result = p.parse_top_level();
    assert!(result.is_err(), "frgn without `from` must be rejected");
}
