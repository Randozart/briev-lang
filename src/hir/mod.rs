// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Typed HIR — H0a callable/export skeleton.
//!
//! See `docs/plans/2026-11-02-typed-hir.md`. The parser AST remains the
//! syntax tree; HIR becomes the semantic source in later phases. This first
//! slice assigns stable IDs, normalizes export wrappers, and verifies the
//! callable surface. No consumer uses it yet.

pub mod expr;
pub mod ids;
pub mod lower;
pub mod pattern;
pub mod program;
pub mod stats;
pub mod stmt;
pub mod verify;

pub use lower::lower_program;
pub use program::{HirCallable, HirCallableKind, HirProgram};
pub use stats::{unsupported_counts, UnsupportedCounts};
pub use verify::verify;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<crate::ast::TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    #[test]
    fn exports_are_normalized_into_callable_metadata() {
        let program = lower_program(&parse(
            r#"
export defn greet(name: String) -> String { term name; };
txn tick(x: Int) -> Int { term x + 1; };
"#,
        ));
        verify(&program).expect("valid HIR");
        assert_eq!(program.callables.len(), 2);
        assert_eq!(program.callables[0].name, "greet");
        assert!(program.callables[0].exported);
        assert_eq!(program.callables[0].export_symbol.as_deref(), Some("greet"));
        assert_eq!(program.callables[1].kind, HirCallableKind::Transaction);
        assert!(!program.callables[1].exported);
    }

    #[test]
    fn ids_and_lowering_are_deterministic() {
        let items = parse("defn f(x: Int) -> Int { term x; };\n");
        let a = lower_program(&items);
        let b = lower_program(&items);
        assert_eq!(a, b);
        assert_eq!(a.callables[0].id.0, 0);
        assert_eq!(a.callables[0].body.statements.len(), 1);
    }

    #[test]
    fn bodies_lower_to_typed_statement_nodes() {
        let program = lower_program(&parse(
            "defn f(x: Int) -> Int { let y: Int = x + 1; term y * 2; };
",
        ));
        verify(&program).expect("valid HIR");
        let body = &program.callables[0].body.statements;
        assert_eq!(body.len(), 2);
        assert!(matches!(body[0].kind, super::stmt::HirStmtKind::Let { .. }));
        assert!(matches!(body[1].kind, super::stmt::HirStmtKind::Term(Some(_))));
    }

    #[test]
    fn foreach_and_match_patterns_lower() {
        let program = lower_program(&parse(
            "defn f(xs: List<Int>) -> Int { let s: Int = 0; foreach x in xs { s = s + x; }; term match s { 0 => 1, n => n }; };
",
        ));
        verify(&program).expect("valid HIR");
        let body = &program.callables[0].body.statements;
        assert!(matches!(body[1].kind, super::stmt::HirStmtKind::Foreach { .. }));
        assert!(matches!(body[2].kind, super::stmt::HirStmtKind::Term(Some(_))));
    }

    #[test]
    fn nested_ids_are_verified_and_unsupported_is_counted() {
        let program = lower_program(&parse(
            "defn f(x: Int) -> Int { let y: Int = x.call(); term y + 1; };\n",
        ));
        verify(&program).expect("valid HIR");
        let counts = unsupported_counts(&program);
        assert_eq!(counts.expressions, 0, "method call is now normalized");
        assert_eq!(counts.statements, 0);
    }

    #[test]
    fn object_members_are_indexed_as_callables() {
        let program = lower_program(&parse(
            "obj Box { defn get(self) -> Int { term 1; }; };\n",
        ));
        verify(&program).expect("valid HIR");
        assert_eq!(program.callables.len(), 1);
        assert_eq!(program.callables[0].kind, HirCallableKind::MemberDefn);
    }
}
