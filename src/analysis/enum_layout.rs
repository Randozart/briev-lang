// Copyright 2026 Randy Smits-Schreuder Goedheijt
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Concrete enum image layout (P2-1 Option B, B1).
//!
//! The image is `1 tag word + max(variant payload words)`, with a descendant
//! enum laid out inline inside the payload area. Unknown, aggregate, or
//! recursive-beyond-cap payloads consume one boxed handle word.

use crate::ast::{EnumVariant, Statement, TopLevel, Type};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
struct EnumDef {
    params: Vec<String>,
    variants: Vec<(String, Type)>,
}

/// Physical representation selected by the enum value model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumValueRepr {
    ScalarValue,
    PackedMemory,
    OwnedBox,
}

/// Initial inline packed-memory threshold.
pub const PACKED_MEMORY_MAX_WORDS: usize = 4;

/// Small object payloads may live inside an internal enum image. The cap keeps
/// enum images bounded; larger objects keep the one-word boxed handle.
pub const INLINE_OBJECT_MAX_WORDS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumLayout {
    /// Total i64 words in the image (tag + payload area).
    pub image_words: usize,
    /// Per-variant payload word count, in declaration order.
    pub variant_payload_words: Vec<usize>,
    /// Per-variant flag: payload is a nested enum image laid out inline.
    pub variant_payload_inline: Vec<bool>,
    /// Per-variant flag: payload is a small object/struct byte image laid out
    /// inline. Mutually exclusive with `variant_payload_inline`.
    pub variant_payload_inline_object: Vec<bool>,
}

#[derive(Debug, Clone)]
pub struct EnumLayoutPlanner {
    enums: HashMap<String, EnumDef>,
    objects: HashMap<String, EnumDef>,
    /// Enums appearing in an exported signature keep the frozen boxed payload
    /// layout; object payloads are never inlined there.
    boundary_enums: HashSet<String>,
    max_depth: usize,
}

impl EnumLayoutPlanner {
    pub fn new(items: &[TopLevel], max_depth: usize) -> Self {
        let mut enums = HashMap::new();
        let mut objects = HashMap::new();
        for item in items {
            collect_declaration(item, &mut enums, &mut objects);
        }
        let boundary_enums = collect_boundary_enums(items, &enums);
        Self { enums, objects, boundary_enums, max_depth }
    }

    /// Layout for a concrete enum instance. `None` when the type is not a
    /// declared enum.
    pub fn layout_for(&self, ty: &Type) -> Option<EnumLayout> {
        self.layout_at(ty, 0)
    }

    fn layout_at(&self, ty: &Type, depth: usize) -> Option<EnumLayout> {
        let (name, args) = match ty {
            Type::Custom(n) => (n.as_str(), &[][..]),
            Type::Applied(n, a) => (n.as_str(), a.as_slice()),
            _ => return None,
        };
        let def = self.enums.get(name)?;
        let subst: HashMap<String, Type> = def
            .params
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        let allow_object_inline = !self.boundary_enums.contains(name);
        let mut variant_payload_words = Vec::with_capacity(def.variants.len());
        let mut variant_payload_inline = Vec::with_capacity(def.variants.len());
        let mut variant_payload_inline_object = Vec::with_capacity(def.variants.len());
        for (_, payload) in &def.variants {
            let concrete = if subst.is_empty() {
                payload.clone()
            } else {
                crate::typechecker::substitute_type(payload, &subst)
            };
            let inline_enum = self.is_inline_enum(&concrete, depth + 1);
            let inline_object =
                allow_object_inline && self.is_inline_object(&concrete, depth + 1);
            variant_payload_inline.push(inline_enum);
            variant_payload_inline_object.push(inline_object);
            let words = if inline_object {
                self.object_words(&concrete, depth + 1).unwrap_or(1)
            } else {
                self.payload_words(&concrete, depth + 1)
            };
            variant_payload_words.push(words);
        }
        let payload_words = variant_payload_words.iter().copied().max().unwrap_or(0).max(1);
        Some(EnumLayout {
            image_words: 1 + payload_words,
            variant_payload_words,
            variant_payload_inline,
            variant_payload_inline_object,
        })
    }

    fn is_inline_enum(&self, ty: &Type, depth: usize) -> bool {
        if depth >= self.max_depth {
            return false;
        }
        match ty {
            Type::Custom(n) => self.enums.contains_key(n),
            Type::Applied(n, _) => self.enums.contains_key(n),
            _ => false,
        }
    }

    fn is_inline_object(&self, ty: &Type, depth: usize) -> bool {
        if depth >= self.max_depth {
            return false;
        }
        self.object_words(ty, depth)
            .is_some_and(|words| words <= INLINE_OBJECT_MAX_WORDS)
    }

    fn object_words(&self, ty: &Type, depth: usize) -> Option<usize> {
        let (name, args) = match ty {
            Type::Custom(name) => (name.as_str(), &[][..]),
            Type::Applied(name, args) => (name.as_str(), args.as_slice()),
            _ => return None,
        };
        let def = self.objects.get(name)?;
        let subst: HashMap<String, Type> = def
            .params
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        let mut words = 0;
        for (_, field) in &def.variants {
            let concrete = if subst.is_empty() {
                field.clone()
            } else {
                crate::typechecker::substitute_type(field, &subst)
            };
            words += self.field_words(&concrete, depth + 1)?;
        }
        Some(words.max(1))
    }

    fn field_words(&self, ty: &Type, _depth: usize) -> Option<usize> {
        match ty {
            Type::Void => Some(0),
            Type::Bits(bits) => Some(((*bits as usize).saturating_add(63) / 64).max(1)),
            Type::Width(bits) => Some(((*bits as usize).saturating_add(63) / 64).max(1)),
            Type::Custom(_) | Type::Applied(_, _) | Type::Ptr(_) | Type::PtrConst(_)
            | Type::LayoutPtr(_) | Type::Number(_) => Some(1),
            Type::Generic(_, _) | Type::TypeVar(_) | Type::Constrained(_, _)
            | Type::Vector(_, _) | Type::Tuple(_) | Type::Union(_)
            | Type::Function(_, _) | Type::Dyn(_) | Type::Task(_) => None,
        }
    }

    /// Select the semantic value representation for a concrete enum.
    pub fn value_repr_for(&self, ty: &Type) -> Option<EnumValueRepr> {
        let layout = self.layout_for(ty)?;
        if layout.image_words <= 2 {
            Some(EnumValueRepr::ScalarValue)
        } else if layout.image_words <= PACKED_MEMORY_MAX_WORDS {
            Some(EnumValueRepr::PackedMemory)
        } else {
            Some(EnumValueRepr::OwnedBox)
        }
    }

    fn payload_words(&self, ty: &Type, depth: usize) -> usize {
        match ty {
            Type::Void => 0,
            Type::Tuple(elems) => elems.iter().map(|e| self.payload_words(e, depth)).sum(),
            // A concrete enum payload is inline while within the depth cap.
            Type::Custom(_) | Type::Applied(_, _) => {
                if depth < self.max_depth {
                    self.layout_at(ty, depth)
                        .map(|l| l.image_words)
                        .unwrap_or(1)
                } else {
                    1
                }
            }
            Type::Vector(inner, _) => self.payload_words(inner, depth).max(1),
            _ => 1,
        }
    }
}

// Keep this module independent of statement parsing internals.
#[allow(dead_code)]
fn _statement_marker(_: &Statement) {}

fn collect_declaration(
    item: &TopLevel,
    enums: &mut HashMap<String, EnumDef>,
    objects: &mut HashMap<String, EnumDef>,
) {
    match item {
        TopLevel::TypeDef(td) => collect_typedef(td, enums, objects),
        TopLevel::Enum(e) => collect_ast_enum(e, enums),
        TopLevel::StaticStruct(s) => {
            objects.insert(
                s.name.clone(),
                EnumDef {
                    params: s.type_params.iter().map(|p| p.name.clone()).collect(),
                    variants: s.fields.clone(),
                },
            );
        }
        _ => {}
    }
}

fn collect_typedef(
    td: &crate::ast::TypeDef,
    enums: &mut HashMap<String, EnumDef>,
    objects: &mut HashMap<String, EnumDef>,
) {
    let params: Vec<String> = td.type_params.iter().map(|p| p.name.clone()).collect();
    let variants: Vec<(String, Type)> = td
        .body
        .slots
        .iter()
        .filter_map(|s| {
            s.name
                .strip_prefix("__variant_")
                .map(|n| (n.to_string(), s.ty.clone()))
        })
        .collect();
    if !variants.is_empty() {
        enums.insert(td.name.clone(), EnumDef { params, variants });
        return;
    }
    let plain_object = !td.coll && td.parent.is_none() && td.protocol.is_none();
    let fields: Vec<(String, Type)> = td
        .body
        .slots
        .iter()
        .map(|slot| (slot.name.clone(), slot.ty.clone()))
        .collect();
    if plain_object && !fields.is_empty() {
        objects.insert(td.name.clone(), EnumDef { params, variants: fields });
    }
}

fn collect_ast_enum(e: &crate::ast::EnumDefinition, enums: &mut HashMap<String, EnumDef>) {
    let variants = e
        .variants
        .iter()
        .map(|v| match v {
            EnumVariant::Unit(n) => (n.clone(), Type::Void),
            EnumVariant::Tuple(n, ts) => {
                let payload = if ts.len() == 1 {
                    ts[0].clone()
                } else {
                    Type::Tuple(ts.clone())
                };
                (n.clone(), payload)
            }
            EnumVariant::Struct(n, fields) => {
                let payload = Type::Tuple(fields.iter().map(|(_, t)| t.clone()).collect());
                (n.clone(), payload)
            }
        })
        .collect();
    enums.insert(
        e.name.clone(),
        EnumDef {
            params: e.type_params.iter().map(|p| p.name.clone()).collect(),
            variants,
        },
    );
}

fn collect_boundary_enums(
    items: &[TopLevel],
    enums: &HashMap<String, EnumDef>,
) -> HashSet<String> {
    let mut out = HashSet::new();
    for item in items {
        let TopLevel::Export(export) = item else {
            continue;
        };
        match export.inner.as_ref() {
            TopLevel::Definition(d) => {
                collect_signature_enums(&d.parameters, d.output_type.as_ref(), enums, &mut out);
            }
            TopLevel::Transaction(t) => {
                collect_signature_enums(&t.parameters, t.output_type.as_ref(), enums, &mut out);
            }
            _ => {}
        }
    }
    out
}

fn collect_signature_enums(
    parameters: &[(String, Type)],
    output: Option<&crate::ast::OutputType>,
    enums: &HashMap<String, EnumDef>,
    out: &mut HashSet<String>,
) {
    for (_, ty) in parameters {
        collect_type_enums(ty, enums, out);
    }
    if let Some(output) = output {
        for ty in output.all_types() {
            collect_type_enums(&ty, enums, out);
        }
    }
}

fn collect_type_enums(ty: &Type, enums: &HashMap<String, EnumDef>, out: &mut HashSet<String>) {
    if let Some(name) = ty.universe_key() {
        if enums.contains_key(name) {
            out.insert(name.to_string());
        }
    }
    match ty {
        Type::Applied(_, args) | Type::Generic(_, args) => {
            for arg in args {
                collect_type_enums(arg, enums, out);
            }
        }
        Type::Tuple(items) => {
            for item in items {
                collect_type_enums(item, enums, out);
            }
        }
        Type::Union(items) => {
            for item in items {
                collect_type_enums(item, enums, out);
            }
        }
        Type::Ptr(inner) | Type::PtrConst(inner) | Type::Task(inner) | Type::Dyn(inner) => {
            collect_type_enums(inner, enums, out);
        }
        Type::Function(params, ret) => {
            for param in params {
                collect_type_enums(param, enums, out);
            }
            collect_type_enums(ret, enums, out);
        }
        Type::Constrained(inner, _) => collect_type_enums(inner, enums, out),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<TopLevel> {
        let toks = crate::lexer::tokenize(src).expect("tokenize");
        crate::parser::Parser::new(toks, src)
            .parse_program()
            .expect("parse")
    }

    fn planner() -> EnumLayoutPlanner {
        let items = parse(
            "enum TcpState { Closed, Listen };\n\
             enum FsmError { Illegal };\n\
             enum WsFrame { Empty };\n\
             enum Result<T, E> { Ok(T), Err(E) };\n",
        );
        EnumLayoutPlanner::new(&items, 2)
    }

    fn applied(name: &str, args: Vec<Type>) -> Type {
        Type::Applied(name.to_string(), args)
    }

    #[test]
    fn nested_enum_payload_is_inline() {
        let p = planner();
        let layout = p
            .layout_for(&applied(
                "Result",
                vec![Type::Custom("TcpState".into()), Type::Custom("FsmError".into())],
            ))
            .expect("layout");
        // TcpState image = 2, FsmError image = 2 -> Result image = 3.
        assert_eq!(layout.image_words, 3);
        assert_eq!(layout.variant_payload_words, vec![2, 2]);
        assert_eq!(layout.variant_payload_inline, vec![true, true]);
    }

    #[test]
    fn small_object_payload_is_inline_in_internal_enum() {
        let items = parse(
            "struct P { a: Int; b: Int; };\n\
             enum E { Wrap(P), Empty };\n",
        );
        let planner = EnumLayoutPlanner::new(&items, 2);
        let layout = planner.layout_for(&Type::Custom("E".to_string())).expect("layout");
        assert_eq!(layout.image_words, 3);
        assert_eq!(layout.variant_payload_inline_object, vec![true, false]);
        assert_eq!(layout.variant_payload_words, vec![2, 0]);
    }

    #[test]
    fn exported_enum_keeps_boxed_object_payload() {
        let items = parse(
            "struct P { a: Int; b: Int; };\n\
             enum E { Wrap(P), Empty };\n\
             export defn take(e: E) -> Int { term 0; };\n",
        );
        let planner = EnumLayoutPlanner::new(&items, 2);
        let layout = planner.layout_for(&Type::Custom("E".to_string())).expect("layout");
        assert_eq!(layout.image_words, 2);
        assert_eq!(layout.variant_payload_inline_object, vec![false, false]);
    }

    #[test]
    fn scalar_payload_is_flat() {
        let p = planner();
        let layout = p
            .layout_for(&applied("Result", vec![Type::int(), Type::int()]))
            .expect("layout");
        assert_eq!(layout.image_words, 2);
        assert_eq!(layout.variant_payload_words, vec![1, 1]);
    }

    #[test]
    fn two_word_enums_are_scalar_values() {
        let p = planner();
        let scalar = applied("Result", vec![Type::int(), Type::int()]);
        assert_eq!(p.value_repr_for(&scalar), Some(EnumValueRepr::ScalarValue));
    }

    #[test]
    fn nested_inline_enums_are_packed_memory() {
        let p = planner();
        let nested = applied(
            "Result",
            vec![Type::Custom("TcpState".into()), Type::Custom("FsmError".into())],
        );
        assert_eq!(p.value_repr_for(&nested), Some(EnumValueRepr::PackedMemory));
    }

    #[test]
    fn oversized_layouts_are_owned_boxes() {
        let items = parse(
            "enum Big { Five(Int, Int, Int, Int, Int) };\n",
        );
        let p = EnumLayoutPlanner::new(&items, 2);
        let big = Type::Custom("Big".into());
        let layout = p.layout_for(&big).expect("layout");
        assert_eq!(layout.image_words, 6);
        assert_eq!(p.value_repr_for(&big), Some(EnumValueRepr::OwnedBox));
    }

    #[test]
    fn depth_capped_nested_payload_does_not_box_the_outer_value() {
        let shallow = parse(
            "enum Result<T, E> { Ok(T), Err(E) };\n",
        );
        let p = EnumLayoutPlanner::new(&shallow, 1);
        let deep = applied(
            "Result",
            vec![
                applied("Result", vec![Type::int(), Type::int()]),
                Type::int(),
            ],
        );
        // Outer aggregate is two words: tag plus one owned nested handle.
        assert_eq!(p.value_repr_for(&deep), Some(EnumValueRepr::ScalarValue));
    }

    #[test]
    fn depth_cap_boxes_deeper_nesting() {
        let p = planner();
        let deep = applied(
            "Result",
            vec![applied("Result", vec![Type::int(), Type::int()]), Type::int()],
        );
        let layout = p.layout_for(&deep).expect("layout");
        // depth 0 Result: TcpState-like nested Result at depth 1 is inline
        // (2 words); overall image = 3.
        assert_eq!(layout.image_words, 3);
        let deeper = applied("Result", vec![deep, Type::int()]);
        // With a depth cap of 1, the nested Result is a boxed handle.
        let shallow = parse(
            "enum TcpState { Closed };\n\
             enum FsmError { Illegal };\n\
             enum Result<T, E> { Ok(T), Err(E) };\n",
        );
        let p1 = EnumLayoutPlanner::new(&shallow, 1);
        let layout = p1.layout_for(&deeper).expect("layout");
        assert_eq!(layout.image_words, 2);
    }
}
