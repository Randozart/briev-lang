//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use super::*;

/// 2026-11-06: defaults are ordinary Self-specialized callables. Register after
/// concrete functions so lexical top-level definitions retain precedence.
/// Retire with a single shared specialization index for analysis and lowering.
pub(super) fn register_defaults(graph: &mut Graph, items: &[TopLevel]) {
    for item in items {
        match item {
            TopLevel::Trait(trait_def) => register_trait(graph, trait_def),
            TopLevel::Export(export) => register_defaults(graph, std::slice::from_ref(&export.inner)),
            TopLevel::SyncGroup { item, .. } | TopLevel::Fuzzed { item, .. } => register_defaults(graph, std::slice::from_ref(item)),
            _ => {}
        }
    }
}

fn register_trait(graph: &mut Graph, trait_def: &crate::ast::TraitDef) {
    for method in &trait_def.functions {
        if method.body.is_empty() || graph.callables.contains_key(&method.name) || graph.generics.contains_key(&method.name) { continue; }
        let mut generic = method.clone();
        generic.type_params = vec![crate::ast::top::TypeParam {
            name: "Self".into(), bound: Some(crate::ast::Type::Custom(trait_def.name.clone())),
        }];
        graph.generics.insert(method.name.clone(), generic);
    }
}

pub(super) fn substitute_output(output: &mut crate::ast::OutputType, substitution: &std::collections::HashMap<String, crate::ast::Type>) {
    use crate::ast::OutputType;
    match output {
        OutputType::Single(ty) => *ty = crate::typechecker::substitute_type(ty, substitution),
        OutputType::Union(items) | OutputType::Tuple(items) => {
            for item in items { substitute_output(item, substitution); }
        }
        OutputType::Array(inner) | OutputType::Named(_, inner) => substitute_output(inner, substitution),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(source: &str) -> crate::analysis::reclamation::ReclamationPlan {
        let tokens = crate::lexer::tokenize(source).unwrap();
        let items = crate::parser::Parser::new(tokens, source).parse_program().unwrap();
        crate::analysis::reclamation::analyze(&items, &Default::default(), None)
    }

    #[test]
    fn defaults_follow_concrete_effects_through_chains_and_free_calls() {
        let result = plan(r#"
            trait Access {
                defn read(value: Self) -> Int;
                defn identity(value: Self) -> Self { term value; };
                defn twice(value: Self) -> Int { term read(value) + read(value); };
            };
            obj Local {
                ptr: Ptr;
                defn read(value: Local) -> Int { term Load#(value.ptr,8); };
            };
            obj Captured {
                ptr: Ptr;
                defn read(value: Captured) -> Int { unknown(value.ptr); term 0; };
            };
            defn closed() -> Int {
                let value: Local = Local { ptr: Malloc#(8) };
                term value.identity().twice();
            };
            defn free_call() -> Int {
                let value: Local = Local { ptr: Malloc#(8) };
                term twice(value);
            };
            defn escaping() -> Int {
                let value: Captured = Captured { ptr: Malloc#(8) };
                term value.identity().twice();
            };
        "#);
        assert!(result.regions.contains("closed"), "{result:?}");
        assert!(result.regions.contains("free_call"), "{result:?}");
        assert!(result.barriers.contains("escaping"), "{result:?}");
        assert!(result.known.contains("twice$Local"), "{result:?}");
    }

    #[test]
    fn concrete_and_top_level_shadowing_keep_their_actual_effects() {
        let result = plan(r#"
            trait Access { defn consume(value: Self) -> Int { term 0; }; };
            obj Captured {
                ptr: Ptr;
                defn consume(value: Captured) -> Int { unknown(value.ptr); term 0; };
            };
            defn consume(value: Captured) -> Int { term 0; };
            defn member() -> Int {
                let value: Captured = Captured { ptr: Malloc#(8) };
                term value.consume();
            };
            defn free_call() -> Int {
                let value: Captured = Captured { ptr: Malloc#(8) };
                term consume(value);
            };
        "#);
        assert!(result.barriers.contains("member"), "{result:?}");
        assert!(result.regions.contains("free_call"), "{result:?}");
    }
}
