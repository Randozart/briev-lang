// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.2 parity: HIR `defn_liveness` must equal the AST analysis (still
//! authoritative) for roots, closure, construction-rooted members, intrinsic
//! helpers, and the `endprogram` provider.

use briev_compiler::analysis::defn_liveness::DefnLiveness as AstLiveness;
use briev_compiler::ast::TopLevel;
use briev_compiler::hir::defn_liveness::DefnLiveness as HirLiveness;
use briev_compiler::hir::lower_program;

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn assert_parity(src: &str, label: &str) {
    let items = parse(src);
    let ast = AstLiveness::build(&items);
    let hir = HirLiveness::build(&lower_program(&items));
    assert_eq!(ast.keep_all, hir.keep_all, "{label}: keep_all parity");
    assert_eq!(ast.live, hir.live, "{label}: live-set parity");
}

#[test]
fn curated_defn_liveness_parity() {
    // A dead defn is emitted by neither pass.
    assert_parity("defn dead() -> Int { term 1; };", "dead");

    // Exports are roots.
    assert_parity("export defn live() -> Int { term 1; };", "export");

    // Reactive txns are reactor-dispatched roots.
    assert_parity(
        "defn helper() -> Int { term 1; };\nnode n [true][true] { let x: Int = helper(); term; };",
        "reactive-txn",
    );

    // Explicit call closure from a rooted export.
    assert_parity(
        "defn a() -> Int { term b(); };\ndefn b() -> Int { term 1; };\nexport defn c() -> Int { term a(); };",
        "closure",
    );

    // `endprogram` roots the exit provider.
    assert_parity(
        "defn exit_prog(x: Int) -> Int { term x; };\nnode n [true][true] { endprogram 0; };",
        "endprogram",
    );

    // Intrinsic helpers are derived from the shared table.
    assert_parity(
        "defn print_int(x: Int) -> Int { term x; };\nexport defn show() -> Int { Print#(1); term 0; };",
        "print-helper",
    );

    // Construction roots a type's behavioral members.
    assert_parity(
        "obj C { v: Int; defn m(this: C) -> Int { term 1; }; };\nexport defn make() -> C { term C { v: 1 }; };",
        "construction",
    );

    // 2026-11-04 (PR #62): runtime `.^Length` evaluates its receiver, so a
    // call under it is live (the pre-fix AST/HIR both dropped it).
    assert_parity(
        "defn receiver() -> String { term \"x\"; };\nnode n [true][true] { let len: Int = receiver().^Length; Print#(len); term; };",
        "runtime-reflect",
    );

    // 2026-11-05: `proto` cast-edge/cross-op bindings (`op Concat = ...`)
    // are emission roots; HIR must carry them or a constructed boundary type
    // loses its binding fns (caught on pp-types by the debug cross-check).
    assert_parity(include_str!("../lib/glue/c.bv"), "protocol-bindings");

    // 2026-11-05: trait defaults are never bare-name live (the backend emits
    // specialized copies); calls to them must not root them (caught by the
    // full-corpus debug sweep on byte_sink/net_stack/trait_generics).
    assert_parity(
        include_str!("../tests/tier1/byte_sink_trait_smoke.bv"),
        "trait-defaults",
    );

    // Binary `==` roots the string-equality helper when present.
    assert_parity(
        "defn briev_str_eq(a: Int, b: Int) -> Int { term 1; };\nexport defn cmp(x: Int, y: Int) -> Bool { term x == y; };",
        "eq-helper",
    );
}
