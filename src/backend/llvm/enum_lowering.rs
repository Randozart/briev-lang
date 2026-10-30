// ── Joined enum lowering plan ─────────────────────────────────────────
// 2026-10-30 (Phase D3b, plan
// docs/plans/2026-10-30-generalized-value-destination-ownership.md): join the
// frontend's semantic enum facts with the target-configured physical layout
// planner once, so construction, match/projection, and call-site ABI paths all
// read the same entry.
//
// The join is conservative: a concrete type with no declaration/layout match
// is absent and callers retain the legacy boxed fallback. This module does not
// inspect Briev type names as physical truth; names identify declared enums,
// while layout comes from EnumLayoutPlanner.
//
// Undo: delete this module and the `enum_lowering` context field, then restore
// emitter reads of `enum_layout_planner`. No ABI or source syntax depends on it.

use crate::analysis::destination_model::{
    CallableId, EnumCallablePlan, EnumDefinitionPlan, EnumResultRepresentation,
    GeneralizedDestinationOwnershipPlan,
};
use crate::analysis::enum_layout::{
    EnumLayout, EnumLayoutPlanner, EnumValueRepr, PACKED_MEMORY_MAX_WORDS,
};
use crate::ast::Type;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariantLowering {
    pub name: String,
    pub tag: usize,
    pub payload: Option<Type>,
    pub payload_words: usize,
    pub payload_inline: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumLoweringEntry {
    pub ty: Type,
    pub layout: EnumLayout,
    pub representation: EnumValueRepr,
    pub variants: Vec<EnumVariantLowering>,
}

impl EnumLoweringEntry {
    pub fn variant(&self, name: &str) -> Option<&EnumVariantLowering> {
        let bare = name.rsplit("::").next().unwrap_or(name);
        self.variants
            .iter()
            .find(|variant| variant.name == bare)
    }

    pub fn variant_at(&self, tag: usize) -> Option<&EnumVariantLowering> {
        self.variants.get(tag)
    }
}

/// Deterministic backend-side join keyed by concrete enum type.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnumLoweringPlan {
    entries: Vec<EnumLoweringEntry>,
    callables: BTreeMap<CallableId, EnumCallablePlan>,
}

impl EnumLoweringPlan {
    pub fn build(
        semantic: &GeneralizedDestinationOwnershipPlan,
        layouts: &EnumLayoutPlanner,
    ) -> Self {
        let mut candidates = collect_candidates(semantic);
        expand_nested_candidates(&mut candidates, semantic);
        candidates.sort_by_key(type_sort_key);
        let entries = candidates
            .into_iter()
            .filter_map(|ty| build_entry(&ty, semantic, layouts))
            .collect();
        EnumLoweringPlan {
            entries,
            callables: semantic.enum_callables.clone(),
        }
    }

    pub fn entry(&self, ty: &Type) -> Option<&EnumLoweringEntry> {
        self.entries.iter().find(|entry| &entry.ty == ty)
    }

    pub fn layout(&self, ty: &Type) -> Option<&EnumLayout> {
        self.entry(ty).map(|entry| &entry.layout)
    }

    pub fn variant(&self, ty: &Type, name: &str) -> Option<&EnumVariantLowering> {
        self.entry(ty)?.variant(name)
    }

    pub fn variant_at(&self, ty: &Type, tag: usize) -> Option<&EnumVariantLowering> {
        self.entry(ty)?.variant_at(tag)
    }

    pub fn callable(&self, callable: &CallableId) -> Option<&EnumCallablePlan> {
        self.callables.get(callable)
    }

    pub fn hidden_result_callable(&self, name: &str) -> bool {
        self.hidden_result_callables().contains(name)
    }

    pub fn hidden_result_callables(&self) -> std::collections::BTreeSet<String> {
        self.callables
            .values()
            .filter(|plan| {
                plan.representation == EnumResultRepresentation::CallerDestination
                    && plan.hidden_destination
            })
            .map(|plan| plan.callable.name.clone())
            .collect()
    }

    pub fn entries(&self) -> &[EnumLoweringEntry] {
        &self.entries
    }

    /// Cheap constructor/match agreement check. An absent entry is not an
    /// error (legacy fallback); a present entry must name the same variant at
    /// the tag the legacy constructor map selected.
    pub fn agrees_with_tag(&self, ty: &Type, tag: usize, name: &str) -> bool {
        match self.variant_at(ty, tag) {
            Some(variant) => {
                variant.name == name.rsplit("::").next().unwrap_or(name)
            }
            None => true,
        }
    }

    /// Structural validation of the joined entry. Returns the first mismatch
    /// so tests and debug assertions can name the broken invariant.
    pub fn validate(&self) -> Result<(), String> {
        for entry in &self.entries {
            validate_entry(entry)?;
        }
        Ok(())
    }
}

fn validate_entry(entry: &EnumLoweringEntry) -> Result<(), String> {
    validate_lengths(entry)?;
    validate_variants(entry)?;
    validate_representation(entry)
}

fn validate_lengths(entry: &EnumLoweringEntry) -> Result<(), String> {
    if entry.variants.len() != entry.layout.variant_payload_words.len() {
        return Err(format!("{:?}: variant/payload width length mismatch", entry.ty));
    }
    if entry.variants.len() != entry.layout.variant_payload_inline.len() {
        return Err(format!("{:?}: variant/inline length mismatch", entry.ty));
    }
    Ok(())
}

fn validate_variants(entry: &EnumLoweringEntry) -> Result<(), String> {
    for (expected_tag, variant) in entry.variants.iter().enumerate() {
        if variant.tag != expected_tag {
            return Err(format!(
                "{:?}: variant {} has tag {}, expected {}",
                entry.ty, variant.name, variant.tag, expected_tag
            ));
        }
        if variant.payload_words != entry.layout.variant_payload_words[expected_tag] {
            return Err(format!(
                "{:?}: variant {} payload width disagrees with layout",
                entry.ty, variant.name
            ));
        }
        if variant.payload_inline != entry.layout.variant_payload_inline[expected_tag] {
            return Err(format!(
                "{:?}: variant {} inline flag disagrees with layout",
                entry.ty, variant.name
            ));
        }
    }
    Ok(())
}

fn validate_representation(entry: &EnumLoweringEntry) -> Result<(), String> {
    let expected_repr = if entry.layout.image_words <= 2 {
        EnumValueRepr::ScalarValue
    } else if entry.layout.image_words <= PACKED_MEMORY_MAX_WORDS {
        EnumValueRepr::PackedMemory
    } else {
        EnumValueRepr::OwnedBox
    };
    if entry.representation == expected_repr {
        return Ok(());
    }
    Err(format!(
        "{:?}: representation {:?} disagrees with image width {}",
        entry.ty, entry.representation, entry.layout.image_words
    ))
}

fn collect_candidates(semantic: &GeneralizedDestinationOwnershipPlan) -> Vec<Type> {
    let mut out = Vec::new();
    for definition in semantic.enums.values() {
        if definition.type_params.is_empty() {
            push_unique(&mut out, Type::Custom(definition.name.clone()));
        }
    }
    for fact in semantic.values.values() {
        if let Some(ty) = &fact.ty {
            add_if_declared_enum(&mut out, ty, semantic);
        }
    }
    let callable_types = semantic.callables.values().flat_map(|callable| {
        callable
            .parameters
            .iter()
            .filter_map(|parameter| parameter.ty.as_ref())
            .chain(callable.result.ty.iter())
    });
    for ty in callable_types {
        add_if_declared_enum(&mut out, ty, semantic);
    }
    for callable in semantic.enum_callables.values() {
        if let Some(ty) = &callable.result_type {
            push_unique(&mut out, ty.clone());
        }
    }
    out
}

fn add_if_declared_enum(
    out: &mut Vec<Type>,
    ty: &Type,
    semantic: &GeneralizedDestinationOwnershipPlan,
) {
    if is_declared_enum(ty, semantic) {
        push_unique(out, ty.clone());
    }
}

fn expand_nested_candidates(
    candidates: &mut Vec<Type>,
    semantic: &GeneralizedDestinationOwnershipPlan,
) {
    let mut index = 0;
    while index < candidates.len() {
        let ty = candidates[index].clone();
        extend_from_variants(candidates, &ty, semantic);
        index += 1;
    }
}

fn extend_from_variants(
    candidates: &mut Vec<Type>,
    ty: &Type,
    semantic: &GeneralizedDestinationOwnershipPlan,
) {
    let Some((definition, subst)) = declared_enum(ty, semantic) else {
        return;
    };
    for variant in &definition.variants {
        let Some(payload) = &variant.payload else {
            continue;
        };
        let concrete = substitute(payload, &subst);
        add_if_declared_enum(candidates, &concrete, semantic);
    }
}

fn build_entry(
    ty: &Type,
    semantic: &GeneralizedDestinationOwnershipPlan,
    layouts: &EnumLayoutPlanner,
) -> Option<EnumLoweringEntry> {
    let layout = layouts.layout_for(ty)?;
    let representation = layouts.value_repr_for(ty)?;
    let (definition, subst) = declared_enum(ty, semantic)?;
    if definition.variants.len() != layout.variant_payload_words.len() {
        return None;
    }
    let variants = definition
        .variants
        .iter()
        .enumerate()
        .map(|(tag, variant)| EnumVariantLowering {
            name: variant.name.clone(),
            tag,
            payload: variant.payload.as_ref().map(|ty| substitute(ty, &subst)),
            payload_words: layout.variant_payload_words[tag],
            payload_inline: layout.variant_payload_inline[tag],
        })
        .collect();
    Some(EnumLoweringEntry {
        ty: ty.clone(),
        layout,
        representation,
        variants,
    })
}

fn declared_enum<'a>(
    ty: &Type,
    semantic: &'a GeneralizedDestinationOwnershipPlan,
) -> Option<(&'a EnumDefinitionPlan, HashMap<String, Type>)> {
    let (name, args) = match ty {
        Type::Custom(name) => (name.as_str(), &[][..]),
        Type::Applied(name, args) => (name.as_str(), args.as_slice()),
        _ => return None,
    };
    let definition = semantic.enums.get(name)?;
    let subst = definition
        .type_params
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect();
    Some((definition, subst))
}

fn substitute(ty: &Type, subst: &HashMap<String, Type>) -> Type {
    if subst.is_empty() {
        return ty.clone();
    }
    crate::typechecker::substitute_type(ty, subst)
}

fn is_declared_enum(ty: &Type, semantic: &GeneralizedDestinationOwnershipPlan) -> bool {
    match ty {
        Type::Custom(name) | Type::Applied(name, _) => semantic.enums.contains_key(name),
        Type::Union(_) => false,
        Type::Generic(_, args) => args.iter().any(|ty| is_declared_enum(ty, semantic)),
        Type::Constrained(inner, _) => is_declared_enum(inner, semantic),
        _ => false,
    }
}

fn push_unique(out: &mut Vec<Type>, ty: Type) {
    if !out.iter().any(|existing| existing == &ty) {
        out.push(ty);
    }
}

fn type_sort_key(ty: &Type) -> String {
    match ty {
        Type::Custom(name) => name.clone(),
        Type::Applied(name, args) => {
            let parts: Vec<String> = args.iter().map(type_sort_key).collect();
            format!("{name}<{}>", parts.join(","))
        }
        Type::Generic(name, args) => {
            let parts: Vec<String> = args.iter().map(type_sort_key).collect();
            format!("generic:{name}<{}>", parts.join(","))
        }
        Type::Union(items) => {
            let parts: Vec<String> = items.iter().map(type_sort_key).collect();
            format!("union<{}>", parts.join(","))
        }
        Type::Constrained(inner, _) => type_sort_key(inner),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::destination_model::GeneralizedDestinationOwnershipPlan;

    fn parse(src: &str) -> Vec<crate::ast::TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    fn joined(src: &str, depth: usize) -> (GeneralizedDestinationOwnershipPlan, EnumLoweringPlan) {
        let items = parse(src);
        let semantic = crate::analysis::destination_ownership::analyze(&items).generalized;
        let layouts = EnumLayoutPlanner::new(&items, depth);
        let plan = EnumLoweringPlan::build(&semantic, &layouts);
        (semantic, plan)
    }

    #[test]
    fn join_matches_layout_and_declared_variants() {
        let (_, plan) = joined(
            r#"
enum R { Ok(Int), Err(String) };
defn make(x: Int) -> R { term Ok(x); };
"#,
            2,
        );
        let entry = plan.entry(&Type::Custom("R".to_string())).expect("entry");
        assert_eq!(entry.layout.image_words, 2);
        assert_eq!(entry.representation, EnumValueRepr::ScalarValue);
        assert_eq!(entry.variants[0].name, "Ok");
        assert_eq!(entry.variants[0].tag, 0);
        assert_eq!(entry.variants[0].payload, Some(Type::int()));
        assert_eq!(entry.variants[1].name, "Err");
        assert!(plan.agrees_with_tag(&Type::Custom("R".to_string()), 0, "R::Ok"));
        assert!(!plan.agrees_with_tag(&Type::Custom("R".to_string()), 0, "Err"));
        assert!(plan.hidden_result_callable("make"));
    }

    #[test]
    fn nested_enum_payload_is_inline_in_join() {
        let (_, plan) = joined(
            r#"
enum Inner { A(Int), B(Int) };
enum Outer { Wrap(Inner), Empty };
defn make(x: Int) -> Outer { term Wrap(A(x)); };
"#,
            2,
        );
        let entry = plan
            .entry(&Type::Custom("Outer".to_string()))
            .expect("outer entry");
        assert!(entry.variants[0].payload_inline);
        assert_eq!(entry.variants[0].payload_words, 2);
        assert!(plan.entry(&Type::Custom("Inner".to_string())).is_some());
    }

    #[test]
    fn generic_payload_is_substituted() {
        let (_, plan) = joined(
            r#"
enum Box<T> { Some(T), None };
defn make(x: Int) -> Box<Int> { term Some(x); };
"#,
            2,
        );
        let ty = Type::Applied("Box".to_string(), vec![Type::int()]);
        let entry = plan.entry(&ty).expect("generic entry");
        assert_eq!(entry.variants[0].payload, Some(Type::int()));
    }

    #[test]
    fn unknown_concrete_layout_is_absent_not_guessed() {
        let (_, plan) = joined(
            r#"
enum R { Ok(Int), Err(Int) };
defn make(x: Int) -> R { term Ok(x); };
"#,
            2,
        );
        assert!(plan.entry(&Type::Custom("Missing".to_string())).is_none());
    }

    #[test]
    fn every_entry_agrees_with_layout_planner() {
        let src = r#"
enum Inner { A(Int), B(Int) };
enum Outer { Wrap(Inner), Empty };
defn make(x: Int) -> Outer { term Wrap(A(x)); };
"#;
        let items = parse(src);
        let semantic = crate::analysis::destination_ownership::analyze(&items).generalized;
        let layouts = EnumLayoutPlanner::new(&items, 2);
        let plan = EnumLoweringPlan::build(&semantic, &layouts);
        assert!(!plan.entries().is_empty());
        plan.validate().expect("valid joined plan");
        for entry in plan.entries() {
            assert_eq!(Some(&entry.layout), layouts.layout_for(&entry.ty).as_ref());
            assert_eq!(
                Some(entry.representation),
                layouts.value_repr_for(&entry.ty)
            );
        }
    }
}
