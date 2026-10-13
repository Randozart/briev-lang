// ── Per-Collection dyn Concrete-Set Analysis ────────────────────────────
//
// 2026-10-12 (SPEC §8.6.1 Option B, plan 2026-10-11-coll-element-size.md):
// a `Collection<dyn Trait>` binding holds elements whose concrete type is
// decided per element at construction. The dispatch form at a use-site
// (Inline → Switch → Vtable → Omit, per §8.6.1) is a FRONTEND decision:
// the reachable concrete set of the collection the value came from, computed
// once here and read by the backend — the same frontend-driven-dispatch
// pillar as loop shapes and GPU tiles.
//
// This pass mirrors the interpreter's reference behavior (Rule 5):
// `src/interpreter/eval.rs` `eval_let_binding` resolves each list-literal
// element's concrete from its SYNTAX (a constructor names itself; a prior
// `dyn` binding carries its provenance). The same resolution happens here,
// at compile time, for every `Collection<dyn Trait>` let whose RHS is a list
// literal. Non-literal initializers (a function returning a collection, an
// alias to another binding) yield an OPEN set — the dispatch form falls to
// Vtable at every use-site, which is the sound default (§8.6.1 form 3).
//
// The concrete set is matched STRUCTURALLY by the annotation's type argument
// (the `Dyn(T)` parameter of an `Applied`/`Generic` collection), never by a
// collection type name (Rule 19). The trait's conformance set is derived
// from the AST exactly as the backend's `register_dyn_thunks` does: a
// `TypeDef`'s declared traits (`td.traits`) or an `impl` whose provided
// methods cover the trait's slots.

use std::collections::{BTreeSet, HashMap};

/// The concrete set a `dyn Trait` collection carries, and the dispatch form
/// the frontend licenses from it (§8.6.1, in ascending cost order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynDispatchForm {
    /// The concrete is provable at every element — inline the impl, no tag.
    Inline,
    /// A small closed set (2..=4 concretes) — a tag switch over the set.
    Switch,
    /// Open set, or a boundary that demands a native vtable — per-trait table.
    Vtable,
}

/// One `Collection<dyn Trait>` binding: the trait it carries, the closed
/// concrete set its construction provably holds (empty = open), and the
/// dispatch form the use-sites of this binding are licensed to use.
#[derive(Debug, Clone)]
pub struct DynCollectionInfo {
    pub trait_name: String,
    /// Closed set of concrete type names provably in the collection at
    /// construction. Empty when the initializer is not a list literal (the
    /// set is open — any conforming concrete may arrive).
    pub concrete_set: BTreeSet<String>,
    pub dispatch_form: DynDispatchForm,
}

/// The Switch-form threshold: a set of at most this many concretes dispatches
/// as a tag switch (the backend's `emit_dyn_switch` cost model uses the same
/// bound); larger sets fall to the vtable.
pub const SWITCH_THRESHOLD: usize = 4;

/// Derive a dispatch form from the reachable concrete set: 0 (open) → Vtable,
/// 1 → Inline, 2..=SWITCH_THRESHOLD → Switch, else Vtable.
pub fn dispatch_form_for(set: &BTreeSet<String>) -> DynDispatchForm {
    match set.len() {
        0 => DynDispatchForm::Vtable,
        1 => DynDispatchForm::Inline,
        n if n <= SWITCH_THRESHOLD => DynDispatchForm::Switch,
        _ => DynDispatchForm::Vtable,
    }
}

/// The `dyn Trait` parameter of a collection annotation, if any —
/// `Collection<dyn Trait>` → `Some("Trait")`. Matched structurally by the
/// type argument, never by collection name. (pub(crate): the backend's local
/// collection construction consumes the same annotation rule — one
/// definition, two consumers.)
pub(crate) fn collection_dyn_trait(ty: &crate::ast::Type) -> Option<String> {
    let args = match ty {
        crate::ast::Type::Applied(_, args) | crate::ast::Type::Generic(_, args) => args,
        _ => return None,
    };
    let arg = args.iter().find(|a| matches!(a, crate::ast::Type::Dyn(_)))?;
    match arg {
        crate::ast::Type::Dyn(inner) => Some(match inner.as_ref() {
            crate::ast::Type::Custom(n) => n.clone(),
            other => format!("{}", other),
        }),
        _ => None,
    }
}

/// Resolve one list-literal element's concrete type name from its syntax —
/// the same provenance rule as the interpreter's reference (eval.rs
/// `dyn_concrete_name`): a constructor names itself; an identifier must be a
/// prior `dyn`-annotated binding whose declared type names the concrete.
/// Anything else (a plain binding, a call, a field) leaves the set open.
fn element_concrete(
    el: &crate::ast::Expr,
    declared_types: &HashMap<String, crate::ast::Type>,
) -> Option<String> {
    match el {
        crate::ast::Expr::StructLiteral { type_name, .. } => Some(type_name.clone()),
        crate::ast::Expr::Identifier(name) => match declared_types.get(name) {
            // A prior `let x: dyn Trait = Concrete{..}` declares its concrete
            // syntactically too — the annotation's dyn param is the TRAIT, so
            // read the concrete from the initializer's constructor is not
            // available here (binding only carries the type); the interpreter
            // keeps the concrete in the value. At compile time the declared
            // type of a scalar dyn binding is `dyn Trait` (concrete erased),
            // so a dyn-bound element leaves the set open — the vtable fallback
            // is the sound choice.
            Some(t) => concrete_of_scalar_dyn(t),
            None => None,
        },
        _ => None,
    }
}

/// A scalar `dyn Trait` binding's declared type erases the concrete (the
/// annotation is the trait, not the concrete), so a dyn-bound element cannot
/// contribute a known concrete to the set. Returns None (set stays open).
fn concrete_of_scalar_dyn(ty: &crate::ast::Type) -> Option<String> {
    match ty {
        crate::ast::Type::Dyn(_) => None,
        _ => None,
    }
}

/// Is `concrete` known to conform to `trait_name` in this program? Mirrors
/// the backend's `register_dyn_conformance` (register_dyn_thunks,
/// llvm/mod.rs:6282): conformance is derived from the impl block — a
/// concrete's provided methods (its own defns + its `impl` blocks) covering
/// the trait's requirement slots. The `TypeDef.traits` declaration is a
/// secondary signal only: the parser places a lone `type X: T` in the PARENT
/// slot (the single non-fundamental parent), so `td.traits` is often empty
/// (llvm/mod.rs:6277) — the impl path is the source of truth.
fn conforms(
    items: &[crate::ast::TopLevel],
    concrete: &str,
    trait_name: &str,
) -> bool {
    let trait_slots = collect_trait_slots(items, trait_name);
    if trait_slots.is_empty() {
        // No declared requirement slots to check — fall back to the
        // `TypeDef.traits` declaration (a `type X: T` parent link).
        for item in items {
            if let crate::ast::TopLevel::TypeDef(td) = item {
                if td.name == concrete && td.traits.iter().any(|t| t == trait_name) {
                    return true;
                }
            }
        }
        return false;
    }
    // Provided methods = the concrete's own defns + its impl blocks.
    let mut provided = BTreeSet::new();
    for item in items {
        match item {
            crate::ast::TopLevel::TypeDef(td) if td.name == concrete => {
                for m in &td.body.members {
                    if let crate::ast::TopLevel::Definition(d) = m {
                        provided.insert(d.name.clone());
                    }
                }
            }
            crate::ast::TopLevel::Impl(i) if i.target == concrete => {
                for d in &i.functions {
                    provided.insert(d.name.clone());
                }
            }
            _ => {}
        }
    }
    trait_slots.iter().all(|s| provided.contains(s))
}

/// The declared method order of a trait (its requirement slots).
fn collect_trait_slots(items: &[crate::ast::TopLevel], trait_name: &str) -> Vec<String> {
    for item in items {
        if let crate::ast::TopLevel::Trait(t) = item {
            if t.name == trait_name {
                return t.functions.iter().map(|d| d.name.clone()).collect();
            }
        }
    }
    Vec::new()
}

/// The declared type of every top-level `let` binding — used to resolve
/// prior `dyn` bindings referenced by list-literal elements.
fn top_level_declared_types(items: &[crate::ast::TopLevel]) -> HashMap<String, crate::ast::Type> {
    let mut out = HashMap::new();
    for item in items {
        if let crate::ast::TopLevel::Statement(s) = item {
            if let crate::ast::Statement::Let { name, ty, expr: Some(_), .. } = s.as_ref() {
                if let Some(t) = ty {
                    out.insert(name.clone(), t.clone());
                }
            }
        }
    }
    out
}

/// Compute the per-collection `dyn Trait` concrete sets for every
/// `Collection<dyn Trait>` let in the program. Keyed by the binding name;
/// the backend reads `analysis.dyn_collection_info.get(binding_name)` at a
/// use-site to choose the dispatch form (§8.6.1) and to derive the
/// per-collection tag table (never a global per-trait table).
///
/// 2026-10-12: a list-literal RHS yields the closed set of its elements'
/// concretes (filtered to those that conform to the trait — a non-conforming
/// element is a typechecker error, so the filter is a safety net, not a
/// semantic decision). A non-literal RHS yields the open set (Vtable).
pub fn analyze(items: &[crate::ast::TopLevel]) -> HashMap<String, DynCollectionInfo> {
    let declared = top_level_declared_types(items);
    let mut out = HashMap::new();
    for item in items {
        if let crate::ast::TopLevel::Statement(s) = item {
            if let crate::ast::Statement::Let { name, ty, expr, .. } = s.as_ref() {
                analyze_let(name, ty.as_ref(), expr.as_ref(), items, &declared, &mut out);
            }
        }
    }
    out
}

fn analyze_let(
    name: &str,
    ty: Option<&crate::ast::Type>,
    expr: Option<&crate::ast::Expr>,
    items: &[crate::ast::TopLevel],
    declared: &HashMap<String, crate::ast::Type>,
    out: &mut HashMap<String, DynCollectionInfo>,
) {
    let Some(ty) = ty else { return };
    let Some(trait_name) = collection_dyn_trait(ty) else { return };
    // Non-literal initializer: the concrete set is open — any conforming
    // concrete may arrive, so every use-site dispatches via the vtable.
    let elems = match expr {
        Some(crate::ast::Expr::List(elems)) => elems,
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
            if conforms(items, &concrete, &trait_name) {
                set.insert(concrete);
            }
        } else {
            // An element whose concrete is unknowable (plain binding, call,
            // …) leaves the set open.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    fn prog(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        crate::parser::Parser::new(tokens, src).parse_program().unwrap()
    }

    fn trait_and_types() -> &'static str {
        r#"
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
"#
    }

    #[test]
    fn heterogeneous_two_elements_yield_switch() {
        let src = format!("{}let handlers: List<dyn Greeter> = [Dog {{ base: 7 }}, Cat {{ lives: 9 }}];", trait_and_types());
        let items = prog(&src);
        let info = analyze(&items);
        let h = info.get("handlers").expect("handlers analyzed");
        assert_eq!(h.trait_name, "Greeter");
        assert_eq!(h.concrete_set.len(), 2);
        assert!(h.concrete_set.contains("Dog"));
        assert!(h.concrete_set.contains("Cat"));
        assert_eq!(h.dispatch_form, DynDispatchForm::Switch);
    }

    #[test]
    fn single_element_yields_inline() {
        let src = format!("{}let only: List<dyn Greeter> = [Dog {{ base: 1 }}];", trait_and_types());
        let items = prog(&src);
        let info = analyze(&items);
        let h = info.get("only").expect("only analyzed");
        assert_eq!(h.concrete_set.len(), 1);
        assert_eq!(h.dispatch_form, DynDispatchForm::Inline);
    }

    #[test]
    fn non_literal_initializer_yields_vtable() {
        let src = format!("{}defn make() -> List<dyn Greeter> {{ term [Dog {{ base: 1 }}]; }}\nlet handlers: List<dyn Greeter> = make();", trait_and_types());
        let items = prog(&src);
        let info = analyze(&items);
        let h = info.get("handlers").expect("handlers analyzed");
        assert!(h.concrete_set.is_empty());
        assert_eq!(h.dispatch_form, DynDispatchForm::Vtable);
    }

    #[test]
    fn five_elements_yield_vtable() {
        let types = "type Bird: Greeter { wing: Int; };\nimpl Bird { defn greet(me: Bird, times: Int) -> Int { term me.wing; } };\ntype Fish: Greeter { fin: Int; };\nimpl Fish { defn greet(me: Fish, times: Int) -> Int { term me.fin; } };\ntype Frog: Greeter { hop: Int; };\nimpl Frog { defn greet(me: Frog, times: Int) -> Int { term me.hop; } };\n";
        let src = format!(
            "{}{}let h: List<dyn Greeter> = [Dog {{ base: 1 }}, Cat {{ lives: 2 }}, Bird {{ wing: 3 }}, Fish {{ fin: 4 }}, Frog {{ hop: 5 }}];",
            trait_and_types(),
            types
        );
        let items = prog(&src);
        let info = analyze(&items);
        let h = info.get("h").expect("h analyzed");
        assert_eq!(h.concrete_set.len(), 5);
        assert_eq!(h.dispatch_form, DynDispatchForm::Vtable);
    }

    #[test]
    fn non_dyn_let_is_ignored() {
        let src = format!("{}let xs: List<Int> = [1, 2];", trait_and_types());
        let items = prog(&src);
        assert!(analyze(&items).is_empty());
    }

    #[test]
    fn dispatch_form_thresholds() {
        let mut s = BTreeSet::new();
        assert_eq!(dispatch_form_for(&s), DynDispatchForm::Vtable);
        s.insert("A".into());
        assert_eq!(dispatch_form_for(&s), DynDispatchForm::Inline);
        s.insert("B".into());
        s.insert("C".into());
        s.insert("D".into());
        assert_eq!(dispatch_form_for(&s), DynDispatchForm::Switch);
        s.insert("E".into());
        assert_eq!(dispatch_form_for(&s), DynDispatchForm::Vtable);
    }
}
