// Copyright 2026 Randy Smits-Schreuder Goedheijt
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Enum/small-sum representation analysis (P2-1, Phase A).
//!
//! Classifies enum-returning defns by whether every return form is a
//! constructor (or a match of constructors), which is the prerequisite for the
//! `InlineLocal` and `ReturnedSmall` (sret) lowering. Defaults to `Boxed` when
//! any fact is unknown; nothing here changes codegen.

use crate::ast::{EnumVariant, Expr, Statement, TopLevel};
use std::collections::{BTreeMap, HashMap};

/// Representation class for an enum-producing surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumRepresentation {
    /// Constructor value consumed entirely inside its defining function.
    InlineLocalCandidate,
    /// Internal defn whose return forms are all constructor-shaped.
    ReturnedSmallCandidate,
    /// Existing malloc-backed `{tag, payload}` ABI.
    Boxed,
}

#[derive(Debug, Clone, Default)]
pub struct EnumRepresentationPlan {
    /// Enum-returning defn → classification. Deterministic by name.
    pub defns: BTreeMap<String, EnumRepresentation>,
    /// All declared variant names (bare and `Enum::Variant` where derivable).
    pub variants: Vec<String>,
}

impl EnumRepresentationPlan {
    pub fn defn_class(&self, name: &str) -> EnumRepresentation {
        self.defns
            .get(name)
            .copied()
            .unwrap_or(EnumRepresentation::Boxed)
    }
}

/// Build the representation plan from the top-level surface.
///
/// This is deliberately conservative: only constructor-shaped return forms are
/// candidates. A local-let candidate is recorded only when the initializer is
/// an enum constructor and every use is a direct tag/payload consumer in the
/// same function; otherwise the value is boxed.
pub fn build_enum_representation_plan(items: &[TopLevel]) -> EnumRepresentationPlan {
    let mut variants = Vec::new();
    let mut enum_names = Vec::new();
    for item in items {
        match item {
            TopLevel::Enum(e) => {
                enum_names.push(e.name.clone());
                for v in &e.variants {
                    match v {
                        EnumVariant::Unit(n)
                        | EnumVariant::Tuple(n, _)
                        | EnumVariant::Struct(n, _) => {
                            variants.push(n.clone());
                            variants.push(format!("{}::{}", e.name, n));
                        }
                    }
                }
            }
            TopLevel::TypeDef(td) => {
                let is_enum = td.body.slots.iter().any(|s| s.name.starts_with("__variant_"));
                if is_enum {
                    enum_names.push(td.name.clone());
                    for slot in &td.body.slots {
                        if let Some(v) = slot.name.strip_prefix("__variant_") {
                            variants.push(v.to_string());
                            variants.push(format!("{}::{}", td.name, v));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    variants.sort();
    variants.dedup();

    let variant_set: HashMap<&str, ()> = variants.iter().map(|v| (v.as_str(), ())).collect();
    let enum_set: HashMap<&str, ()> = enum_names.iter().map(|n| (n.as_str(), ())).collect();
    let mut defns = BTreeMap::new();

    for item in items {
        let TopLevel::Definition(d) = item else { continue; };
        let return_is_enum = defn_return_type(d)
            .map(|t| type_is_enum(&t, &enum_set))
            .unwrap_or(false);
        if !return_is_enum {
            continue;
        }
        let class = if all_return_forms_constructor(d, &variant_set) {
            EnumRepresentation::ReturnedSmallCandidate
        } else {
            EnumRepresentation::Boxed
        };
        defns.insert(d.name.clone(), class);
    }

    EnumRepresentationPlan { defns, variants }
}

fn defn_return_type(d: &crate::ast::Definition) -> Option<crate::ast::Type> {
    if let Some(ot) = &d.output_type {
        return ot.all_types().into_iter().next();
    }
    d.outputs.first().cloned()
}

fn type_is_enum(ty: &crate::ast::Type, enum_set: &HashMap<&str, ()>) -> bool {
    match ty {
        crate::ast::Type::Custom(name) | crate::ast::Type::Applied(name, _) => {
            enum_set.contains_key(name.as_str())
        }
        _ => false,
    }
}

fn all_return_forms_constructor(
    d: &crate::ast::Definition,
    variants: &HashMap<&str, ()>,
) -> bool {
    let mut saw = false;
    for stmt in &d.body {
        let ret = match stmt {
            Statement::Term(Some(e)) | Statement::Tail(e) => Some(e),
            _ => None,
        };
        if let Some(e) = ret {
            saw = true;
            if !constructor_shaped_return(e, variants) {
                return false;
            }
        }
    }
    saw
}

/// A return expression is constructor-shaped when it is a constructor call,
/// a qualified variant field, or a match whose every arm is constructor-shaped.
fn constructor_shaped_return(expr: &Expr, variants: &HashMap<&str, ()>) -> bool {
    match expr {
        Expr::Call(name, _, _) => is_variant_name(name, variants),
        Expr::Field(_, name) => is_variant_name(name, variants),
        Expr::Match(_, arms) => arms
            .iter()
            .all(|arm| constructor_shaped_return(&arm.body, variants)),
        Expr::Block(stmts) => stmts
            .iter()
            .rev()
            .find_map(|s| match s {
                Statement::Expression(e) | Statement::Term(Some(e)) => Some(e),
                _ => None,
            })
            .map(|e| constructor_shaped_return(e, variants))
            .unwrap_or(false),
        _ => false,
    }
}

fn is_variant_name(name: &str, variants: &HashMap<&str, ()>) -> bool {
    variants.contains_key(name)
        || name
            .rsplit("::")
            .next()
            .map(|bare| variants.contains_key(bare))
            .unwrap_or(false)
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

    #[test]
    fn direct_constructor_return_is_candidate() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn f(x: Int) -> R { term Ok(x); };\n",
        );
        let plan = build_enum_representation_plan(&items);
        assert_eq!(
            plan.defn_class("f"),
            EnumRepresentation::ReturnedSmallCandidate
        );
    }

    #[test]
    fn match_of_constructors_is_candidate() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn f(x: Int) -> R { term match x { 0 => Ok(1), _ => Err(2) }; };\n",
        );
        let plan = build_enum_representation_plan(&items);
        assert_eq!(
            plan.defn_class("f"),
            EnumRepresentation::ReturnedSmallCandidate
        );
    }

    #[test]
    fn non_constructor_return_is_boxed() {
        let items = parse(
            "enum R { Ok(Int), Err(Int) };\n\
             defn f(x: R) -> R { term x; };\n",
        );
        let plan = build_enum_representation_plan(&items);
        assert_eq!(plan.defn_class("f"), EnumRepresentation::Boxed);
    }

    #[test]
    fn non_enum_return_is_not_in_plan() {
        let items = parse("defn f(x: Int) -> Int { term x; };\n");
        let plan = build_enum_representation_plan(&items);
        assert_eq!(plan.defn_class("f"), EnumRepresentation::Boxed);
    }
}
