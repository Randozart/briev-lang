// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H5 inventory port: per-collection `dyn Trait` concrete sets over HIR.
//! Mirror of `analysis::dyn_concrete_sets` (AST, authoritative until the
//! consumer flip). Reuses `DynCollectionInfo`, `DynDispatchForm`,
//! `dispatch_form_for`, `SWITCH_THRESHOLD`, and `collection_dyn_trait`.

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirProgram, HirTypeDefKind};
use crate::analysis::dyn_concrete_sets::{
    collection_dyn_trait, dispatch_form_for, DynCollectionInfo, DynDispatchForm,
};
use crate::ast::Type;
use std::collections::{BTreeSet, HashMap};

pub fn analyze(program: &HirProgram) -> HashMap<String, DynCollectionInfo> {
    let declared = top_level_declared_types(program);
    let mut out = HashMap::new();
    for g in &program.globals {
        if g.is_state_decl {
            continue;
        }
        analyze_let(
            &g.name,
            g.ty.as_ref(),
            g.expr.as_ref(),
            program,
            &declared,
            &mut out,
        );
    }
    out
}

fn analyze_let(
    name: &str,
    ty: Option<&Type>,
    expr: Option<&HirExpr>,
    program: &HirProgram,
    declared: &HashMap<String, Type>,
    out: &mut HashMap<String, DynCollectionInfo>,
) {
    let Some(ty) = ty else { return };
    let Some(trait_name) = collection_dyn_trait(ty) else {
        return;
    };
    let elems = match expr {
        Some(HirExpr { kind: HirExprKind::List(elems), .. }) => elems,
        _ => {
            out.insert(
                name.to_string(),
                DynCollectionInfo {
                    trait_name,
                    concrete_set: BTreeSet::new(),
                    dispatch_form: DynDispatchForm::Vtable,
                },
            );
            return;
        }
    };
    let mut set = BTreeSet::new();
    for el in elems {
        if let Some(concrete) = element_concrete(el, declared) {
            if conforms(program, &concrete, &trait_name) {
                set.insert(concrete);
            }
        } else {
            set.clear();
            break;
        }
    }
    out.insert(
        name.to_string(),
        DynCollectionInfo {
            trait_name,
            concrete_set: set.clone(),
            dispatch_form: dispatch_form_for(&set),
        },
    );
}

fn element_concrete(el: &HirExpr, declared: &HashMap<String, Type>) -> Option<String> {
    match &el.kind {
        HirExprKind::StructLiteral { type_name, .. } => Some(type_name.clone()),
        HirExprKind::Identifier(name) => declared.get(name).and_then(concrete_of_scalar_dyn),
        _ => None,
    }
}

/// A scalar `dyn Trait` binding erases the concrete — always open.
fn concrete_of_scalar_dyn(ty: &Type) -> Option<String> {
    match ty {
        Type::Dyn(_) => None,
        _ => None,
    }
}

fn conforms(program: &HirProgram, concrete: &str, trait_name: &str) -> bool {
    let trait_slots = collect_trait_slots(program, trait_name);
    if trait_slots.is_empty() {
        return program
            .type_defs
            .iter()
            .find(|td| td.name == concrete)
            .map_or(false, |td| td.traits.iter().any(|t| t == trait_name));
    }
    let mut provided: BTreeSet<String> = BTreeSet::new();
    for c in program.callables.iter().filter(|c| c.owner.as_deref() == Some(concrete)) {
        provided.insert(c.name.clone());
    }
    trait_slots.iter().all(|s| provided.contains(s))
}

fn collect_trait_slots(program: &HirProgram, trait_name: &str) -> Vec<String> {
    program
        .trait_functions
        .iter()
        .find(|(n, _)| n == trait_name)
        .map(|(_, slots)| slots.clone())
        .unwrap_or_default()
}

/// Declared type of every top-level `let` binding WITH an initializer —
/// mirrors the AST helper (`Statement::Let { ty: Some, expr: Some }`).
fn top_level_declared_types(program: &HirProgram) -> HashMap<String, Type> {
    let mut out = HashMap::new();
    for g in &program.globals {
        if g.is_state_decl {
            continue;
        }
        if let (Some(t), Some(_)) = (&g.ty, &g.expr) {
            out.insert(g.name.clone(), t.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_program;

    const TRAITS: &str = r#"
trait Greeter {
    defn greet(me: Self, times: Int) -> Int;
};
type Dog: Greeter { base: Int; };
type Cat: Greeter { lives: Int; };
impl Dog {
    defn greet(me: Dog, times: Int) -> Int { term me.base * 100 + times; }
};
impl Cat {
    defn greet(me: Cat, times: Int) -> Int { term me.lives * 10 + times; }
};
"#;

    fn compare(src: &str) -> HashMap<String, DynCollectionInfo> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let items = crate::parser::Parser::new(tokens, src).parse_program().unwrap();
        let ast = crate::analysis::dyn_concrete_sets::analyze(&items);
        let hir = analyze(&lower_program(&items));
        assert_eq!(hir, ast, "HIR/AST dyn concrete-set drift");
        hir
    }

    #[test]
    fn heterogeneous_two_elements_yield_switch() {
        let info = compare(&format!(
            "{TRAITS}let handlers: List<dyn Greeter> = [Dog {{ base: 7 }}, Cat {{ lives: 9 }}];"
        ));
        let h = info.get("handlers").expect("handlers analyzed");
        assert_eq!(h.trait_name, "Greeter");
        assert_eq!(h.concrete_set.len(), 2);
        assert_eq!(h.dispatch_form, DynDispatchForm::Switch);
    }

    #[test]
    fn single_inline_nonliteral_vtable_and_non_dyn_ignored() {
        let info = compare(&format!(
            "{TRAITS}let only: List<dyn Greeter> = [Dog {{ base: 1 }}];"
        ));
        assert_eq!(info["only"].dispatch_form, DynDispatchForm::Inline);
        let info = compare(&format!(
            "{TRAITS}defn make() -> List<dyn Greeter> {{ term [Dog {{ base: 1 }}]; }}\nlet handlers: List<dyn Greeter> = make();"
        ));
        assert!(info["handlers"].concrete_set.is_empty());
        assert_eq!(info["handlers"].dispatch_form, DynDispatchForm::Vtable);
        let info = compare(&format!("{TRAITS}let xs: List<Int> = [1, 2];"));
        assert!(!info.contains_key("xs"));
    }

    #[test]
    fn five_elements_yield_vtable() {
        let types = "type Bird: Greeter { wing: Int; };\nimpl Bird { defn greet(me: Bird, times: Int) -> Int { term me.wing; } };\ntype Fish: Greeter { fin: Int; };\nimpl Fish { defn greet(me: Fish, times: Int) -> Int { term me.fin; } };\ntype Frog: Greeter { hop: Int; };\nimpl Frog { defn greet(me: Frog, times: Int) -> Int { term me.hop; } };\n";
        let info = compare(&format!(
            "{TRAITS}{types}let h: List<dyn Greeter> = [Dog {{ base: 1 }}, Cat {{ lives: 2 }}, Bird {{ wing: 3 }}, Fish {{ fin: 4 }}, Frog {{ hop: 5 }}];"
        ));
        assert_eq!(info["h"].concrete_set.len(), 5);
        assert_eq!(info["h"].dispatch_form, DynDispatchForm::Vtable);
    }
}
