// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Typed HIR — H0a callable/export skeleton.
//!
//! See `docs/plans/2026-11-02-typed-hir.md`. The parser AST remains the
//! syntax tree; HIR becomes the semantic source in later phases. This first
//! slice assigns stable IDs, normalizes export wrappers, and verifies the
//! callable surface. No consumer uses it yet.

pub mod cast_paths;
pub mod expr;
pub mod frontend;
pub mod ids;
pub mod lower;
pub mod pattern;
pub mod program;
pub mod stats;
pub mod stmt;
pub mod verify;
pub mod walk;

pub use cast_paths::resolve_cast_paths;
pub use frontend::lower_program_after_frontend;
pub use lower::{lower_program, lower_program_checked, HirLowerIssue};
pub use program::{HirCallable, HirCallableKind, HirProgram};
pub use stats::{unsupported_counts, UnsupportedCounts};
pub use verify::verify;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, StageKind, TopLevel};
    use crate::plugin::{Plugin, PluginManager};
    use crate::type_universe::TypeUniverse;

    #[derive(Debug)]
    struct DemoPlugin;

    impl Plugin for DemoPlugin {
        fn name(&self) -> &str { "hir-demo" }
        fn stages(&self) -> Vec<StageKind> { vec![StageKind::Parsed] }
        fn on_ast(
            &self,
            program: &mut Vec<TopLevel>,
            _universe: &mut TypeUniverse,
        ) -> Result<(), String> {
            let mut rewrite = |expr: &mut Expr| {
                if matches!(expr, Expr::PluginIntercept { name, .. } if name == "demo") {
                    *expr = Expr::Decimal(7);
                }
            };
            for item in program {
                crate::ast::visit_mut::walk_toplevel(item, &mut rewrite);
            }
            Ok(())
        }
    }

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
    fn cast_paths_are_explicit_when_endpoints_are_typed() {
        let mut program = lower_program(&parse(
            "defn f(x: Int) -> Float { term 1 as Float; };\n",
        ));
        let graph = crate::casting::graph::CastingGraph::new();
        let universe = TypeUniverse::new();
        let resolved = resolve_cast_paths(&mut program, &graph, &universe);
        assert_eq!(resolved, 1);
        let body = &program.callables[0].body.statements;
        let super::stmt::HirStmtKind::Term(Some(expr)) = &body[0].kind else {
            panic!("term expression expected");
        };
        let super::expr::HirExprKind::Cast { via, .. } = &expr.kind else {
            panic!("cast expected");
        };
        assert!(via.as_ref().is_some_and(|path| !path.is_empty()));
    }

    #[test]
    fn frontend_plugins_run_before_checked_hir() {
        let src = "defn f(x: Int) -> Int { term demo!(x); };\n";
        let mut items = parse(src);
        let mut pm = PluginManager::new();
        pm.register(Box::new(DemoPlugin));
        let mut universe = TypeUniverse::new();
        let program = lower_program_after_frontend(&mut items, &pm, &mut universe)
            .expect("plugin resolves before HIR");
        verify(&program).expect("valid HIR");
        assert_eq!(unsupported_counts(&program).total(), 0);
    }

    #[test]
    fn checked_lowering_rejects_frontend_only_syntax() {
        let plain = parse("defn f(x: Int) -> Int { term x + 1; };\n");
        assert!(lower_program_checked(&plain).is_ok());

        let plugin = parse("defn f(x: Int) -> Int { term demo!(x); };\n");
        let issues = lower_program_checked(&plugin).expect_err("plugin intercept must block HIR");
        assert!(issues.iter().any(|issue| issue.tag == "plugin-intercept"));
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
