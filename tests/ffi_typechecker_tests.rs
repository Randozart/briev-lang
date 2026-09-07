//! FFI Typechecker Integration Tests
//!
//! Tests `frgn` declaration validation against the current GLUE
//! config-driven architecture (2026-09-07 rewrite).
//!
//! The typechecker no longer validates TOML binding files — that is the
//! backend's responsibility. What the frontend typechecker does:
//!   1. Registers return types so `term frgn_foo(x)` resolves correctly.
//!   2. Tracks optional frgn names for `^^Available` reflection.
//!   3. Catches type mismatches in code that calls foreign bindings.

use briev_compiler::errors::TypeError;
use briev_compiler::lexer::tokenize;
use briev_compiler::parser::Parser;
use briev_compiler::typechecker::check_program;
use briev_compiler::type_universe::TypeUniverse;

fn check(src: &str) -> Result<(), Vec<TypeError>> {
    let tokens = tokenize(src).unwrap();
    let mut p = Parser::new(tokens, src);
    let mut items = p.parse_program().unwrap();
    let universe = TypeUniverse::new();
    check_program(&mut items, &universe)
}

// ── frgn return type resolves through defn ──────────────────────────

#[test]
fn test_frgn_return_type_resolves() {
    let src = r#"
frgn strlen(s: String) -> Int from #System;
defn call_strlen(s: String) -> Int { term strlen(s); };
"#;
    assert!(check(src).is_ok(), "frgn return type must be visible to callers");
}

#[test]
fn test_frgn_void_return() {
    let src = r#"
frgn panic(msg: String) from #System;
"#;
    assert!(
        check(src).is_ok(),
        "void-returning frgn must typecheck"
    );
}

// ── frgn return type mismatch in defn ───────────────────────────────

#[test]
fn test_frgn_return_type_mismatch_in_defn() {
    let src = r#"
frgn strlen(s: String) -> Int from #System;
defn bad_return(s: String) -> String { term strlen(s); };
"#;
    let err = check(src).unwrap_err();
    assert!(
        err.iter().any(|e| format!("{}", e).contains("mismatch")),
        "returning Int frgn as String must error, got {:?}",
        err
    );
}

// ── colon alias ─────────────────────────────────────────────────────

#[test]
fn test_frgn_colon_alias_resolves() {
    let src = r#"
frgn local_add(a: Int, b: Int) -> Int: external_add from #System;
defn use_add(a: Int, b: Int) -> Int { term local_add(a, b); };
"#;
    assert!(
        check(src).is_ok(),
        "colon alias must resolve to briev_name"
    );
}

// ── optional frgn + ^^Available ─────────────────────────────────────

#[test]
fn test_optional_frgn_available_is_bool() {
    let src = r#"
optional frgn feature(x: Int) -> Int from #System;
let fired: Int = 0;
node go [fired == 0][fired == 1] {
    let avail: Bool = feature.^^Available;
    fired = fired + 1;
    term;
};
"#;
    assert!(
        check(src).is_ok(),
        "^^Available on optional frgn must resolve as Bool"
    );
}

#[test]
fn test_available_on_non_optional_frgn_is_error() {
    let src = r#"
frgn feature(x: Int) -> Int from #System;
let fired: Int = 0;
node go [fired == 0][fired == 1] {
    let avail: Bool = feature.^^Available;
    fired = fired + 1;
    term;
};
"#;
    let err = check(src).unwrap_err();
    assert!(
        err.iter().any(|e| format!("{}", e).contains("only on an `optional frgn`")),
        "^^Available on non-optional must error, got {:?}",
        err
    );
}

// ── multiple frgns coexist ─────────────────────────────────────────

#[test]
fn test_multiple_frgns_and_defns() {
    let src = r#"
frgn strlen(s: String) -> Int from #System;
frgn panic(msg: String) from #System;
defn safe_strlen(s: String) -> Int { term strlen(s); };
"#;
    assert!(
        check(src).is_ok(),
        "multiple frgn + defn must coexist"
    );
}
