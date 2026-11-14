// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H4.1 plumbing: every backend analysis run carries the typed HIR for the
//! post-desugar AST, including the ownership releases inserted by analysis.

use briev_compiler::ast::TopLevel;
use briev_compiler::backend::analyze_program;
use briev_compiler::hir::lower_program;

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

#[test]
fn analysis_carries_hir() {
    let src = "obj A { x: Int; };\n\
               let done: Bool = false;\n\
               node main [done == false][done] {\n\
                 let a: List<A> = [A { x: 1 }];\n\
                 Print#(Count#(a));\n\
                 done = true;\n\
                 term;\n\
               };";
    let items = parse(src);
    let analysis = analyze_program(&items, false, 4, None, None);
    let direct = lower_program(&items);
    let mut before_releases = analysis.hir.clone();
    if briev_compiler::backend::hir_emit_enabled() {
        let main = before_releases.callables.iter_mut().find(|c| c.name == "main").unwrap();
        assert!(main.body.statements.iter().any(|s| matches!(
            &s.kind, briev_compiler::hir::stmt::HirStmtKind::ReleaseOwned { name, .. } if name == "a"
        )), "analysis must release the local list");
        main.body.statements.retain(|s| !matches!(
            s.kind, briev_compiler::hir::stmt::HirStmtKind::ReleaseOwned { .. }
        ));
    }
    assert_eq!(before_releases, direct, "one lowering, same program before ownership insertion");
    assert!(!analysis.hir.callables.is_empty());
    briev_compiler::hir::verify(&analysis.hir).expect("HIR verifies");
    assert!(analysis.hir.callables.iter().any(|c| c.name == "main"));
}
