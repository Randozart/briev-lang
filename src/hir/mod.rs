// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Typed HIR — H0a callable/export skeleton.
//!
//! See `docs/plans/2026-11-02-typed-hir.md`. The parser AST remains the
//! syntax tree; HIR becomes the semantic source in later phases. This first
//! slice assigns stable IDs, normalizes export wrappers, and verifies the
//! callable surface. No consumer uses it yet.

pub mod boundary_ownership;
pub mod cast_paths;
pub mod constructors;
pub mod defn_liveness;
pub mod expr;
pub mod facts;
pub mod frontend;
pub mod ids;
pub mod lower;
pub mod needs_state;
pub mod ownership;
pub mod pattern;
pub mod program;
pub mod sinks;
pub mod storage_class;
pub mod stats;
pub mod tail_calls;
pub mod type_facts;
pub mod stmt;
pub mod tail_recursion;
pub mod verify;
pub mod walk;

pub use cast_paths::resolve_cast_paths;
pub use constructors::{build_constructor_facts, HirConstructorFacts, HirVariantFact, HirVariantRef};
pub use facts::{build_facts, HirFacts};
pub use frontend::lower_program_after_frontend;
pub use lower::{lower_program, lower_program_checked, HirLowerIssue};
pub use needs_state::{compute_defn_needs_plain, compute_defn_needs_state, compute_export_needs_state};
pub use ownership::{build_value_facts, HirDestinationClass, HirOwnershipClass, HirValueFacts, HirValueKey};
pub use sinks::{build_sink_facts, HirAggregateKind, HirSinkFacts, HirSinkKey};
pub use program::{HirCallable, HirCallableKind, HirOutput, HirProgram};
pub use stats::{unsupported_counts, UnsupportedCounts};
pub use tail_calls::{build_tail_call_facts, HirCallableTailCalls, HirTailCall, HirTailCallFacts};
pub use type_facts::{build_type_facts, HirEnumFacts, HirObjectFacts, HirTypeFacts};
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
    fn members_and_trait_defaults_carry_owner() {
        let program = lower_program(&parse(
            r#"
obj Box { side: Int;
    defn area(this: Box) -> Int { term 2; };
};
trait Shape {
    defn area(this: Self) -> Int;
    defn twice(this: Self) -> Int { term 4; };
};
impl Box {
    defn scale(this: Box, k: Int) -> Int { term k * 2; };
};
"#,
        ));
        verify(&program).expect("valid HIR");
        let box_area = program
            .callables
            .iter()
            .find(|c| c.name == "area" && c.owner.as_deref() == Some("Box"))
            .expect("Box::area");
        assert!(!box_area.trait_default);
        let twice = program
            .callables
            .iter()
            .find(|c| c.name == "twice")
            .expect("trait default twice");
        assert!(twice.trait_default);
        assert_eq!(twice.owner.as_deref(), Some("Shape"));
        let scale = program
            .callables
            .iter()
            .find(|c| c.name == "scale" && c.owner.as_deref() == Some("Box"))
            .expect("impl Box::scale");
        assert!(!scale.trait_default);
    }

    #[test]
    fn top_level_globals_are_lowered() {
        let program = lower_program(&parse(
            r#"
let counter: Int = 0;
let label: String = "x";
"#,
        ));
        assert_eq!(program.globals.len(), 2);
        assert_eq!(program.globals[0].name, "counter");
        assert_eq!(
            program.globals[0].ty,
            Some(crate::ast::Type::Custom("Int".to_string()))
        );
        assert!(!program.globals[0].is_state_decl);
        assert_eq!(program.globals[1].name, "label");
    }

    #[test]
    fn foreign_bindings_and_declared_protocols_are_lowered() {
        let program = lower_program(&parse(
            r#"
frgn HostAdd(a: Int, b: Int) -> Int : host_add from "host";
type CStr: String<C_String> { };
"#,
        ));
        assert_eq!(program.foreign.len(), 1);
        assert_eq!(program.foreign[0].name, "HostAdd");
        assert_eq!(program.foreign[0].params.len(), 2);
        assert_eq!(
            program.foreign[0].ret,
            Some(crate::ast::Type::Custom("Int".to_string()))
        );
        let cstr = program
            .type_defs
            .iter()
            .find(|t| t.name == "CStr")
            .expect("CStr alias");
        assert_eq!(cstr.protocol.as_deref(), Some("String<C_String>"));
    }

    #[test]
    fn outputs_and_constants_are_lowered() {
        let program = lower_program(&parse(
            r#"
const K: Int = 1;
defn f() -> Int { term 1; };
"#,
        ));
        assert!(program.constants.contains(&"K".to_string()));
        assert!(matches!(
            program.callables[0].outputs,
            Some(HirOutput::Single(_))
        ));
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
    fn unified_facts_bundle_is_consistent() {
        let program = lower_program(&parse(
            "enum Maybe { None, Some(Int) }; struct Point { x: Int; y: Int; }; defn f(n: Int) -> Int { term f(n); };\n",
        ));
        let facts = build_facts(&program);
        assert_eq!(facts.types.enums.len(), 1);
        assert_eq!(facts.types.structs.len(), 1);
        assert_eq!(facts.tail_calls.callables.len(), 1);
        assert!(!facts.values.values.is_empty());
    }

    #[test]
    fn tail_call_facts_only_count_return_position() {
        let program = lower_program(&parse(
            "defn f(n: Int) -> Int { let x: Int = f(n); term f(n); };\n",
        ));
        let facts = build_tail_call_facts(&program);
        let callable = &facts.callables[&program.callables[0].id];
        assert_eq!(callable.calls.len(), 1);
        assert_eq!(callable.calls[0].args.len(), 1);
    }

    #[test]
    fn enum_constructors_and_patterns_resolve_to_candidates() {
        let program = lower_program(&parse(
            "enum Maybe { None, Some(Int) }; defn f(x: Int) -> Int { term match Some(x) { Some(n) => n, None => 0 }; };\n",
        ));
        let facts = build_constructor_facts(&program);
        assert!(facts.calls.values().any(|fact| fact.name == "Some"));
        assert!(facts.patterns.values().any(|fact| fact.name == "Some"));
        assert!(facts.patterns.values().any(|fact| fact.name == "None"));
    }

    #[test]
    fn type_declarations_are_captured_by_hir() {
        let program = lower_program(&parse(
            "enum Maybe { None, Some(Int) }; struct Point { x: Int; y: Int; }; defn f(x: Int) -> Int { term x; };\n",
        ));
        let facts = build_type_facts(&program);
        assert!(facts.enums.contains_key("Maybe"));
        assert_eq!(facts.structs["Point"].fields.len(), 2);
        assert_eq!(program.type_defs.len(), 2);
        assert_eq!(program.callables[0].name, "f");
    }

    #[test]
    fn sink_facts_cover_returns_fields_collections_and_calls() {
        let program = lower_program(&parse(
            "defn f(x: Int) -> Int { let p: Point = Point { a: x; b: 2 }; let l: List<Int> = [x, 2]; term p.a; };\n",
        ));
        let facts = build_sink_facts(&program);
        let fields = facts
            .sinks
            .keys()
            .filter(|key| matches!(key, HirSinkKey::ObjectField { .. }))
            .count();
        assert_eq!(fields, 2);
        assert!(facts.sinks.keys().any(|key| matches!(
            key,
            HirSinkKey::Aggregate { kind: HirAggregateKind::Collection, .. }
        )));
        assert!(facts.sinks.keys().any(|key| matches!(key, HirSinkKey::Return { .. })));
    }

    #[test]
    fn ownership_facts_use_stable_binding_ids() {
        let program = lower_program(&parse(
            "defn f(x: Int) -> Int { let y: Int = 1; foreach z in [1,2] { y = y + z; }; term match y { n => n }; };\n",
        ));
        let facts = build_value_facts(&program);
        assert_eq!(facts.values.len(), 4);
        assert!(facts.values.contains_key(&HirValueKey::Param {
            callable: program.callables[0].id,
            index: 0,
        }));
        assert_eq!(
            facts
                .values
                .values()
                .filter(|fact| fact.destination == HirDestinationClass::Boxed)
                .count(),
            4
        );
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
