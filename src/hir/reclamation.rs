// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! R (2026-11-11): reclamation analysis over typed HIR — first slice.
//!
//! The full port translates the AST `Graph`/solver in
//! `analysis::reclamation` behind a parity gate. This module starts with the
//! closed-initializer predicate the entry-region decision reads; AST remains
//! authoritative until the port is complete.

use super::program::HirProgram;

/// True when every top-level initializer is a closed (literal/absent) value:
/// no state declaration and no non-literal top-level `let` initializer.
///
/// AST parity (`analysis::reclamation::walk::closed_initializers`): StateDecl
/// is closed=false; a top-level `let` is closed only for Int/Float/Bool/Char
/// literal or absent initializers; all other item kinds are closed=true.
pub fn closed_initializers(program: &HirProgram) -> bool {
    program.globals.iter().all(|global| {
        if global.is_state_decl {
            return false;
        }
        match &global.expr {
            None => true,
            Some(expr) => matches!(
                expr.kind,
                super::expr::HirExprKind::Literal(
                    super::expr::HirLiteral::Int(_)
                        | super::expr::HirLiteral::Float(_)
                        | super::expr::HirLiteral::Bool(_)
                        | super::expr::HirLiteral::Char(_)
                )
            ),
        }
    })
}

/// AST parity: a member uses an implicit receiver when its body mentions
/// `self` or any declaring-type slot name.
fn uses_implicit_receiver(callable: &super::program::HirCallable, field_names: &[String]) -> bool {
    let mut implicit = false;
    crate::hir::expr_walk::walk_stmts(&callable.body.statements, &mut |e| {
        if let super::expr::HirExprKind::Identifier(name) = &e.kind {
            implicit |= name == "self" || field_names.iter().any(|f| f == name);
        }
    });
    implicit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hir(src: &str) -> HirProgram {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        crate::hir::lower_program(&parser.parse_program().expect("parse"))
    }

    fn ast(src: &str) -> bool {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        crate::analysis::reclamation::closed_initializers_for_test(&items)
    }

    fn parity(src: &str) {
        assert_eq!(closed_initializers(&hir(src)), ast(src), "drift for:\n{src}");
    }

    #[test]
    fn literal_globals_are_closed() {
        parity("let a: Int = 1;\nlet b: Bool = false;\nlet c: Float = 1.5;\nlet d: Char = 'x';\n");
    }

    #[test]
    fn state_and_non_literal_globals_are_open() {
        parity("state let n: Int = 0;\n");
        parity("let xs: List<Int> = [1, 2];\n");
        parity("let n: Int = compute();\ndefn compute() -> Int { term 1; };\n");
    }

    fn facts_hir(src: &str) -> RegistrationFacts {
        registration_facts(&hir(src))
    }
    fn facts_ast(src: &str) -> RegistrationFacts {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        crate::analysis::reclamation::registration_facts_for_test(&items)
    }
    fn parity_facts(src: &str) {
        assert_eq!(facts_hir(src), facts_ast(src), "registration drift:\n{src}");
    }

    #[test]
    fn registration_matches_ast_for_callables() {
        parity_facts(
            "defn plain(a: Int) -> Int { term a; };\n\
             export defn exported(a: Int) -> Int { term a; };\n\
             defn generic<T>(x: T) -> T { term x; };\n\
             node reactive [true][true] { term; };\n\
             async txn waiting [true][true] { term; };",
        );
    }

    #[test]
    fn registration_matches_ast_for_enum_constructors() {
        parity_facts("enum Mode { Active(Int), Idle };");
    }

    #[test]
    fn registration_matches_ast_for_methods_and_user_operators() {
        parity_facts(
            "defn add_impl(a: Int, b: Int) -> Int { term a + b; };\n\
             obj Box { value: Int; defn get(b: Box) -> Int { term b.value; };\n\
                        op Add: add_impl(#Lh, #Rh); };",
        );
    }

    #[test]
    fn registration_matches_ast_for_globals() {
        parity_facts(
            "let a: Int = 1;\nstate let n: Int = 0;\nconst C: Int = 3;\n\
             defn f(a: Int) -> Int { term a; };",
        );
    }

    #[test]
    fn no_globals_is_closed() {
        parity("defn f() -> Int { term 1; };\n");
    }
}

/// R2 (2026-11-11): structural registration facts the reclamation walker needs
/// before constraints can be built. Fields not yet ported (constructors,
/// default-encoding targets, user operators, method alias map) are left out
/// until their AST sources have HIR equivalents.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistrationFacts {
    pub bodies: std::collections::BTreeMap<String, (Vec<(String, crate::ast::Type)>, usize)>,
    pub generics: std::collections::BTreeSet<String>,
    pub return_types: std::collections::BTreeMap<String, crate::ast::Type>,
    pub entry_nodes: std::collections::BTreeSet<String>,
    pub unsupported: std::collections::BTreeSet<String>,
    /// Enum variant names (constructor keys).
    pub constructors: std::collections::BTreeSet<String>,
    /// Types with a non-intrinsic op binding (user operator barrier).
    pub user_operators: std::collections::BTreeSet<String>,
    /// (owner, member) → synthetic `Owner::member` callable when the receiver
    /// is explicit; `None` shadows transactions and implicit-receiver methods.
    pub methods: std::collections::BTreeMap<(String, String), Option<String>>,
    pub globals: std::collections::BTreeSet<String>,
    pub global_types: std::collections::BTreeMap<String, crate::ast::Type>,
}

/// Mirror of `analysis::reclamation::walk::register` for the structural fields
/// above (AST parity is asserted in tests).
pub fn registration_facts(program: &HirProgram) -> RegistrationFacts {
    use super::program::HirCallableKind;
    let mut facts = RegistrationFacts {
        bodies: Default::default(),
        generics: Default::default(),
        return_types: Default::default(),
        entry_nodes: Default::default(),
        unsupported: Default::default(),
        constructors: Default::default(),
        user_operators: Default::default(),
        methods: Default::default(),
        globals: Default::default(),
        global_types: Default::default(),
    };
    for global in &program.globals {
        facts.globals.insert(global.name.clone());
        if let Some(ty) = &global.ty {
            facts.global_types.insert(global.name.clone(), ty.clone());
        }
    }
    for (name, ty) in &program.const_types {
        facts.globals.insert(name.clone());
        facts.global_types.insert(name.clone(), ty.clone());
    }
    for def in &program.type_defs {
        if let super::program::HirTypeDefKind::Enum { variants } = &def.kind {
            for variant in variants {
                let name = match variant {
                    super::program::HirEnumVariant::Unit(n)
                    | super::program::HirEnumVariant::Tuple(n, _)
                    | super::program::HirEnumVariant::Struct(n, _) => n,
                };
                facts.constructors.insert(name.clone());
            }
        }
    }
    for def in &program.type_defs {
        if def.has_user_operators {
            facts.user_operators.insert(def.name.clone());
        }
    }
    // Explicit-receiver methods register as `Owner::method` synthetic
    // callables; implicit-receiver members only shadow the key (AST parity).
    for def in &program.type_defs {
        if !matches!(
            def.kind,
            super::program::HirTypeDefKind::Object { .. }
                | super::program::HirTypeDefKind::Struct { .. }
        ) {
            continue;
        }
        let field_names: Vec<String> = match &def.kind {
            super::program::HirTypeDefKind::Object { fields }
            | super::program::HirTypeDefKind::Struct { fields } => {
                fields.iter().map(|f| f.name.clone()).collect()
            }
            _ => Vec::new(),
        };
        for callable in program.callables.iter().filter(|c| {
            c.owner.as_deref() == Some(def.name.as_str())
                && !c.is_impl_fn
                && !c.trait_default
                && matches!(
                    c.kind,
                    super::program::HirCallableKind::MemberDefn
                        | super::program::HirCallableKind::MemberTransaction
                )
        }) {
            let key = (def.name.clone(), callable.name.clone());
            facts.methods.insert(key.clone(), None);
            if callable.kind != super::program::HirCallableKind::MemberDefn {
                continue;
            }
            if !def.type_params.is_empty() || !callable.type_params.is_empty() {
                continue;
            }
            if uses_implicit_receiver(callable, &field_names) {
                continue;
            }
            let Some(first) = callable.params.first() else { continue };
            if first.ty.universe_key() != Some(def.name.as_str()) {
                continue;
            }
            let synthetic = format!("{}::{}", def.name, callable.name);
            facts.methods.insert(key, Some(synthetic.clone()));
            let params = callable
                .params
                .iter()
                .map(|p| (p.name.clone(), p.ty.clone()))
                .collect();
            facts
                .bodies
                .insert(synthetic.clone(), (params, callable.body.statements.len()));
            if let Some(ty) = &callable.result {
                facts.return_types.insert(synthetic, ty.clone());
            }
        }
    }
    for callable in &program.callables {
        if callable.owner.is_some() || callable.is_isr || callable.trait_default
            || callable.is_impl_fn
        {
            continue;
        }
        let is_defn = callable.kind == HirCallableKind::Defn;
        if is_defn && !callable.type_params.is_empty() {
            facts.generics.insert(callable.name.clone());
            continue;
        }
        if !matches!(
            callable.kind,
            HirCallableKind::Defn | HirCallableKind::Transaction
        ) {
            continue;
        }
        let params: Vec<(String, crate::ast::Type)> = callable
            .params
            .iter()
            .map(|p| (p.name.clone(), p.ty.clone()))
            .collect();
        let mut len = callable.body.statements.len();
        if callable.kind == HirCallableKind::Transaction && callable.contract.is_some() {
            // AST register_transaction appends pre/post condition expressions.
            len += 2;
        }
        facts.bodies.insert(callable.name.clone(), (params, len));
        if let Some(ty) = &callable.result {
            facts.return_types.insert(callable.name.clone(), ty.clone());
        }
        if callable.is_reactive {
            facts.entry_nodes.insert(callable.name.clone());
        }
        if callable.is_async {
            facts.unsupported.insert(callable.name.clone());
        }
    }
    facts
}
