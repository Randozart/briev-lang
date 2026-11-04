// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.3b parity: HIR boundary-ownership inference must equal the AST analysis
//! (still authoritative) for frgn seeding, export params/returns, transitive
//! call-chain propagation, state-field reads, and the lto convention.

use briev_compiler::analysis::boundary_ownership::compute_boundary_ownership as ast_compute;
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::boundary_ownership::compute_boundary_ownership as hir_compute;
use briev_compiler::hir::lower_program;
use std::collections::HashMap;

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn assert_parity(src: &str, label: &str, convention: Option<&str>) {
    let items = parse(src);
    let conv = |_: &str| convention.map(|c| c.to_string());
    let ast = ast_compute(&items, None, &conv);
    let hir = hir_compute(&lower_program(&items), None, &conv);
    assert_eq!(ast.frgns, hir.frgns, "{label}: frgn ownership");
    assert_eq!(ast.exports, hir.exports, "{label}: export ownership");
}

#[test]
fn curated_boundary_ownership_parity() {
    // frgn C-string return propagates ZeroCopy into the export.
    assert_parity(
        r#"
frgn host_get() -> CStr : host_get from "host";
type CStr: String<C_String> { };
export defn ship() -> CStr { term host_get(); };
"#,
        "frgn-ret",
        Some("c_abi"),
    );

    // Export C-string parameter crosses host→Briev (Borrowed); Int return Value.
    assert_parity(
        r#"
type CStr: String<C_String> { };
export defn take(name: CStr) -> Int { term 0; };
"#,
        "param",
        Some("c_abi"),
    );

    // lto has no boundary.
    assert_parity(
        r#"
frgn host_get() -> CStr : host_get from "host";
type CStr: String<C_String> { };
export defn ship() -> CStr { term host_get(); };
"#,
        "lto",
        Some("lto"),
    );

    // A state-field read yields an Owned value.
    assert_parity(
        r#"
let g: Int = 0;
export defn read() -> Int { term g; };
"#,
        "state-read",
        Some("c_abi"),
    );

    // Export→export transitive return propagation.
    assert_parity(
        r#"
export defn inner() -> String { term ""; };
export defn outer() -> String { term inner(); };
"#,
        "chain",
        Some("c_abi"),
    );
}

#[test]
fn boundary_ownership_maps_are_deterministic() {
    let src = r#"
frgn host_get() -> CStr : host_get from "host";
type CStr: String<C_String> { };
export defn ship() -> CStr { term host_get(); };
"#;
    let items = parse(src);
    let conv = |_: &str| Some("c_abi".to_string());
    let a = hir_compute(&lower_program(&items), None, &conv);
    let b = hir_compute(&lower_program(&items), None, &conv);
    assert_eq!(a.exports, b.exports);
    assert_eq!(a.frgns, b.frgns);
    let _: HashMap<_, _> = a.exports;
}
